//! Dual-listener serve infrastructure for TCP and Unix domain socket.
//!
//! Provides `run_server` which binds the complete API Router to both a TCP
//! listener (for standard HTTP clients) and a Unix domain socket listener
//! (for lower-latency local consumers). Both listeners share the same
//! `CancellationToken` for coordinated graceful shutdown on SIGTERM/SIGINT.
//!
//! Requirement IDs: INFRA-04, INFRA-05, INFRA-06, UDS-01, UDS-02

use std::path::PathBuf;

use tokio::net::{TcpListener, UnixListener};
use tokio_util::sync::CancellationToken;

use crate::api;
use crate::state::SharedState;

/// Listen for shutdown signals (SIGINT via Ctrl+C, SIGTERM on Unix).
///
/// Uses `tokio::select!` to race between Ctrl+C and SIGTERM. On non-Unix
/// platforms, SIGTERM is replaced with a future that never resolves, so
/// only Ctrl+C triggers shutdown.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received SIGINT (Ctrl+C), initiating graceful shutdown");
        },
        _ = terminate => {
            tracing::info!("Received SIGTERM, initiating graceful shutdown");
        },
    }
}

/// Start the dual-listener HTTP server and block until shutdown completes.
///
/// This function:
/// 1. Builds the API router from `api::build_router(state)`
/// 2. Creates a `CancellationToken` shared between both listeners and watcher
/// 3. Spawns the filesystem watcher on a blocking thread and the watcher_loop
///    as a tokio task for live JSONL ingestion with SSE event emission
/// 4. Binds a `TcpListener` on `127.0.0.1:{port}`
/// 5. Removes any stale socket file and binds a `UnixListener` on `socket_path`
/// 6. Spawns both listeners as tokio tasks with `with_graceful_shutdown`
/// 7. Spawns a signal handler task that cancels the token on SIGTERM/SIGINT
/// 8. Waits for both listener tasks to complete (either via shutdown or error)
/// 9. Cleans up the socket file after shutdown
///
/// The watcher thread is not explicitly shut down -- when the daemon process
/// exits, all threads are cleaned up. The CancellationToken causes the
/// watcher_loop to exit its select! loop, which drops the mpsc Receiver.
///
/// The function runs as a foreground process -- no daemonization logic is
/// included. Background management is intended for launchd/systemd.
pub async fn run_server(
    state: SharedState,
    port: u16,
    socket_path: PathBuf,
    projects_dir: PathBuf,
) -> anyhow::Result<()> {
    let mcp_service = crate::mcp::build_streamable_http_service(state.clone());
    let app = api::build_router(state.clone())
        .nest_service("/mcp", mcp_service);
    let token = CancellationToken::new();

    // --- File watcher for live JSONL ingestion ---
    // Create an mpsc channel for the notify watcher to send filesystem events
    // to the async watcher_loop. Buffer of 4096 provides headroom during burst
    // ingestion — a full channel causes try_send to drop events (logged as
    // warnings) rather than stalling the FSEvents callback thread.
    let (watch_tx, watch_rx) = tokio::sync::mpsc::channel(4096);

    // --- Cold-boot ordering signal between sync_all and watcher startup backfill ---
    // The watcher_loop's startup version_history backfill (in watcher.rs) and the
    // sync_all task spawned below both touch the version_history table. If the
    // watcher's startup backfill ran before sync_all finished writing newer-version
    // sessions, those versions would be missed by that boot's backfill.
    //
    // D1 already moved a sibling backfill to the end of sync_all, so version_history
    // is re-populated on every successful sync_all. D2 closes the cold-boot race by
    // gating ONLY the watcher's one-shot startup backfill query behind a oneshot
    // signal fired when sync_all completes (success or failure). The watcher's live
    // filesystem-event branch and cancellation branch remain ungated — the API
    // listener and live ingestion stay immediately active during cold boot.
    let (sync_done_tx, sync_done_rx) = tokio::sync::oneshot::channel::<()>();

    // Spawn the notify watcher on a blocking thread. If the projects_dir does
    // not exist yet (Claude Code may not have created sessions), this will
    // return an error that we log as a warning and continue — the watcher is
    // optional for basic daemon operation.
    //
    // The receiver `sync_done_rx` is moved into the watcher_loop spawn closure on
    // success. If spawn_watcher fails, the receiver is dropped here; that does not
    // matter because no watcher_loop is running to await it. The sender side is
    // still fired below to avoid leaking resources.
    let mut sync_done_rx_opt = Some(sync_done_rx);
    match crate::watcher::spawn_watcher(projects_dir.clone(), watch_tx) {
        Ok(()) => {
            // Spawn the watcher processing loop as a tokio task.
            // It shares the database connection (Arc-based Clone) and the
            // broadcast sender for SSE event emission.
            let watcher_conn = state.conn.clone();
            let watcher_event_tx = state.event_tx.clone();
            let watcher_token = token.clone();
            let watcher_sync_done = sync_done_rx_opt
                .take()
                .expect("sync_done_rx is taken exactly once when spawn_watcher succeeds");
            tokio::spawn(async move {
                crate::watcher::watcher_loop(
                    watch_rx,
                    watcher_conn,
                    watcher_event_tx,
                    watcher_token,
                    watcher_sync_done,
                )
                .await;
            });
            tracing::info!(
                path = %projects_dir.display(),
                "File watcher started for projects directory"
            );
        }
        Err(e) => {
            tracing::warn!(
                path = %projects_dir.display(),
                error = %e,
                "Failed to start file watcher — live ingestion will be unavailable. \
                 The daemon will still serve API requests from existing data."
            );
        }
    }

    // --- Initial sync to catch up on files the watcher may have missed ---
    // The watcher only sees events from the point it starts. Any JSONL files
    // created or modified before the watcher was established (or missed due to
    // FSEvents coalescing) need to be caught up via a full directory walk.
    // sync_all is incremental — files already fully ingested are skipped via
    // sync_metadata offset tracking, so this is safe to run unconditionally.
    //
    // After sync_all returns (Ok or Err), fire `sync_done_tx` so the watcher's
    // startup backfill (if any) can proceed. The signal is "sync_all has
    // finished, regardless of outcome" — sending on Err preserves liveness so the
    // watcher's own backfill still has a chance to run on the latest sessions
    // table state. If the receiver was already dropped (spawn_watcher failed),
    // `let _ = ...send(())` swallows the error.
    {
        let sync_conn = state.conn.clone();
        let sync_dir = projects_dir.clone();
        tokio::spawn(async move {
            match claude_history_store::sync::sync_all(&sync_conn, &sync_dir).await {
                Ok(result) => {
                    tracing::info!(
                        files_discovered = result.files_discovered,
                        files_synced = result.files_synced,
                        files_skipped = result.files_skipped,
                        files_errored = result.files_errored,
                        total_records = result.total_records,
                        "Startup sync completed"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Startup sync failed — existing data still available, \
                         live watcher will handle new changes"
                    );
                }
            }
            // Always fire the completion signal so the watcher's startup
            // backfill is unblocked, regardless of sync_all's outcome.
            let _ = sync_done_tx.send(());
        });
    }

    // --- TCP listener ---
    let tcp_listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    tracing::info!("TCP listener bound to 127.0.0.1:{}", port);

    // --- Unix domain socket listener ---
    // Remove stale socket file from a previous run or crash (UDS-01 cleanup).
    if socket_path.exists() {
        tracing::debug!(
            "Removing stale socket file at {}",
            socket_path.display()
        );
        let _ = std::fs::remove_file(&socket_path);
    }
    let uds_listener = UnixListener::bind(&socket_path)?;
    tracing::info!(
        "Unix domain socket listener bound to {}",
        socket_path.display()
    );

    // --- Spawn TCP serve task ---
    let tcp_app = app.clone();
    let tcp_token = token.clone();
    let tcp_handle = tokio::spawn(async move {
        axum::serve(tcp_listener, tcp_app)
            .with_graceful_shutdown(async move { tcp_token.cancelled().await })
            .await
    });

    // --- Spawn UDS serve task ---
    let uds_app = app;
    let uds_token = token.clone();
    let uds_handle = tokio::spawn(async move {
        axum::serve(uds_listener, uds_app)
            .with_graceful_shutdown(async move { uds_token.cancelled().await })
            .await
    });

    // --- Spawn signal handler that cancels the token ---
    let signal_token = token.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        signal_token.cancel();
    });

    tracing::info!(
        "Server running — TCP on 127.0.0.1:{}, UDS on {}",
        port,
        socket_path.display()
    );

    // --- Wait for both listeners to complete ---
    // tokio::select! resolves when either listener finishes (normally via
    // shutdown or due to an unexpected error). After the first completes,
    // the cancellation token propagates shutdown to the other.
    tokio::select! {
        tcp_result = tcp_handle => {
            match tcp_result {
                Ok(Ok(())) => tracing::info!("TCP listener shut down"),
                Ok(Err(e)) => tracing::error!("TCP listener error: {}", e),
                Err(e) => tracing::error!("TCP listener task panicked: {}", e),
            }
        },
        uds_result = uds_handle => {
            match uds_result {
                Ok(Ok(())) => tracing::info!("UDS listener shut down"),
                Ok(Err(e)) => tracing::error!("UDS listener error: {}", e),
                Err(e) => tracing::error!("UDS listener task panicked: {}", e),
            }
        },
    }

    // Ensure token is cancelled so the remaining listener also shuts down.
    token.cancel();

    // Brief delay to allow the second listener to drain in-flight requests.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // --- Clean up socket file ---
    if socket_path.exists() {
        match std::fs::remove_file(&socket_path) {
            Ok(()) => tracing::debug!(
                "Removed socket file at {}",
                socket_path.display()
            ),
            Err(e) => tracing::warn!(
                "Failed to remove socket file at {}: {}",
                socket_path.display(),
                e
            ),
        }
    }

    tracing::info!("Server shutdown complete");
    Ok(())
}
