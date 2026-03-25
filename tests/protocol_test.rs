//! Protocol integration tests.
//!
//! Tests full message construction and parsing (header + payload together),
//! covering cross-message-type scenarios and edge cases that go beyond
//! individual unit tests.

use cyclone_dds_ws_bridge::protocol::*;
use cyclone_dds_ws_bridge::qos::*;

/// Build a message and parse it back, verifying header and payload.
fn round_trip_message(msg_type: MsgType, request_id: u32, payload: &[u8]) -> (Header, Vec<u8>) {
    let msg = build_message(msg_type, request_id, payload);
    let header = parse_header(&msg).expect("header parse failed");
    assert_eq!(header.msg_type, msg_type);
    assert_eq!(header.request_id, request_id);
    assert_eq!(header.payload_length as usize, payload.len());
    assert_eq!(msg.len(), HEADER_SIZE + payload.len());
    let payload_bytes = msg[HEADER_SIZE..].to_vec();
    (header, payload_bytes)
}

#[test]
fn test_subscribe_full_message_round_trip() {
    let sub = SubscribePayload {
        is_keyed: true,
        topic_name: "SensorData".into(),
        type_name: "sensor::SensorData".into(),
        qos: QosSet {
            policies: vec![
                QosPolicy::Reliability {
                    kind: 1,
                    max_blocking_time_ms: 100,
                },
                QosPolicy::Durability { kind: 1 },
                QosPolicy::History {
                    kind: 1,
                    depth: 10,
                },
            ],
        },
        key_descriptors: KeyDescriptors {
            keys: vec![
                KeyField {
                    offset: 4,
                    size: 4,
                    type_hint: KeyTypeHint::Int32,
                },
                KeyField {
                    offset: 12,
                    size: 0,
                    type_hint: KeyTypeHint::String,
                },
            ],
        },
    };
    let payload_bytes = serialize_subscribe(&sub);
    let (_, payload) = round_trip_message(MsgType::Subscribe, 1, &payload_bytes);
    let parsed = parse_subscribe(&payload).unwrap();
    assert_eq!(parsed, sub);
}

#[test]
fn test_write_then_data_correlation() {
    // Simulate: client sends WRITE, bridge sends DATA to subscribers
    let write = WritePayload(WriteMode::TopicName {
        topic_name: "Events".into(),
        type_name: "app::Event".into(),
        qos: QosSet::default(),
        key_descriptors: KeyDescriptors { keys: vec![] },
        data: vec![0x00, 0x01, 0x00, 0x00, 0xCA, 0xFE, 0xBA, 0xBE],
    });
    let write_bytes = serialize_write(&write);
    let write_msg = build_message(MsgType::Write, 42, &write_bytes);

    // Parse the write message
    let write_header = parse_header(&write_msg).unwrap();
    assert_eq!(write_header.request_id, 42);
    let parsed_write = parse_write(&write_msg[HEADER_SIZE..]).unwrap();

    // Extract the data from the write
    let original_data = match &parsed_write.0 {
        WriteMode::TopicName { data, .. } => data.clone(),
        _ => panic!("expected TopicName mode"),
    };

    // Bridge would create a DATA message for subscribers
    let data_msg = DataPayload {
        topic_name: "Events".into(),
        source_timestamp: 1700000000_000_000_000,
        data: original_data.clone(),
    };
    let data_bytes = serialize_data(&data_msg);
    let (data_header, data_payload) = round_trip_message(MsgType::Data, 0, &data_bytes);

    assert_eq!(data_header.request_id, 0); // unsolicited
    let parsed_data = parse_data(&data_payload).unwrap();
    assert_eq!(parsed_data.topic_name, "Events");
    assert_eq!(parsed_data.data, original_data);
}

