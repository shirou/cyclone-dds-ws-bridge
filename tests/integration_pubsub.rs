//! Integration tests for the bridge: WebSocket client <-> DDS loopback.
//!
//! These tests require Cyclone DDS to be installed and run with:
//!   cargo test --test integration_pubsub -- --test-threads=1
//!
//! They are run automatically inside the Docker build (see Dockerfile).
//! Each test starts the bridge on an ephemeral port and connects via WebSocket.

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::Message;

use cyclone_dds_ws_bridge::bridge;
use cyclone_dds_ws_bridge::config::Config;
use cyclone_dds_ws_bridge::protocol::*;
use cyclone_dds_ws_bridge::qos::*;
use cyclone_dds_ws_bridge::ws;

static PORT_COUNTER: AtomicU16 = AtomicU16::new(19876);

fn next_port() -> u16 {
    PORT_COUNTER.fetch_add(1, Ordering::Relaxed)
}

struct TestBridge {
    addr: String,
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl TestBridge {
    async fn start() -> Self {
        let port = next_port();
        let addr = format!("127.0.0.1:{port}");

        let mut config = Config::default();
        config.websocket.addr = "127.0.0.1".into();
        config.websocket.port = port;

        let bridge_handle = bridge::create_bridge(config).expect("failed to create bridge");
        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

        // Spawn reader poll loop
        let poll_bridge = bridge_handle.clone();
        tokio::spawn(async move {
            bridge::reader_poll_loop(poll_bridge, Duration::from_millis(5)).await;
        });

        // Spawn WS server
        let ws_bridge = bridge_handle.clone();
        let ws_addr = "127.0.0.1".to_string();
        tokio::spawn(async move {
            let _ = ws::server::start(ws_bridge, &ws_addr, port, 64, shutdown_rx).await;
        });

        // Wait for server to be ready
        for _ in 0..100 {
            if tokio::net::TcpStream::connect(&addr).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        TestBridge {
            addr: format!("ws://{addr}"),
            shutdown_tx,
        }
    }

    async fn connect(&self) -> WsClient {
        let (ws, _) = tokio_tungstenite::connect_async(&self.addr)
            .await
            .expect("ws connect failed");
        WsClient {
            ws,
            next_request_id: 1,
        }
    }
}

impl Drop for TestBridge {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(());
    }
}

struct WsClient {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_request_id: u32,
}

impl WsClient {
    fn next_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    async fn send_msg(&mut self, msg_type: MsgType, payload: &[u8]) -> u32 {
        let rid = self.next_id();
        let msg = build_message(msg_type, rid, payload);
        self.ws.send(Message::Binary(msg.into())).await.unwrap();
        rid
    }

    async fn recv_response(&mut self, expected_rid: u32, timeout: Duration) -> (MsgType, Vec<u8>) {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!("timeout waiting for response rid={expected_rid}");
            }
            let msg = tokio::time::timeout(remaining, self.ws.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .expect("ws error");
            if let Message::Binary(data) = msg {
                let header = parse_header(&data).unwrap();
                if header.request_id == expected_rid {
                    return (header.msg_type, data[HEADER_SIZE..].to_vec());
                }
                // Not our response, skip (could be DATA notification)
            }
        }
    }

