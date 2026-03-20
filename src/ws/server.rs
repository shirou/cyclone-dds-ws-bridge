use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

use crate::bridge::{self, Bridge};
use crate::ws::session;

/// Start the WebSocket server. Runs until the provided shutdown signal fires.
pub async fn start(
    bridge: Bridge,
    addr: &str,
    port: u16,
    max_connections: usize,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<(), std::io::Error> {
    let listener = TcpListener::bind(format!("{addr}:{port}")).await?;
    tracing::info!("WebSocket server listening on {addr}:{port}");

    let active_connections = Arc::new(AtomicUsize::new(0));

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, peer_addr) = match accept_result {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        continue;
                    }
                };

                let current = active_connections.load(Ordering::Relaxed);
                if current >= max_connections {
                    tracing::warn!(peer = %peer_addr, "connection rejected: max_connections reached ({max_connections})");
                    drop(stream);
                    continue;
                }

                let bridge_clone = bridge.clone();
                let conns = active_connections.clone();
                conns.fetch_add(1, Ordering::Relaxed);

                // Spawn immediately so WS handshake doesn't block accept loop
                tokio::spawn(async move {
                    let ws = match accept_async(stream).await {
                        Ok(ws) => ws,
                        Err(e) => {
                            tracing::warn!(peer = %peer_addr, error = %e, "WebSocket handshake failed");
                            conns.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }
                    };

                    let session_id = bridge::next_session_id();
                    tracing::info!(session_id, peer = %peer_addr, "new connection");

                    session::run_session(bridge_clone, ws, session_id).await;
                    conns.fetch_sub(1, Ordering::Relaxed);
                });
            }
            _ = shutdown.recv() => {
                tracing::info!("WebSocket server shutting down");
                break;
            }
        }
    }

    Ok(())
}
