use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;

use crate::bridge::{self, Bridge};
use crate::ws::session;

const HEALTH_RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK";

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
                let (mut stream, peer_addr) = match accept_result {
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

                // Peek to detect health check before counting as a connection
                let mut buf = [0u8; 11];
                let is_health = matches!(stream.peek(&mut buf).await, Ok(n) if n >= 11 && &buf[..11] == b"GET /health");
                if is_health {
                    // Drain the request to avoid TCP RST on close
                    let mut discard = [0u8; 512];
                    let _ = stream.read(&mut discard).await;
                    let _ = stream.write_all(HEALTH_RESPONSE).await;
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