#[test]
fn test_create_writer_ok_response_with_writer_id() {
    // Client sends CREATE_WRITER
    let cw = CreateWriterPayload {
        is_keyed: true,
        topic_name: "Motor".into(),
        type_name: "ctrl::Motor".into(),
        qos: QosSet {
            policies: vec![QosPolicy::Reliability {
                kind: 1,
                max_blocking_time_ms: 500,
            }],
        },
        key_descriptors: KeyDescriptors { keys: vec![] },
    };
    let cw_bytes = serialize_create_writer(&cw);
    let (_, cw_payload) = round_trip_message(MsgType::CreateWriter, 10, &cw_bytes);
    let parsed_cw = parse_create_writer(&cw_payload).unwrap();
    assert_eq!(parsed_cw, cw);

    // Bridge responds with OK containing writer_id
    let writer_id: u32 = 7;
    let ok_payload = writer_id.to_le_bytes().to_vec();
    let ok_msg = build_message(MsgType::Ok, 10, &ok_payload);
    let ok_header = parse_header(&ok_msg).unwrap();
    assert_eq!(ok_header.msg_type, MsgType::Ok);
    assert_eq!(ok_header.request_id, 10);
    let resp_writer_id = u32::from_le_bytes(ok_msg[HEADER_SIZE..HEADER_SIZE + 4].try_into().unwrap());
    assert_eq!(resp_writer_id, 7);
}

#[test]
fn test_error_response_all_codes() {
    let codes = [
        ErrorCode::InvalidMessage,
        ErrorCode::TopicNotFound,
        ErrorCode::DdsError,
        ErrorCode::WriterNotFound,
        ErrorCode::ReaderNotFound,
        ErrorCode::InvalidQos,
        ErrorCode::AlreadyExists,
        ErrorCode::InternalError,
        ErrorCode::BufferOverflow,
    ];
    for code in codes {
        let err = ErrorPayload {
            error_code: code,
            error_message: format!("test error for {code:?}"),
        };
        let bytes = serialize_error(&err);
        let (_, payload) = round_trip_message(MsgType::Error, 99, &bytes);
        let parsed = parse_error(&payload).unwrap();
        assert_eq!(parsed.error_code, code);
        assert!(parsed.error_message.contains(&format!("{code:?}")));
    }
}

#[test]
fn test_dispose_writer_id_mode_full_message() {
    let dispose = DisposePayload(WriteMode::WriterId {
        writer_id: 55,
        data: vec![0x00, 0x01, 0x00, 0x00, 0x42, 0x00, 0x00, 0x00],
    });
    let bytes = serialize_dispose(&dispose);
    let (_, payload) = round_trip_message(MsgType::Dispose, 20, &bytes);
    let parsed = parse_dispose(&payload).unwrap();
    assert_eq!(parsed, dispose);
}

#[test]
fn test_write_dispose_topic_mode_full_message() {
    let wd = WriteDisposePayload(WriteMode::TopicName {
        topic_name: "Keyed".into(),
        type_name: "test::Keyed".into(),
        qos: QosSet::default(),
        key_descriptors: KeyDescriptors {
            keys: vec![KeyField {
                offset: 4,
                size: 4,
                type_hint: KeyTypeHint::Int32,
            }],
        },
        data: vec![0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xFF],
    });
    let bytes = serialize_write_mode(&wd.0);
    let msg = build_message(MsgType::WriteDispose, 30, &bytes);
    let header = parse_header(&msg).unwrap();
    assert_eq!(header.msg_type, MsgType::WriteDispose);
    let parsed = parse_write_dispose(&msg[HEADER_SIZE..]).unwrap();
    assert_eq!(parsed.0, wd.0);
}

#[test]
fn test_validate_client_header_valid() {
    let header = Header {
        version: VERSION,
        msg_type: MsgType::Subscribe,
        request_id: 1,
        payload_length: 100,
    };
    assert!(validate_client_header(&header, VERSION, DEFAULT_MAX_PAYLOAD_SIZE).is_ok());
}

