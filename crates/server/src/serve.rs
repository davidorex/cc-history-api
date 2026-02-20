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
/// 2. Creates a `CancellationToken` shared between both listeners
/// 3. Binds a `TcpListener` on `127.0.0.1:{port}`
/// 4. Removes any stale socket file and binds a `UnixListener` on `socket_path`
/// 5. Spawns both listeners as tokio tasks with `with_graceful_shutdown`
/// 6. Spawns a signal handler task that cancels the token on SIGTERM/SIGINT
/// 7. Waits for both listener tasks to complete (either via shutdown or error)
/// 8. Cleans up the socket file after shutdown
///
/// The function runs as a foreground process -- no daemonization logic is
/// included. Background management is intended for launchd/systemd.
pub async fn run_server(
    state: SharedState,
    port: u16,
    socket_path: PathBuf,
) -> anyhow::Result<()> {
    let app = api::build_router(state);
    let token = CancellationToken::new();

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
