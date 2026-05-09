//! Filesystem watcher for live JSONL file ingestion.
//!
//! This module provides the real-time ingestion pipeline: notify-based filesystem
//! watching with per-file debounce, an async processing loop that triggers
//! `sync_file` for each changed .jsonl file, and emits all seven SSE event types
//! (record:added, session:started, schema:drift, version:changed, file:written,
//! file:edited, git:commit) through the broadcast channel. Version changes are
//! also persisted to the version_history table for historical tracking.
//!
//! Architecture:
//! - `spawn_watcher` creates a blocking std::thread with the notify watcher
//!   (the watcher must live on a real thread, not a tokio task, because it
//!   blocks indefinitely and must not be dropped)
//! - `watcher_loop` runs as a tokio task, receiving filesystem events through
//!   an mpsc channel and processing them asynchronously
//! - FTS5 indexes (message content and file operations) are rebuilt periodically
//!   (every 30 seconds) rather than per-file to avoid excessive rebuild overhead
//!   during rapid ingestion
//!
//! Requirement IDs: WATCH-01, WATCH-02, WATCH-03, SSE-02, SSE-03, SSE-04, SSE-05,
//!                  SSE-06, SSE-07

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc};
use tokio_util::sync::CancellationToken;

use tokio_rusqlite::rusqlite;

use crate::events::SseEvent;

/// Per-file debounce tracker to avoid re-syncing a file that was just synced.
///
/// Claude Code appends to JSONL files rapidly during active sessions. Without
/// debounce, the notify watcher would trigger sync_file for every individual
/// write, causing redundant work. The 2-second debounce window coalesces rapid
/// writes into a single sync operation.
///
/// Requirement ID: WATCH-02
pub struct FileDebouncer {
    /// Tracks the last time each file was synced, keyed by canonical path.
    last_synced: HashMap<PathBuf, Instant>,
    /// Minimum duration between syncs for the same file.
    debounce_duration: Duration,
}

impl FileDebouncer {
    /// Create a new debouncer with the given minimum gap between syncs (in seconds).
    pub fn new(debounce_secs: u64) -> Self {
        Self {
            last_synced: HashMap::new(),
            debounce_duration: Duration::from_secs(debounce_secs),
        }
    }

    /// Returns true if enough time has elapsed since the last sync of this file.
    ///
    /// Updates the last_synced timestamp on true (the caller is expected to
    /// proceed with sync_file). Returns false if the debounce window has not
    /// elapsed (the caller should skip this event).
    pub fn should_sync(&mut self, path: &PathBuf) -> bool {
        let now = Instant::now();
        if let Some(last) = self.last_synced.get(path) {
            if now.duration_since(*last) < self.debounce_duration {
                return false;
            }
        }
        self.last_synced.insert(path.clone(), now);
        true
    }

    /// Remove entries older than `max_age` to prevent unbounded HashMap growth.
    ///
    /// Called periodically from the watcher loop (every ~100 events or every
    /// minute, whichever comes first). Files that have not been modified for
    /// longer than `max_age` are unlikely to trigger events again soon.
    ///
    /// Addresses Research Pitfall 5 (unbounded memory growth from long-running
    /// watcher accumulating entries for files that are no longer active).
    pub fn prune_stale(&mut self, max_age: Duration) {
        let now = Instant::now();
        self.last_synced
            .retain(|_, last| now.duration_since(*last) < max_age);
    }
}

/// Internal state for the watcher loop, bundling debouncer with version tracking
/// and FTS rebuild scheduling.
struct WatcherState {
    /// Per-file debounce tracker.
    debouncer: FileDebouncer,
    /// Last known Claude Code version string, used for SSE-05 version:changed
    /// detection. Initialized from the database at startup so version events
    /// only fire when the version genuinely differs from the last-known value.
    last_known_version: Option<String>,
    /// True if any data has been ingested since the last FTS rebuild.
    /// Used to skip unnecessary rebuilds on the 30-second timer.
    data_ingested_since_fts_rebuild: bool,
    /// Timestamp of the last FTS rebuild, for the 30-second interval check.
    last_fts_rebuild: Instant,
}