    async fn recv_data(&mut self, timeout: Duration) -> DataPayload {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!("timeout waiting for DATA message");
            }
            let msg = tokio::time::timeout(remaining, self.ws.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .expect("ws error");
            if let Message::Binary(data) = msg {
                let header = parse_header(&data).unwrap();
                if header.msg_type == MsgType::Data && header.request_id == 0 {
                    return parse_data(&data[HEADER_SIZE..]).unwrap();
                }
            }
        }
    }

    async fn recv_data_disposed(&mut self, timeout: Duration) -> DataDisposedPayload {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!("timeout waiting for DATA_DISPOSED message");
            }
            let msg = tokio::time::timeout(remaining, self.ws.next())
                .await
                .expect("timeout")
                .expect("stream ended")
                .expect("ws error");
            if let Message::Binary(data) = msg {
                let header = parse_header(&data).unwrap();
                if header.msg_type == MsgType::DataDisposed && header.request_id == 0 {
                    return parse_data_disposed(&data[HEADER_SIZE..]).unwrap();
                }
            }
        }
    }

    async fn subscribe(&mut self, topic: &str, type_name: &str) {
        let payload = serialize_subscribe(&SubscribePayload {
            is_keyed: true,
        topic_name: topic.into(),
            type_name: type_name.into(),
            qos: QosSet::default(),
            key_descriptors: KeyDescriptors { keys: vec![] },
        });
        let rid = self.send_msg(MsgType::Subscribe, &payload).await;
        let (msg_type, _) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Ok, "subscribe failed");
    }

    async fn unsubscribe(&mut self, topic: &str, type_name: &str) {
        let payload = serialize_unsubscribe(&UnsubscribePayload {
            topic_name: topic.into(),
            type_name: type_name.into(),
        });
        let rid = self.send_msg(MsgType::Unsubscribe, &payload).await;
        let (msg_type, _) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Ok, "unsubscribe failed");
    }

    async fn write_topic_mode(&mut self, topic: &str, type_name: &str, data: &[u8]) {
        let payload = serialize_write(&WritePayload(WriteMode::TopicName {
            topic_name: topic.into(),
            type_name: type_name.into(),
            qos: QosSet::default(),
            key_descriptors: KeyDescriptors { keys: vec![] },
            data: data.to_vec(),
        }));
        let rid = self.send_msg(MsgType::Write, &payload).await;
        let (msg_type, _) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Ok, "write failed");
    }

    async fn dispose_topic_mode(&mut self, topic: &str, type_name: &str, key_data: &[u8]) {
        let payload = serialize_dispose(&DisposePayload(WriteMode::TopicName {
            topic_name: topic.into(),
            type_name: type_name.into(),
            qos: QosSet::default(),
            key_descriptors: KeyDescriptors { keys: vec![] },
            data: key_data.to_vec(),
        }));
        let rid = self.send_msg(MsgType::Dispose, &payload).await;
        let (msg_type, _) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Ok, "dispose failed");
    }

    async fn create_writer(&mut self, topic: &str, type_name: &str) -> u32 {
        let payload = serialize_create_writer(&CreateWriterPayload {
            is_keyed: true,
        topic_name: topic.into(),
            type_name: type_name.into(),
            qos: QosSet::default(),
            key_descriptors: KeyDescriptors { keys: vec![] },
        });
        let rid = self.send_msg(MsgType::CreateWriter, &payload).await;
        let (msg_type, resp) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Ok, "create_writer failed");
        u32::from_le_bytes(resp[0..4].try_into().unwrap())
    }

    async fn delete_writer(&mut self, writer_id: u32) {
        let payload = serialize_delete_writer(&DeleteWriterPayload { writer_id });
        let rid = self.send_msg(MsgType::DeleteWriter, &payload).await;
        let (msg_type, _) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Ok, "delete_writer failed");
    }

    async fn write_writer_id(&mut self, writer_id: u32, data: &[u8]) {
        let payload = serialize_write(&WritePayload(WriteMode::WriterId {
            writer_id,
            key_bytes: vec![],
            data: data.to_vec(),
        }));
        let rid = self.send_msg(MsgType::Write, &payload).await;
        let (msg_type, _) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Ok, "write (writer_id) failed");
    }

    async fn ping(&mut self) {
        let rid = self.send_msg(MsgType::Ping, &[]).await;
        let (msg_type, _) = self.recv_response(rid, Duration::from_secs(5)).await;
        assert_eq!(msg_type, MsgType::Pong);
    }
}

// --- Tests ---

#[tokio::test]
async fn test_ping_pong() {
    let bridge = TestBridge::start().await;
    let mut client = bridge.connect().await;
    client.ping().await;
}

#[tokio::test]
async fn test_write_subscribe_data_round_trip() {
    let bridge = TestBridge::start().await;
    let mut client = bridge.connect().await;
    let topic = "IntegPubSub";
    let type_name = "test::Opaque";

    client.subscribe(topic, type_name).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let test_data = vec![0x00, 0x01, 0x00, 0x00, 0xAA, 0xBB, 0xCC, 0xDD];
    client.write_topic_mode(topic, type_name, &test_data).await;

    let data = client.recv_data(Duration::from_secs(5)).await;
    assert_eq!(data.topic_name, topic);
    assert_eq!(data.data, test_data);
}

#[tokio::test]
async fn test_dispose_propagation() {
    let bridge = TestBridge::start().await;
    let mut client = bridge.connect().await;
    let topic = "IntegDispose";
    let type_name = "test::Opaque";

    client.subscribe(topic, type_name).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Write first
    let test_data = vec![0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04];
    client.write_topic_mode(topic, type_name, &test_data).await;
    let _ = client.recv_data(Duration::from_secs(5)).await;

    // Dispose
    let key_data = vec![0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04];
    client.dispose_topic_mode(topic, type_name, &key_data).await;
    let disposed = client.recv_data_disposed(Duration::from_secs(5)).await;
    assert_eq!(disposed.topic_name, topic);
}

#[tokio::test]
async fn test_multiple_clients_fan_out() {
    let bridge = TestBridge::start().await;
    let topic = "IntegFanOut";
    let type_name = "test::Opaque";

    let mut sub1 = bridge.connect().await;
    let mut sub2 = bridge.connect().await;
    let mut writer = bridge.connect().await;

    sub1.subscribe(topic, type_name).await;
    sub2.subscribe(topic, type_name).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let test_data = vec![0x00, 0x01, 0x00, 0x00, 0xFA, 0xCE];
    writer.write_topic_mode(topic, type_name, &test_data).await;

    let d1 = sub1.recv_data(Duration::from_secs(5)).await;
    let d2 = sub2.recv_data(Duration::from_secs(5)).await;
    assert_eq!(d1.data, test_data);
    assert_eq!(d2.data, test_data);
}

