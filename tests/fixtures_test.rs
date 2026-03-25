//! Protocol conformance test: generate and verify .bin fixtures.
//!
//! These fixtures are shared with the Go client library (Phase 5)
//! to ensure both implementations produce identical wire formats.
//!
//! To regenerate fixtures after a protocol change:
//!   cargo test -- --ignored generate_fixtures

use bytes::BufMut;
use bytes::BytesMut;
use std::path::PathBuf;

use cyclone_dds_ws_bridge::protocol::*;
use cyclone_dds_ws_bridge::qos::*;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// --- Fixture definitions ---

fn subscribe_fixture() -> Vec<u8> {
    let payload = SubscribePayload {
        topic_name: "SensorData".into(),
        type_name: "sensor::SensorData".into(),
        qos: QosSet {
            policies: vec![
                QosPolicy::Reliability {
                    kind: 1,
                    max_blocking_time_ms: 100,
                },
                QosPolicy::Durability { kind: 1 },
            ],
        },
        is_keyed: true,
        key_descriptors: KeyDescriptors {
            keys: vec![KeyField {
                offset: 4,
                size: 4,
                type_hint: KeyTypeHint::Int32,
            }],
        },
    };
    let bytes = serialize_subscribe(&payload);
    build_message(MsgType::Subscribe, 1, &bytes)
}

fn unsubscribe_fixture() -> Vec<u8> {
    let payload = UnsubscribePayload {
        topic_name: "SensorData".into(),
        type_name: "sensor::SensorData".into(),
    };
    let bytes = serialize_unsubscribe(&payload);
    build_message(MsgType::Unsubscribe, 2, &bytes)
}

fn write_topic_mode_fixture() -> Vec<u8> {
    let payload = WritePayload(WriteMode::TopicName {
        topic_name: "SensorData".into(),
        type_name: "sensor::SensorData".into(),
        qos: QosSet::default(),
        key_descriptors: KeyDescriptors { keys: vec![] },
        data: vec![0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00],
    });
    let bytes = serialize_write(&payload);
    build_message(MsgType::Write, 3, &bytes)
}

fn write_writer_id_mode_fixture() -> Vec<u8> {
    let payload = WritePayload(WriteMode::WriterId {
        writer_id: 42,
        data: vec![0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00],
    });
    let bytes = serialize_write(&payload);
    build_message(MsgType::Write, 4, &bytes)
}

fn dispose_fixture() -> Vec<u8> {
    let payload = DisposePayload(WriteMode::WriterId {
        writer_id: 42,
        data: vec![0x2A, 0x00, 0x00, 0x00],
    });
    let bytes = serialize_dispose(&payload);
    build_message(MsgType::Dispose, 5, &bytes)
}

fn create_writer_fixture() -> Vec<u8> {
    let payload = CreateWriterPayload {
        topic_name: "SensorData".into(),
        type_name: "sensor::SensorData".into(),
        qos: QosSet {
            policies: vec![QosPolicy::Reliability {
                kind: 1,
                max_blocking_time_ms: 100,
            }],
        },
        is_keyed: true,
        key_descriptors: KeyDescriptors {
            keys: vec![KeyField {
                offset: 4,
                size: 4,
                type_hint: KeyTypeHint::Int32,
            }],
        },
    };
    let bytes = serialize_create_writer(&payload);
    build_message(MsgType::CreateWriter, 6, &bytes)
}

fn delete_writer_fixture() -> Vec<u8> {
    let payload = DeleteWriterPayload { writer_id: 42 };
    let bytes = serialize_delete_writer(&payload);
    build_message(MsgType::DeleteWriter, 7, &bytes)
}

fn ping_fixture() -> Vec<u8> {
    build_message(MsgType::Ping, 8, &[])
}

fn ok_empty_fixture() -> Vec<u8> {
    build_message(MsgType::Ok, 6, &[])
}

