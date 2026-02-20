//! Filesystem watcher for live JSONL file ingestion.
//!
//! This module provides the real-time ingestion pipeline: notify-based filesystem
//! watching with per-file debounce, an async processing loop that triggers
//! `sync_file` for each changed .jsonl file, and emits all seven SSE event types
//! (record:added, session:started, schema:drift, version:changed, file:written,
//! file:edited, git:commit) through the broadcast channel.
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
            // blocking_send is used because this callback runs on a non-async
            // thread (the OS filesystem event thread). The `let _ =` prefix
            // intentionally discards errors — if the channel is closed or full,
            // the event is lost (acceptable: the watcher loop has shut down).
            let _ = event_tx.blocking_send(result);
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
                state.last_known_version = Some(new_version);
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