/// Spawn the notify filesystem watcher on a dedicated blocking thread.
///
/// Creates a `notify::recommended_watcher` that monitors `projects_dir`
/// recursively for filesystem changes. Events are forwarded through the
/// provided mpsc channel sender using `blocking_send` (the notify callback
/// runs on a non-async thread, so `.await` is not available).
///
/// The watcher lives on a `std::thread` (not `tokio::spawn_blocking`) because
/// it must block indefinitely. The thread is parked after setup to prevent
/// the watcher from being dropped (Research Pitfall 1: watcher is dropped
/// when the creating function returns).
///
/// Returns Ok(()) after the thread is spawned. Errors during watcher creation
/// are propagated. The thread itself will live until the process exits.
///
/// Requirement IDs: WATCH-01, WATCH-03
pub fn spawn_watcher(
    projects_dir: PathBuf,
    tx: mpsc::Sender<notify::Result<notify::Event>>,
) -> notify::Result<()> {
    use notify::{RecursiveMode, Watcher};

    // Create the watcher on the current thread first to catch creation errors,
    // then move it into the spawned thread where it will live indefinitely.
    //
    // Actually, we need to create it inside the thread because the watcher's
    // callback captures the tx sender and must live as long as the watcher.
    // We use a oneshot channel to propagate creation errors back to the caller.
    let (setup_tx, setup_rx) = std::sync::mpsc::channel::<notify::Result<()>>();

    std::thread::spawn(move || {
        let event_tx = tx;
        let mut watcher = match notify::recommended_watcher(move |result| {
            // try_send is used because this callback runs on the OS filesystem
            // event thread (FSEvents on macOS). Using blocking_send here would
            // stall the FSEvents callback thread when the channel is full,
            // causing macOS to coalesce subsequent events into directory-level
            // notifications that silently fail the .jsonl extension filter.
            // try_send returns immediately, preventing FSEvents thread stalling.
            if let Err(e) = event_tx.try_send(result) {
                tracing::warn!("Watcher channel full or closed, event dropped: {}", e);
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                let _ = setup_tx.send(Err(e));
                return;
            }
        };

        match watcher.watch(&projects_dir, RecursiveMode::Recursive) {
            Ok(()) => {
                tracing::info!(
                    path = %projects_dir.display(),
                    "Filesystem watcher established for projects directory"
                );
                let _ = setup_tx.send(Ok(()));
            }
            Err(e) => {
                let _ = setup_tx.send(Err(e));
                return;
            }
        }

        // Park the thread forever. The watcher is kept alive as a local variable
        // in this scope. If this function returned, the watcher would be dropped
        // and filesystem monitoring would stop (Research Pitfall 1).
        //
        // The _watcher variable is intentionally kept in scope to prevent drop.
        let _watcher = watcher;
        std::thread::park();
    });

    // Wait for the setup result from the spawned thread.
    // The recv() will block until the watcher thread sends Ok or Err.
    setup_rx
        .recv()
        .unwrap_or_else(|_| Err(notify::Error::generic("Watcher thread died during setup")))
}

/// Query the current maximum rowid from file_operations and git_operations.
///
/// Used as a "before sync" snapshot so that after sync_file completes, we can
/// query for rows with id > this snapshot to find newly-created artifact rows
/// and emit corresponding SSE events.
///
/// Returns (max_file_op_id, max_git_op_id). Returns 0 if the tables are empty.
async fn snapshot_artifact_ids(conn: &tokio_rusqlite::Connection) -> (i64, i64) {
    let result = conn
        .call(|conn| -> Result<_, rusqlite::Error> {
            let file_max: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(id), 0) FROM file_operations",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            let git_max: i64 = conn
                .query_row(
                    "SELECT COALESCE(MAX(id), 0) FROM git_operations",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok((file_max, git_max))
        })
        .await;

    result.unwrap_or((0, 0))
}

/// Emit SSE events for file operations and git operations created after the
/// given ID snapshots.
///
/// Queries file_operations for write/edit rows with id > file_op_snapshot and
/// git_operations for commit rows with id > git_op_snapshot, then sends the
/// corresponding FileWritten, FileEdited, or GitCommit events through the
/// broadcast channel.
///
/// Query failures are logged at debug level and do not prevent other events
/// from being emitted.
///
/// Requirement IDs: SSE-06, SSE-07
async fn emit_artifact_events(
    conn: &tokio_rusqlite::Connection,
    session_id: &str,
    file_op_snapshot: i64,
    git_op_snapshot: i64,
    event_tx: &broadcast::Sender<SseEvent>,
) {
    // SSE-06: file:written and file:edited events
    let sid = session_id.to_string();
    let file_ops = conn
        .call(move |conn| -> Result<_, rusqlite::Error> {
            let mut stmt = conn.prepare(
                "SELECT file_path, operation_type, message_uuid
                 FROM file_operations
                 WHERE session_id = ?1 AND id > ?2
                   AND operation_type IN ('write', 'edit')",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![sid, file_op_snapshot], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await;

    match file_ops {
        Ok(ops) => {
            for (file_path, op_type, message_uuid) in ops {
                match op_type.as_str() {
                    "write" => {
                        let _ = event_tx.send(SseEvent::FileWritten {
                            session_id: session_id.to_string(),
                            file_path,
                            message_uuid,
                        });
                    }
                    "edit" => {
                        let _ = event_tx.send(SseEvent::FileEdited {
                            session_id: session_id.to_string(),
                            file_path,
                            message_uuid,
                        });
                    }
                    _ => {} // Other operation types do not produce SSE events
                }
            }
        }
        Err(e) => {
            tracing::debug!(
                error = %e,
                session_id = session_id,
                "Failed to query new file operations for SSE events"
            );
        }
    }

    // SSE-07: git:commit events
    let sid = session_id.to_string();
    let git_ops = conn
        .call(move |conn| -> Result<_, rusqlite::Error> {
            let mut stmt = conn.prepare(
                "SELECT commit_message, branch, message_uuid
                 FROM git_operations
                 WHERE session_id = ?1 AND id > ?2
                   AND operation_type = 'commit'",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![sid, git_op_snapshot], |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await;

    match git_ops {
        Ok(ops) => {
            for (commit_message, branch, message_uuid) in ops {
                let _ = event_tx.send(SseEvent::GitCommit {
                    session_id: session_id.to_string(),
                    commit_message,
                    branch,
                    message_uuid,
                });
            }
        }
        Err(e) => {
            tracing::debug!(
                error = %e,
                session_id = session_id,
                "Failed to query new git operations for SSE events"
            );
        }
    }
}

/// Check whether a file has been previously synced (exists in sync_metadata).
///
/// Returns true if the file has NOT been synced before (count == 0 in
/// sync_metadata). This is used to determine whether to emit a
/// session:started SSE event for the first ingestion of a new session file.
///
/// On query error, conservatively returns true (treat as new file) — this
/// may cause a spurious session:started event, which is preferable to
/// missing a genuine new-session notification.
///
/// Requirement ID: SSE-03
async fn is_new_file(conn: &tokio_rusqlite::Connection, file_path: &str) -> bool {
    let fp = file_path.to_string();
    let result = conn
        .call(move |conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM sync_metadata WHERE file_path = ?1",
                [&fp],
                |row| row.get::<_, i64>(0),
            )
        })
        .await;

    match result {
        Ok(count) => count == 0,
        Err(e) => {
            tracing::debug!(
                error = %e,
                file_path = file_path,
                "Failed to check sync_metadata for new file detection, treating as new"
            );
            true
        }
    }
}

/// Check if the Claude Code version has changed and emit version:changed SSE event.
///
/// Queries the sessions table for the version associated with the given session_id.
/// If the version differs from the cached `last_known_version` in WatcherState,
/// updates the cache and sends an `SseEvent::VersionChanged` through the broadcast
/// channel.
///
/// Query failures are logged at debug level and do not emit events — transient
/// database contention during rapid ingestion should not produce false positives.
///
/// Requirement ID: SSE-05
async fn check_version_change(
    state: &mut WatcherState,
    conn: &tokio_rusqlite::Connection,
    session_id: &str,
    event_tx: &broadcast::Sender<SseEvent>,
) {
    let sid = session_id.to_string();
    let version_result = conn
        .call(move |conn| {
            conn.query_row(
                "SELECT version FROM sessions WHERE session_id = ?1",
                [&sid],
                |row| row.get::<_, Option<String>>(0),
            )
        })
        .await;

    match version_result {
        Ok(Some(new_version)) => {
            let changed = match &state.last_known_version {
                Some(old) => old != &new_version,
                None => true,
            };
            if changed {
                let old_version = state.last_known_version.take();
                let _ = event_tx.send(SseEvent::VersionChanged {
                    old_version: old_version.clone(),
                    new_version: new_version.clone(),
                    session_id: session_id.to_string(),
                });
                tracing::info!(
                    old = ?old_version,
                    new = %new_version,
                    "Claude Code version change detected"
                );
                state.last_known_version = Some(new_version.clone());

                // Persist version change to version_history table.
                // This happens AFTER SSE event emission so a DB failure
                // does not prevent event delivery.
                let ver = new_version.clone();
                let sid = session_id.to_string();
                // D3: re-derive session_count from the sessions table on every
                // upsert rather than incrementing by 1 per version-change event.
                // The backfill paths (D1's sync_all backfill and the watcher's
                // gated startup backfill above) already source session_count as
                // COUNT(*) FROM sessions GROUP BY version. Aligning the live
                // ON CONFLICT path with that pattern aims to give every row in
                // version_history a consistent semantic — distinct sessions per
                // version per the sessions table at upsert time — rather than a
                // mix of "distinct count" (backfilled rows) and "version-change
                // event count" (rows ever updated by this live path).
                let persist_result = conn.call(move |conn| {
                    conn.execute(
                        "INSERT INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
                         VALUES (?1, datetime('now'), datetime('now'), ?2, 1)
                         ON CONFLICT(version) DO UPDATE SET
                           last_seen_at = datetime('now'),
                           session_count = (SELECT COUNT(*) FROM sessions WHERE version = ?1)",
                        rusqlite::params![ver, sid],
                    )
                }).await;

                if let Err(e) = persist_result {
                    tracing::warn!(
                        error = %e,
                        version = %new_version,
                        "Failed to persist version change to version_history table — SSE event was still emitted"
                    );
                }
            }
        }
        Ok(None) => {
            tracing::debug!(
                session_id = session_id,
                "Session has no version field, skipping version check"
            );
        }
        Err(e) => {
            tracing::debug!(
                error = %e,
                session_id = session_id,
                "Failed to query session version, skipping version check"
            );
        }
    }
}

/// Main processing loop for filesystem watcher events.
///
/// Receives notify events through the mpsc channel, filters to .jsonl files,
/// applies per-file debounce, calls sync_file for each eligible file, and
/// emits the appropriate SSE events based on sync results:
///
/// - `record:added` — when sync_file returns records_synced > 0 (SSE-02)
/// - `session:started` — when the file was not previously in sync_metadata (SSE-03)
/// - `schema:drift` — when overflow_fields_logged > 0 (SSE-04)
/// - `version:changed` — when the session's version differs from last known (SSE-05)
///
/// Also manages periodic FTS5 index rebuilds (every 30 seconds, only when new
/// data has been ingested) and debouncer pruning to prevent unbounded memory
/// growth.
///
/// The loop exits when the CancellationToken is cancelled (graceful shutdown)
/// or the mpsc channel is closed (watcher thread died).
///
/// Requirement IDs: WATCH-01, WATCH-02, WATCH-03, SSE-02, SSE-03, SSE-04, SSE-05
pub async fn watcher_loop(
    mut rx: mpsc::Receiver<notify::Result<notify::Event>>,
    conn: tokio_rusqlite::Connection,
    event_tx: broadcast::Sender<SseEvent>,
    cancel: CancellationToken,
    sync_all_done: tokio::sync::oneshot::Receiver<()>,
) {
    // Initialize last_known_version from the database. This sets a baseline from
    // existing data so version:changed events only fire when the version genuinely
    // differs from the last-known DB value (avoids spurious events on daemon restart).
    let initial_version = conn
        .call(|conn| {
            conn.query_row(
                "SELECT version FROM sessions ORDER BY rowid DESC LIMIT 1",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
        })
        .await
        .ok()
        .flatten();

    if let Some(ref v) = initial_version {
        tracing::info!(version = %v, "Initialized watcher with last known version from DB");
    }

    let mut state = WatcherState {
        debouncer: FileDebouncer::new(2),
        last_known_version: initial_version,
        data_ingested_since_fts_rebuild: false,
        last_fts_rebuild: Instant::now(),
    };

    // Cold-boot ordering: wait for sync_all (spawned in serve.rs) to finish before
    // running the startup version_history backfill. Without this gate, the backfill
    // here can race against sync_all's writes to the sessions table and miss
    // newer-version rows that sync_all is still in the process of inserting.
    //
    // Only this one-shot startup backfill query is gated. The select! loop below
    // (live filesystem events and cancellation) is intentionally NOT gated, so the
    // watcher remains responsive to live ingestion events that arrive concurrently
    // with the catch-up sync_all.
    //
    // If the sender is dropped without firing (would happen if the sync_all task
    // panicked before its send line, or if spawn_watcher failed and serve.rs never
    // wired the sender), fall through to running the backfill best-effort and emit
    // a warning so the lost-signal case is visible in logs.
    match sync_all_done.await {
        Ok(()) => {
            tracing::debug!("Watcher received sync_all completion signal, running startup backfill");
        }
        Err(_) => {
            tracing::warn!(
                "sync_all completion signal lost (sender dropped without firing) — \
                 running startup version_history backfill best-effort"
            );
        }
    }

    // Backfill version_history from sessions table on startup.
    // This aims to ensure version_history is populated even if the migration's
    // backfill ran on an empty database (first-time setup).
    let backfill_result = conn.call(|conn| {
        conn.execute_batch(
            "INSERT OR IGNORE INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
             SELECT
                 version,
                 MIN(first_seen_at),
                 MAX(COALESCE(last_seen_at, first_seen_at)),
                 (SELECT s2.session_id FROM sessions s2 WHERE s2.version = sessions.version
                  ORDER BY s2.first_seen_at ASC LIMIT 1),
                 COUNT(*)
             FROM sessions
             WHERE version IS NOT NULL AND version != ''
             GROUP BY version"
        )
    }).await;

    match backfill_result {
        Ok(()) => tracing::info!("version_history backfill completed on startup"),
        Err(e) => tracing::warn!(error = %e, "version_history backfill failed on startup — will be populated incrementally"),
    }

    // Counter for periodic debouncer pruning (every ~100 events).
    let mut event_count: u64 = 0;
    let mut last_prune = Instant::now();

    loop {
        tokio::select! {
            // Branch (a): new filesystem event from the notify watcher
            event_result = rx.recv() => {
                match event_result {
                    Some(Ok(event)) => {
                        event_count += 1;

                        for path in &event.paths {
                            // Filter to .jsonl files only
                            let ext = path.extension().and_then(|e| e.to_str());
                            if ext != Some("jsonl") {
                                continue;
                            }

                            // Apply per-file debounce
                            let path_buf = path.to_path_buf();
                            if !state.debouncer.should_sync(&path_buf) {
                                tracing::debug!(
                                    path = %path.display(),
                                    "Debounced: skipping sync (within 2-second window)"
                                );
                                continue;
                            }

                            // Extract session ID from path
                            let session_id = match claude_history_store::sync::extract_session_id(path) {
                                Some(id) => id,
                                None => {
                                    tracing::debug!(
                                        path = %path.display(),
                                        "Could not extract session ID from path, skipping"
                                    );
                                    continue;
                                }
                            };

                            let path_str = path.to_string_lossy().to_string();

                            // Check if this is a new file BEFORE sync_file updates sync_metadata
                            let was_new_file = is_new_file(&conn, &path_str).await;

                            // Snapshot artifact table max IDs before sync so we can detect
                            // newly-created artifact rows after sync completes (SSE-06, SSE-07)
                            let (file_op_snap, git_op_snap) = snapshot_artifact_ids(&conn).await;

                            // Sync the file
                            match claude_history_store::sync::sync_file(&conn, path, &session_id).await {
                                Ok(result) if result.records_synced > 0 => {
                                    tracing::info!(
                                        path = %path_str,
                                        session_id = %session_id,
                                        records = result.records_synced,
                                        overflow = result.overflow_fields_logged,
                                        "Live sync completed"
                                    );

                                    // SSE-02: record:added
                                    let _ = event_tx.send(SseEvent::RecordAdded {
                                        session_id: session_id.clone(),
                                        records_synced: result.records_synced,
                                        file_path: path_str.clone(),
                                    });

                                    // SSE-03: session:started (first time seeing this file)
                                    if was_new_file {
                                        let _ = event_tx.send(SseEvent::SessionStarted {
                                            session_id: session_id.clone(),
                                        });
                                        tracing::info!(
                                            session_id = %session_id,
                                            "New session detected"
                                        );
                                    }

                                    // SSE-04: schema:drift (overflow fields detected)
                                    if result.overflow_fields_logged > 0 {
                                        let _ = event_tx.send(SseEvent::SchemaDrift {
                                            new_fields: result.overflow_fields_logged,
                                            session_id: session_id.clone(),
                                        });
                                    }

                                    // SSE-05: version:changed
                                    check_version_change(&mut state, &conn, &session_id, &event_tx).await;

                                    // SSE-06, SSE-07: file:written, file:edited, git:commit
                                    emit_artifact_events(
                                        &conn, &session_id,
                                        file_op_snap, git_op_snap,
                                        &event_tx,
                                    ).await;

                                    state.data_ingested_since_fts_rebuild = true;
                                }
                                Ok(result) if result.skipped => {
                                    tracing::debug!(
                                        path = %path_str,
                                        "File had no new data, skipped"
                                    );
                                }
                                Ok(_) => {
                                    // records_synced == 0 but not skipped — file had trailing
                                    // whitespace or empty new lines. Not actionable.
                                    tracing::debug!(
                                        path = %path_str,
                                        "Synced with zero records (non-record content appended)"
                                    );
                                }
                                Err(e) => {
                                    // Graceful recovery: log and continue (WATCH-03, success criterion 4)
                                    tracing::warn!(
                                        path = %path_str,
                                        error = %e,
                                        "Live sync failed for file, continuing"
                                    );
                                }
                            }
                        }

                        // Periodic debouncer pruning: every ~100 events or every minute
                        if event_count % 100 == 0
                            || last_prune.elapsed() > Duration::from_secs(60)
                        {
                            state.debouncer.prune_stale(Duration::from_secs(600));
                            last_prune = Instant::now();
                        }
                    }
                    Some(Err(e)) => {
                        // Transient filesystem error: log and continue (WATCH-03)
                        tracing::warn!(
                            error = %e,
                            "File watcher error, continuing"
                        );
                    }
                    None => {
                        // Channel closed: watcher thread died or was cleaned up.
                        tracing::info!("Watcher event channel closed, exiting watcher loop");
                        break;
                    }
                }
            }

            // Branch (b): cancellation signal (graceful shutdown)
            _ = cancel.cancelled() => {
                tracing::info!("Watcher loop received cancellation signal, shutting down");
                break;
            }

            // Branch (c): periodic FTS rebuild check (every 30 seconds)
            // Rebuilds both message content and file operations FTS5 indexes.
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                if state.data_ingested_since_fts_rebuild
                    && state.last_fts_rebuild.elapsed() >= Duration::from_secs(30)
                {
                    match conn
                        .call(|conn| -> Result<(), rusqlite::Error> {
                            claude_history_store::fts::rebuild_fts_index(conn)?;
                            claude_history_store::fts::rebuild_fts_file_operations(conn)?;
                            Ok(())
                        })
                        .await
                    {
                        Ok(()) => {
                            tracing::info!("Periodic FTS rebuild complete (message content + file operations)");
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Periodic FTS rebuild failed, will retry on next interval"
                            );
                        }
                    }
                    state.data_ingested_since_fts_rebuild = false;
                    state.last_fts_rebuild = Instant::now();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    /// D2 regression test: the watcher's startup version_history backfill must not
    /// run until the sync_all completion oneshot fires. We seed the sessions table
    /// AFTER spawning watcher_loop but BEFORE firing the oneshot, then assert that
    /// version_history is observed populated only after we send the signal.
    ///
    /// The test does not exercise the live filesystem-event branch — it only
    /// verifies the ordering of the one-shot startup backfill query against the
    /// completion signal. The cancellation token at the end winds the loop down
    /// cleanly so the test does not leak a spawned task.
    #[tokio::test]
    async fn watcher_startup_backfill_waits_for_sync_all_signal() {
        // Set up an in-process SQLite database with full migrations applied so
        // the version_history table and its constraints are present.
        let tmp_dir = std::env::temp_dir().join("claude-history-watcher-d2-test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let db_path = tmp_dir.join(format!("d2-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db_path);

        let conn = claude_history_store::db::init_db(&db_path)
            .await
            .expect("init_db should succeed for D2 test");

        // Channels mirroring the real serve.rs wiring.
        let (_watch_tx, watch_rx) =
            mpsc::channel::<notify::Result<notify::Event>>(16);
        let (event_tx, _event_rx) = broadcast::channel::<SseEvent>(16);
        let (sync_done_tx, sync_done_rx) = tokio::sync::oneshot::channel::<()>();
        let cancel = CancellationToken::new();

        // Spawn watcher_loop. With the D2 sequencing in place, the loop awaits
        // sync_done_rx before issuing the startup backfill query.
        let loop_conn = conn.clone();
        let loop_cancel = cancel.clone();
        let loop_handle = tokio::spawn(async move {
            watcher_loop(watch_rx, loop_conn, event_tx, loop_cancel, sync_done_rx).await;
        });

        // Insert a sessions row AFTER spawning the watcher_loop but BEFORE firing
        // the completion signal. If the watcher's backfill ran without waiting on
        // the signal, it could observe an empty sessions table here and produce
        // an empty version_history.
        conn.call(|c| {
            c.execute(
                "INSERT INTO sessions (session_id, project_path, first_seen_at, version)
                 VALUES ('d2-test-session', '/tmp/d2', datetime('now'), '2.0.0-d2-test')",
                [],
            )
        })
        .await
        .expect("insert seeded session row");

        // Confirm version_history is empty before we fire the signal. A small
        // sleep gives any erroneously-ungated backfill a chance to run; the
        // assertion below would catch a race in either direction.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let pre_signal_count: i64 = conn
            .call(|c| c.query_row("SELECT COUNT(*) FROM version_history", [], |row| row.get(0)))
            .await
            .expect("count version_history pre-signal");
        assert_eq!(
            pre_signal_count, 0,
            "version_history should be empty before sync_all completion signal fires"
        );

        // Capture timestamp before firing the oneshot.
        let t_signal = SystemTime::now();

        // Fire the signal — watcher_loop's startup backfill is now allowed to run.
        sync_done_tx
            .send(())
            .expect("send on sync_done channel should succeed");

        // Poll version_history until non-empty, with a 1-second timeout.
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut observed_count: i64 = 0;
        let mut t_observed: Option<SystemTime> = None;
        while std::time::Instant::now() < deadline {
            observed_count = conn
                .call(|c| {
                    c.query_row("SELECT COUNT(*) FROM version_history", [], |row| row.get(0))
                })
                .await
                .expect("count version_history during poll");
            if observed_count > 0 {
                t_observed = Some(SystemTime::now());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            observed_count > 0,
            "watcher startup backfill should populate version_history within 1s after signal fires"
        );
        let t_observed = t_observed.expect("observed timestamp recorded when count went positive");
        assert!(
            t_observed >= t_signal,
            "backfill observation timestamp ({:?}) should not precede signal-fire timestamp ({:?})",
            t_observed,
            t_signal
        );

        // Wind the watcher_loop down cleanly via cancellation, then await the
        // task handle so the test does not leak a spawned task.
        cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(2), loop_handle).await;

        // Best-effort cleanup of the on-disk DB file.
        let _ = std::fs::remove_file(&db_path);
    }

    /// D3 regression test: when `check_version_change` upserts into
    /// version_history via the ON CONFLICT path, the resulting `session_count`
    /// must reflect `COUNT(*) FROM sessions WHERE version = ?` rather than the
    /// pre-D3 `version_history.session_count + 1`. We seed two sessions at the
    /// same version, plant a stale version_history row with `session_count = 0`
    /// to make the divergence observable, and call `check_version_change` to
    /// trigger the upsert. Pre-D3 the post-call value would be 1 (stale 0 + 1).
    /// Post-D3 it is 2 (the truth from sessions). Asserting equality with 2
    /// pins the intended re-derive semantic.
    ///
    /// The test does not exercise watcher_loop or any spawned task — it calls
    /// `check_version_change` directly with a freshly built `WatcherState`,
    /// avoiding the gated-backfill code path entirely. A `CancellationToken` is
    /// not required because no task is spawned.
    #[tokio::test]
    async fn check_version_change_session_count_re_derives_from_sessions_truth() {
        // PID-scoped temp DB with full migrations applied.
        let tmp_dir = std::env::temp_dir().join("claude-history-watcher-d3-test");
        let _ = std::fs::create_dir_all(&tmp_dir);
        let db_path = tmp_dir.join(format!("d3-test-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&db_path);

        let conn = claude_history_store::db::init_db(&db_path)
            .await
            .expect("init_db should succeed for D3 test");

        // Seed: two distinct sessions at the same version. This is the "truth"
        // that the D3 SQL aims to surface into version_history.session_count.
        conn.call(|c| {
            c.execute_batch(
                "INSERT INTO sessions (session_id, project_path, first_seen_at, version)
                 VALUES ('d3-session-a', '/tmp/d3', datetime('now'), '2.1.99');
                 INSERT INTO sessions (session_id, project_path, first_seen_at, version)
                 VALUES ('d3-session-b', '/tmp/d3', datetime('now'), '2.1.99');",
            )
            .map_err(tokio_rusqlite::Error::from)
        })
        .await
        .expect("seed two sessions at version 2.1.99");

        // Plant a stale version_history row with session_count = 0. The pre-D3
        // path would compute `0 + 1 = 1` on conflict; the D3 path re-derives
        // from sessions and yields 2.
        conn.call(|c| {
            c.execute(
                "INSERT INTO version_history (version, first_seen_at, last_seen_at, session_id, session_count)
                 VALUES ('2.1.99', datetime('now'), datetime('now'), 'd3-session-a', 0)",
                [],
            )
        })
        .await
        .expect("plant stale version_history row with session_count = 0");

        // Build a WatcherState whose last_known_version differs from the
        // session's version, so check_version_change takes the "changed"
        // branch and reaches the ON CONFLICT upsert.
        let mut state = WatcherState {
            debouncer: FileDebouncer::new(2),
            last_known_version: Some("2.1.0-prior".to_string()),
            data_ingested_since_fts_rebuild: false,
            last_fts_rebuild: Instant::now(),
        };
        let (event_tx, _event_rx) = broadcast::channel::<SseEvent>(16);

        check_version_change(&mut state, &conn, "d3-session-a", &event_tx).await;

        // Assert the upsert re-derived session_count from the sessions table.
        let observed_count: i64 = conn
            .call(|c| {
                c.query_row(
                    "SELECT session_count FROM version_history WHERE version = '2.1.99'",
                    [],
                    |row| row.get(0),
                )
            })
            .await
            .expect("read back session_count for version 2.1.99");

        assert_eq!(
            observed_count, 2,
            "post-D3, session_count must re-derive from COUNT(*) FROM sessions \
             (truth = 2 sessions at version 2.1.99); pre-D3 it would have been \
             1 (stale 0 + 1 increment)"
        );

        // Best-effort cleanup of the on-disk DB file.
        let _ = std::fs::remove_file(&db_path);
    }
}