fn ok_writer_id_fixture() -> Vec<u8> {
    let mut payload = BytesMut::new();
    payload.put_u32_le(42);
    build_message(MsgType::Ok, 6, &payload)
}

fn data_fixture() -> Vec<u8> {
    let payload = DataPayload {
        topic_name: "SensorData".into(),
        source_timestamp: 1700000000_000_000_000,
        data: vec![0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00],
    };
    let bytes = serialize_data(&payload);
    build_message(MsgType::Data, 0, &bytes)
}

fn data_disposed_fixture() -> Vec<u8> {
    let payload = DataDisposedPayload {
        topic_name: "SensorData".into(),
        source_timestamp: 1700000000_000_000_000,
        key_data: vec![0x2A, 0x00, 0x00, 0x00],
    };
    let bytes = serialize_data_disposed(&payload);
    build_message(MsgType::DataDisposed, 0, &bytes)
}

fn data_no_writers_fixture() -> Vec<u8> {
    let payload = DataNoWritersPayload {
        topic_name: "SensorData".into(),
        key_data: vec![0x2A, 0x00, 0x00, 0x00],
    };
    let bytes = serialize_data_no_writers(&payload);
    build_message(MsgType::DataNoWriters, 0, &bytes)
}

fn error_fixture() -> Vec<u8> {
    let payload = ErrorPayload {
        error_code: ErrorCode::InvalidMessage,
        error_message: "unsupported protocol version 3; bridge supports up to version 1".into(),
    };
    let bytes = serialize_error(&payload);
    build_message(MsgType::Error, 1, &bytes)
}

fn pong_fixture() -> Vec<u8> {
    build_message(MsgType::Pong, 8, &[])
}

struct Fixture {
    name: &'static str,
    data: Vec<u8>,
}

fn all_fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "subscribe",
            data: subscribe_fixture(),
        },
        Fixture {
            name: "unsubscribe",
            data: unsubscribe_fixture(),
        },
        Fixture {
            name: "write_topic_mode",
            data: write_topic_mode_fixture(),
        },
        Fixture {
            name: "write_writer_id_mode",
            data: write_writer_id_mode_fixture(),
        },
        Fixture {
            name: "dispose",
            data: dispose_fixture(),
        },
        Fixture {
            name: "create_writer",
            data: create_writer_fixture(),
        },
        Fixture {
            name: "delete_writer",
            data: delete_writer_fixture(),
        },
        Fixture {
            name: "ping",
            data: ping_fixture(),
        },
        Fixture {
            name: "ok_empty",
            data: ok_empty_fixture(),
        },
        Fixture {
            name: "ok_writer_id",
            data: ok_writer_id_fixture(),
        },
        Fixture {
            name: "data",
            data: data_fixture(),
        },
        Fixture {
            name: "data_disposed",
            data: data_disposed_fixture(),
        },
        Fixture {
            name: "data_no_writers",
            data: data_no_writers_fixture(),
        },
        Fixture {
            name: "error",
            data: error_fixture(),
        },
        Fixture {
            name: "pong",
            data: pong_fixture(),
        },
    ]
}

/// Write .bin fixtures to disk. Run manually with:
///   cargo test -- --ignored generate_fixtures
#[test]
#[ignore]
fn generate_fixtures() {
    let dir = fixtures_dir();
    for fixture in all_fixtures() {
        let path = dir.join(format!("{}.bin", fixture.name));
        std::fs::write(&path, &fixture.data)
            .unwrap_or_else(|e| panic!("failed to write {}: {}", path.display(), e));
    }
    eprintln!("Generated {} fixtures in {}", all_fixtures().len(), dir.display());
}

