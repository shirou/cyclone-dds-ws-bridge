use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

use crate::bridge::{self, Bridge, OutboundMessage};
use crate::protocol::{self, HEADER_SIZE, VERSION};

/// Run a single client session to completion.
pub async fn run_session(bridge: Bridge, ws: WebSocketStream<TcpStream>, session_id: u64) {
    let (buffer_size, max_payload_size) = {
        let inner = bridge.lock().await;
        (
            inner.config.bridge.session_buffer_size,
            inner.config.bridge.max_payload_size,
        )
    };

    let (bridge_tx, mut bridge_rx) = mpsc::channel::<OutboundMessage>(buffer_size);
    bridge::register_session(&bridge, session_id, bridge_tx).await;

    let (mut ws_sink, mut ws_stream) = ws.split();

    // Channel for responses from inbound message handling back to the write side
    let (resp_tx, mut resp_rx) = mpsc::channel::<Vec<u8>>(buffer_size);

    // Writer task: merges bridge notifications and request-response messages onto ws_sink
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(outbound) = bridge_rx.recv() => {
                    if ws_sink.send(Message::Binary(outbound.data.into())).await.is_err() {
                        break;
                    }
                }
                Some(resp) = resp_rx.recv() => {
                    if ws_sink.send(Message::Binary(resp.into())).await.is_err() {
                        break;
                    }
                }
                else => break,
            }
        }
    });

    // Reader loop: receive WS frames and dispatch to bridge
    while let Some(msg_result) = ws_stream.next().await {
        let msg = match msg_result {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(session_id, error = %e, "websocket read error");
                break;
            }
        };

        match msg {
            Message::Binary(data) => {
                let messages =
                    process_binary_frame(&bridge, session_id, &data, max_payload_size).await;
                for msg in messages {
                    if resp_tx.send(msg).await.is_err() {
                        break;
                    }
                }
            }
            Message::Ping(_) => {
                // tokio-tungstenite auto-replies to WebSocket pings
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    // Drop the response sender to signal the writer task to stop
    drop(resp_tx);
    let _ = writer_task.await;

    // Clean up DDS resources
    bridge::unregister_session(&bridge, session_id).await;
    tracing::info!(session_id, "session disconnected");
}

async fn process_binary_frame(
    bridge: &Bridge,
    session_id: u64,
    data: &[u8],
    max_payload_size: usize,
) -> Vec<Vec<u8>> {
    if data.len() < HEADER_SIZE {
        return vec![make_parse_error(0, "message too short")];
    }

    let header = match protocol::parse_header(data) {
        Ok(h) => h,
        Err(e) => {
            return vec![make_parse_error(0, &e.to_string())];
        }
    };

    if let Err(e) = protocol::validate_client_header(&header, VERSION, max_payload_size) {
        return vec![make_parse_error(header.request_id, &e.to_string())];
    }

    let expected_len = HEADER_SIZE + header.payload_length as usize;
    if data.len() < expected_len {
        return vec![make_parse_error(
            header.request_id,
            &format!(
                "incomplete message: expected {} bytes, got {}",
                expected_len,
                data.len()
            ),
        )];
    }

    let payload = &data[HEADER_SIZE..expected_len];
    bridge::handle_message(bridge, session_id, header.msg_type, header.request_id, payload)
        .await
}

fn make_parse_error(request_id: u32, message: &str) -> Vec<u8> {
    let payload = protocol::serialize_error(&protocol::ErrorPayload {
        error_code: protocol::ErrorCode::InvalidMessage,
        error_message: message.to_string(),
    });
    protocol::build_message(protocol::MsgType::Error, request_id, &payload)
}