#[tokio::test]
async fn test_writer_id_mode() {
    let bridge = TestBridge::start().await;
    let mut client = bridge.connect().await;
    let topic = "IntegWriterId";
    let type_name = "test::Opaque";

    client.subscribe(topic, type_name).await;
    let writer_id = client.create_writer(topic, type_name).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let test_data = vec![0x00, 0x01, 0x00, 0x00, 0x11, 0x22];
    client.write_writer_id(writer_id, &test_data).await;

    let data = client.recv_data(Duration::from_secs(5)).await;
    assert_eq!(data.data, test_data);

    client.delete_writer(writer_id).await;
}

#[tokio::test]
async fn test_writer_ownership_violation() {
    let bridge = TestBridge::start().await;
    let mut client_a = bridge.connect().await;
    let mut client_b = bridge.connect().await;
    let topic = "IntegOwnership";
    let type_name = "test::Opaque";

    let writer_id = client_a.create_writer(topic, type_name).await;

    // Client B tries to use client A's writer
    let payload = serialize_write(&WritePayload(WriteMode::WriterId {
        writer_id,
        key_bytes: vec![],
        data: vec![0x00, 0x01, 0x00, 0x00, 0xFF],
    }));
    let rid = client_b.send_msg(MsgType::Write, &payload).await;
    let (msg_type, resp) = client_b.recv_response(rid, Duration::from_secs(5)).await;
    assert_eq!(msg_type, MsgType::Error, "should reject cross-session writer use");
    let err = parse_error(&resp).unwrap();
    assert_eq!(err.error_code, ErrorCode::WriterNotFound);

    // Client B tries to delete client A's writer
    let dw_payload = serialize_delete_writer(&DeleteWriterPayload { writer_id });
    let rid = client_b.send_msg(MsgType::DeleteWriter, &dw_payload).await;
    let (msg_type, _) = client_b.recv_response(rid, Duration::from_secs(5)).await;
    assert_eq!(msg_type, MsgType::Error, "should reject cross-session delete");
}

#[tokio::test]
async fn test_unsubscribe_stops_delivery() {
    let bridge = TestBridge::start().await;
    let topic = "IntegUnsub";
    let type_name = "test::Opaque";

    let mut sub1 = bridge.connect().await;
    let mut sub2 = bridge.connect().await;
    let mut writer = bridge.connect().await;

    sub1.subscribe(topic, type_name).await;
    sub2.subscribe(topic, type_name).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Both receive first message
    let data1 = vec![0x00, 0x01, 0x00, 0x00, 0x01];
    writer.write_topic_mode(topic, type_name, &data1).await;
    let _ = sub1.recv_data(Duration::from_secs(5)).await;
    let _ = sub2.recv_data(Duration::from_secs(5)).await;

    // sub1 unsubscribes
    sub1.unsubscribe(topic, type_name).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Only sub2 should receive
    let data2 = vec![0x00, 0x01, 0x00, 0x00, 0x02];
    writer.write_topic_mode(topic, type_name, &data2).await;
    let received = sub2.recv_data(Duration::from_secs(5)).await;
    assert_eq!(received.data, data2);

    // sub1 should not receive anything (check with a short timeout)
    let result = tokio::time::timeout(Duration::from_millis(500), sub1.ws.next()).await;
    assert!(result.is_err(), "sub1 should not receive data after unsubscribe");
}

#[tokio::test]
async fn test_invalid_message_returns_error() {
    let bridge = TestBridge::start().await;
    let mut client = bridge.connect().await;

    // Send a message with bad magic
    let mut bad_msg = build_message(MsgType::Ping, 1, &[]);
    bad_msg[0] = 0xFF;
    client.ws.send(Message::Binary(bad_msg.into())).await.unwrap();

    // Should receive an error response
    let msg = tokio::time::timeout(Duration::from_secs(5), client.ws.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("ws error");

    if let Message::Binary(data) = msg {
        let header = parse_header(&data).unwrap();
        assert_eq!(header.msg_type, MsgType::Error);
    } else {
        panic!("expected binary message");
    }
}

#[tokio::test]
async fn test_request_id_zero_rejected() {
    let bridge = TestBridge::start().await;
    let mut client = bridge.connect().await;

    // Send a message with request_id=0 (reserved)
    let msg = build_message(MsgType::Ping, 0, &[]);
    client.ws.send(Message::Binary(msg.into())).await.unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(5), client.ws.next())
        .await
        .expect("timeout")
        .expect("stream ended")
        .expect("ws error");

    if let Message::Binary(data) = resp {
        let header = parse_header(&data).unwrap();
        assert_eq!(header.msg_type, MsgType::Error);
    } else {
        panic!("expected binary message");
    }
}