#[test]
fn test_validate_client_header_all_rejections() {
    // Version too high
    let h1 = Header {
        version: VERSION + 1,
        msg_type: MsgType::Subscribe,
        request_id: 1,
        payload_length: 0,
    };
    assert!(matches!(
        validate_client_header(&h1, VERSION, DEFAULT_MAX_PAYLOAD_SIZE),
        Err(ProtocolError::UnsupportedVersion { .. })
    ));

    // Payload too large
    let h2 = Header {
        version: VERSION,
        msg_type: MsgType::Write,
        request_id: 1,
        payload_length: (DEFAULT_MAX_PAYLOAD_SIZE + 1) as u32,
    };
    assert!(matches!(
        validate_client_header(&h2, VERSION, DEFAULT_MAX_PAYLOAD_SIZE),
        Err(ProtocolError::PayloadTooLarge { .. })
    ));

    // Reserved request_id
    let h3 = Header {
        version: VERSION,
        msg_type: MsgType::Ping,
        request_id: 0,
        payload_length: 0,
    };
    assert!(matches!(
        validate_client_header(&h3, VERSION, DEFAULT_MAX_PAYLOAD_SIZE),
        Err(ProtocolError::ReservedRequestId)
    ));
}

#[test]
fn test_ping_pong_minimal_messages() {
    let ping_msg = build_message(MsgType::Ping, 1, &[]);
    assert_eq!(ping_msg.len(), HEADER_SIZE);
    let header = parse_header(&ping_msg).unwrap();
    assert_eq!(header.msg_type, MsgType::Ping);
    assert_eq!(header.payload_length, 0);

    let pong_msg = build_message(MsgType::Pong, 1, &[]);
    assert_eq!(pong_msg.len(), HEADER_SIZE);
    let header = parse_header(&pong_msg).unwrap();
    assert_eq!(header.msg_type, MsgType::Pong);
}

#[test]
fn test_data_disposed_and_no_writers_full_messages() {
    let disposed = DataDisposedPayload {
        topic_name: "KeyedTopic".into(),
        source_timestamp: 999_000_000,
        key_data: vec![0x01, 0x00, 0x00, 0x00],
    };
    let bytes = serialize_data_disposed(&disposed);
    let (_, payload) = round_trip_message(MsgType::DataDisposed, 0, &bytes);
    let parsed = parse_data_disposed(&payload).unwrap();
    assert_eq!(parsed, disposed);

    let no_writers = DataNoWritersPayload {
        topic_name: "KeyedTopic".into(),
        key_data: vec![0x01, 0x00, 0x00, 0x00],
    };
    let bytes = serialize_data_no_writers(&no_writers);
    let (_, payload) = round_trip_message(MsgType::DataNoWriters, 0, &bytes);
    let parsed = parse_data_no_writers(&payload).unwrap();
    assert_eq!(parsed, no_writers);
}

#[test]
fn test_large_payload_within_limits() {
    let large_data = vec![0xAB; 1024 * 1024]; // 1 MiB
    let data_payload = DataPayload {
        topic_name: "BigData".into(),
        source_timestamp: 42,
        data: large_data.clone(),
    };
    let bytes = serialize_data(&data_payload);
    let msg = build_message(MsgType::Data, 0, &bytes);
    let header = parse_header(&msg).unwrap();
    let parsed = parse_data(&msg[HEADER_SIZE..]).unwrap();
    assert_eq!(parsed.data.len(), 1024 * 1024);
    assert_eq!(header.payload_length as usize, bytes.len());
}