#[test]
fn verify_fixtures_parse_correctly() {
    for fixture in all_fixtures() {
        let data = &fixture.data;
        let header = parse_header(data)
            .unwrap_or_else(|e| panic!("fixture '{}': header parse failed: {}", fixture.name, e));

        assert_eq!(
            header.payload_length as usize,
            data.len() - HEADER_SIZE,
            "fixture '{}': payload_length mismatch",
            fixture.name
        );

        let payload = &data[HEADER_SIZE..];

        match header.msg_type {
            MsgType::Subscribe => {
                parse_subscribe(payload).unwrap();
            }
            MsgType::Unsubscribe => {
                parse_unsubscribe(payload).unwrap();
            }
            MsgType::Write | MsgType::WriteDispose => {
                parse_write(payload).unwrap();
            }
            MsgType::Dispose => {
                parse_dispose(payload).unwrap();
            }
            MsgType::CreateWriter => {
                parse_create_writer(payload).unwrap();
            }
            MsgType::DeleteWriter => {
                parse_delete_writer(payload).unwrap();
            }
            MsgType::Ping | MsgType::Pong => {
                assert_eq!(
                    payload.len(),
                    0,
                    "fixture '{}': expected empty payload",
                    fixture.name
                );
            }
            MsgType::Ok => {}
            MsgType::Data => {
                parse_data(payload).unwrap();
            }
            MsgType::DataDisposed => {
                parse_data_disposed(payload).unwrap();
            }
            MsgType::DataNoWriters => {
                parse_data_no_writers(payload).unwrap();
            }
            MsgType::Error => {
                parse_error(payload).unwrap();
            }
        }
    }
}

#[test]
fn verify_fixtures_byte_exact() {
    let dir = fixtures_dir();
    for fixture in all_fixtures() {
        let path = dir.join(format!("{}.bin", fixture.name));
        let on_disk = std::fs::read(&path).unwrap_or_else(|e| {
            panic!(
                "fixture '{}' not found at {}. Run: cargo test -- --ignored generate_fixtures\nError: {}",
                fixture.name,
                path.display(),
                e
            )
        });
        assert_eq!(
            on_disk, fixture.data,
            "fixture '{}': on-disk bytes differ from generated bytes. \
             Re-run: cargo test -- --ignored generate_fixtures",
            fixture.name
        );
    }
}

// --- Specific fixture field verification ---

#[test]
fn verify_subscribe_fixture_fields() {
    let data = subscribe_fixture();
    let header = parse_header(&data).unwrap();
    assert_eq!(header.msg_type, MsgType::Subscribe);
    assert_eq!(header.request_id, 1);

    let payload = parse_subscribe(&data[HEADER_SIZE..]).unwrap();
    assert_eq!(payload.topic_name, "SensorData");
    assert_eq!(payload.type_name, "sensor::SensorData");
    assert_eq!(payload.qos.policies.len(), 2);
    assert_eq!(payload.key_descriptors.keys.len(), 1);
    assert_eq!(payload.key_descriptors.keys[0].offset, 4);
    assert_eq!(
        payload.key_descriptors.keys[0].type_hint,
        KeyTypeHint::Int32
    );
}

#[test]
fn verify_error_fixture_fields() {
    let data = error_fixture();
    let header = parse_header(&data).unwrap();
    assert_eq!(header.msg_type, MsgType::Error);
    assert_eq!(header.request_id, 1);

    let payload = parse_error(&data[HEADER_SIZE..]).unwrap();
    assert_eq!(payload.error_code, ErrorCode::InvalidMessage);
    assert!(payload.error_message.contains("version"));
}

#[test]
fn verify_data_fixture_fields() {
    let data = data_fixture();
    let header = parse_header(&data).unwrap();
    assert_eq!(header.msg_type, MsgType::Data);
    assert_eq!(header.request_id, 0);

    let payload = parse_data(&data[HEADER_SIZE..]).unwrap();
    assert_eq!(payload.topic_name, "SensorData");
    assert_eq!(payload.source_timestamp, 1700000000_000_000_000);
    assert_eq!(payload.data.len(), 8);
}