#[test]
fn test_qos_all_policies_in_subscribe() {
    let sub = SubscribePayload {
        is_keyed: true,
        topic_name: "AllQoS".into(),
        type_name: "test::AllQoS".into(),
        qos: QosSet {
            policies: vec![
                QosPolicy::Reliability {
                    kind: 1,
                    max_blocking_time_ms: 100,
                },
                QosPolicy::Durability { kind: 1 },
                QosPolicy::History {
                    kind: 1,
                    depth: 100,
                },
                QosPolicy::Deadline { period_ms: 1000 },
                QosPolicy::LatencyBudget { duration_ms: 50 },
                QosPolicy::OwnershipStrength { value: 10 },
                QosPolicy::Liveliness {
                    kind: 1,
                    lease_duration_ms: 5000,
                },
                QosPolicy::DestinationOrder { kind: 0 },
                QosPolicy::Presentation {
                    access_scope: 1,
                    coherent_access: 1,
                    ordered_access: 0,
                },
                QosPolicy::Partition {
                    partitions: vec!["p1".into(), "p2".into()],
                },
                QosPolicy::Ownership { kind: 1 },
                QosPolicy::WriterDataLifecycle {
                    autodispose_unregistered_instances: 0,
                },
                QosPolicy::TimeBasedFilter {
                    minimum_separation_ms: 100,
                },
            ],
        },
        key_descriptors: KeyDescriptors { keys: vec![] },
    };
    let bytes = serialize_subscribe(&sub);
    let msg = build_message(MsgType::Subscribe, 1, &bytes);
    let parsed = parse_subscribe(&msg[HEADER_SIZE..]).unwrap();
    assert_eq!(parsed.qos.policies.len(), 13);
    assert_eq!(parsed, sub);
}

#[test]
fn test_unsubscribe_delete_writer_sequence() {
    // Verify that unsubscribe and delete_writer messages can be
    // constructed and parsed independently (cleanup sequence)
    let unsub = UnsubscribePayload {
        topic_name: "Topic1".into(),
        type_name: "Type1".into(),
    };
    let unsub_bytes = serialize_unsubscribe(&unsub);
    let unsub_msg = build_message(MsgType::Unsubscribe, 100, &unsub_bytes);

    let dw = DeleteWriterPayload { writer_id: 5 };
    let dw_bytes = serialize_delete_writer(&dw);
    let dw_msg = build_message(MsgType::DeleteWriter, 101, &dw_bytes);

    // Both should parse independently
    let h1 = parse_header(&unsub_msg).unwrap();
    let h2 = parse_header(&dw_msg).unwrap();
    assert_eq!(h1.msg_type, MsgType::Unsubscribe);
    assert_eq!(h2.msg_type, MsgType::DeleteWriter);
    assert_eq!(h1.request_id, 100);
    assert_eq!(h2.request_id, 101);

    parse_unsubscribe(&unsub_msg[HEADER_SIZE..]).unwrap();
    parse_delete_writer(&dw_msg[HEADER_SIZE..]).unwrap();
}

#[test]
fn test_empty_strings_in_messages() {
    let sub = SubscribePayload {
        is_keyed: true,
        topic_name: "".into(),
        type_name: "".into(),
        qos: QosSet::default(),
        key_descriptors: KeyDescriptors { keys: vec![] },
    };
    let bytes = serialize_subscribe(&sub);
    let parsed = parse_subscribe(&bytes).unwrap();
    assert_eq!(parsed.topic_name, "");
    assert_eq!(parsed.type_name, "");
}

#[test]
fn test_unicode_strings() {
    let sub = SubscribePayload {
        is_keyed: true,
        topic_name: "sensor/temperature/\u{00B0}C".into(),
        type_name: "test::\u{30C7}\u{30FC}\u{30BF}".into(), // katakana
        qos: QosSet::default(),
        key_descriptors: KeyDescriptors { keys: vec![] },
    };
    let bytes = serialize_subscribe(&sub);
    let parsed = parse_subscribe(&bytes).unwrap();
    assert_eq!(parsed, sub);
}

#[test]
fn test_max_request_id() {
    let msg = build_message(MsgType::Ping, u32::MAX, &[]);
    let header = parse_header(&msg).unwrap();
    assert_eq!(header.request_id, u32::MAX);
}

#[test]
fn test_write_empty_data() {
    let write = WritePayload(WriteMode::TopicName {
        topic_name: "T".into(),
        type_name: "T".into(),
        qos: QosSet::default(),
        key_descriptors: KeyDescriptors { keys: vec![] },
        data: vec![],
    });
    let bytes = serialize_write(&write);
    let parsed = parse_write(&bytes).unwrap();
    if let WriteMode::TopicName { data, .. } = &parsed.0 {
        assert!(data.is_empty());
    } else {
        panic!("expected TopicName mode");
    }
}
