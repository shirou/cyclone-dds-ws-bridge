use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex};

use crate::config::Config;
use crate::dds::{
    make_qos, DdsParticipant, DdsWriter, InstanceState, ReaderRegistry, ReceivedSample,
    WriterRegistry,
};
use crate::protocol::{self, build_message, ErrorCode, ErrorPayload, MsgType};

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_session_id() -> u64 {
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

/// Message sent from the bridge to a client session.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    pub data: Vec<u8>,
}

/// Shared bridge state, protected by a mutex since DDS entities are not Send+Sync.
pub struct BridgeInner {
    pub participant: DdsParticipant,
    pub writers: WriterRegistry,
    pub readers: ReaderRegistry,
    /// session_id -> sender for outbound messages
    pub session_senders: HashMap<u64, mpsc::Sender<OutboundMessage>>,
    pub config: Config,
}

/// Thread-safe bridge handle.
pub type Bridge = Arc<Mutex<BridgeInner>>;

pub fn create_bridge(config: Config) -> Result<Bridge, crate::dds::DdsError> {
    let domain_id = if config.dds.domain_id == 0 {
        None
    } else {
        Some(config.dds.domain_id)
    };
    let participant = DdsParticipant::new(domain_id)?;
    Ok(Arc::new(Mutex::new(BridgeInner {
        participant,
        writers: WriterRegistry::new(),
        readers: ReaderRegistry::new(),
        session_senders: HashMap::new(),
        config,
    })))
}

/// Register a session's outbound channel.
pub async fn register_session(
    bridge: &Bridge,
    session_id: u64,
    sender: mpsc::Sender<OutboundMessage>,
) {
    let mut inner = bridge.lock().await;
    inner.session_senders.insert(session_id, sender);
}

/// Unregister a session and clean up all its DDS resources.
pub async fn unregister_session(bridge: &Bridge, session_id: u64) {
    let mut inner = bridge.lock().await;
    inner.session_senders.remove(&session_id);
    inner.writers.remove_session(session_id);
    inner.readers.remove_session(session_id);
    tracing::info!(session_id, "session cleaned up");
}

/// Handle a parsed client message. Returns a response message (header+payload) to send back.
pub async fn handle_message(
    bridge: &Bridge,
    session_id: u64,
    msg_type: MsgType,
    request_id: u32,
    payload: &[u8],
) -> Vec<u8> {
    let result = match msg_type {
        MsgType::Subscribe => handle_subscribe(bridge, session_id, request_id, payload).await,
        MsgType::Unsubscribe => handle_unsubscribe(bridge, session_id, request_id, payload).await,
        MsgType::Write => handle_write(bridge, session_id, request_id, payload).await,
        MsgType::Dispose => handle_dispose(bridge, session_id, request_id, payload).await,
        MsgType::WriteDispose => {
            handle_write_dispose(bridge, session_id, request_id, payload).await
        }
        MsgType::CreateWriter => {
            handle_create_writer(bridge, session_id, request_id, payload).await
        }
        MsgType::DeleteWriter => {
            handle_delete_writer(bridge, session_id, request_id, payload).await
        }
        MsgType::Ping => Ok(build_message(MsgType::Pong, request_id, &[])),
        _ => Err(make_error_response(
            request_id,
            ErrorCode::InvalidMessage,
            &format!("unexpected message type: {msg_type:?}"),
        )),
    };

    match result {
        Ok(response) => response,
        Err(error_msg) => error_msg,
    }
}

async fn handle_subscribe(
    bridge: &Bridge,
    session_id: u64,
    request_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let sub = protocol::parse_subscribe(payload)
        .map_err(|e| make_error_response(request_id, ErrorCode::InvalidMessage, &e.to_string()))?;

    let mut inner = bridge.lock().await;
    let topic = inner
        .participant
        .get_or_create_topic(&sub.topic_name, &sub.type_name, &sub.key_descriptors)
        .map_err(|e| make_error_response(request_id, ErrorCode::DdsError, &e.to_string()))?;

    let (_qos_handle, qos_ptr) = make_qos(&sub.qos);
    let participant_handle = inner.participant.handle;
    inner
        .readers
        .subscribe(
            participant_handle,
            &topic,
            qos_ptr,
            &sub.qos,
            session_id,
        )
        .map_err(|e| make_error_response(request_id, ErrorCode::DdsError, &e.to_string()))?;

    tracing::debug!(
        session_id,
        topic = sub.topic_name,
        type_name = sub.type_name,
        "subscribed"
    );

    Ok(build_message(MsgType::Ok, request_id, &[]))
}

async fn handle_unsubscribe(
    bridge: &Bridge,
    session_id: u64,
    request_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let unsub = protocol::parse_unsubscribe(payload)
        .map_err(|e| make_error_response(request_id, ErrorCode::InvalidMessage, &e.to_string()))?;

    let mut inner = bridge.lock().await;
    let found = inner
        .readers
        .unsubscribe(&unsub.topic_name, &unsub.type_name, session_id);

    if !found {
        return Err(make_error_response(
            request_id,
            ErrorCode::ReaderNotFound,
            &format!(
                "no subscription for topic='{}' type='{}'",
                unsub.topic_name, unsub.type_name
            ),
        ));
    }

    tracing::debug!(
        session_id,
        topic = unsub.topic_name,
        type_name = unsub.type_name,
        "unsubscribed"
    );

    Ok(build_message(MsgType::Ok, request_id, &[]))
}

async fn handle_write(
    bridge: &Bridge,
    session_id: u64,
    request_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let write_payload = protocol::parse_write(payload)
        .map_err(|e| make_error_response(request_id, ErrorCode::InvalidMessage, &e.to_string()))?;

    let mut inner = bridge.lock().await;
    do_write_op(&mut inner, session_id, request_id, &write_payload.0, WriteOp::Write)
}

async fn handle_dispose(
    bridge: &Bridge,
    session_id: u64,
    request_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let dispose_payload = protocol::parse_dispose(payload)
        .map_err(|e| make_error_response(request_id, ErrorCode::InvalidMessage, &e.to_string()))?;

    let mut inner = bridge.lock().await;
    do_write_op(
        &mut inner,
        session_id,
        request_id,
        &dispose_payload.0,
        WriteOp::Dispose,
    )
}

async fn handle_write_dispose(
    bridge: &Bridge,
    session_id: u64,
    request_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let wd_payload = protocol::parse_write_dispose(payload)
        .map_err(|e| make_error_response(request_id, ErrorCode::InvalidMessage, &e.to_string()))?;

    let mut inner = bridge.lock().await;
    do_write_op(
        &mut inner,
        session_id,
        request_id,
        &wd_payload.0,
        WriteOp::WriteDispose,
    )
}

#[derive(Clone, Copy)]
enum WriteOp {
    Write,
    Dispose,
    WriteDispose,
}

fn do_write_op(
    inner: &mut BridgeInner,
    session_id: u64,
    request_id: u32,
    mode: &protocol::WriteMode,
    op: WriteOp,
) -> Result<Vec<u8>, Vec<u8>> {
    match mode {
        protocol::WriteMode::TopicName {
            topic_name,
            type_name,
            qos,
            key_descriptors,
            data,
        } => {
            let normalized = qos.normalize();

            // Try to reuse an existing implicit writer for this session
            let writer_exists = inner
                .writers
                .find_by_topic(session_id, topic_name, type_name, &normalized)
                .is_some();

            if !writer_exists {
                // Create topic + writer
                let topic = inner
                    .participant
                    .get_or_create_topic(topic_name, type_name, key_descriptors)
                    .map_err(|e| {
                        make_error_response(request_id, ErrorCode::DdsError, &e.to_string())
                    })?;

                let (_qos_handle, qos_ptr) = make_qos(qos);
                let writer =
                    DdsWriter::new(inner.participant.handle, &topic, qos_ptr, normalized.clone())
                        .map_err(|e| {
                            make_error_response(request_id, ErrorCode::DdsError, &e.to_string())
                        })?;
                inner.writers.register(writer, session_id);
            }

            // Now get the writer and perform the operation
            let writer = inner
                .writers
                .find_by_topic(session_id, topic_name, type_name, &normalized)
                .ok_or_else(|| {
                    make_error_response(
                        request_id,
                        ErrorCode::InternalError,
                        "writer disappeared after creation",
                    )
                })?;

            execute_write_op(writer, op, data, request_id)?;
            Ok(build_message(MsgType::Ok, request_id, &[]))
        }
        protocol::WriteMode::WriterId { writer_id, data } => {
            let writer = inner.writers.get(*writer_id, session_id).map_err(|e| {
                let code = match &e {
                    crate::dds::DdsError::WriterNotFound(_) => ErrorCode::WriterNotFound,
                    crate::dds::DdsError::WriterOwnership(_) => ErrorCode::WriterNotFound,
                    _ => ErrorCode::DdsError,
                };
                make_error_response(request_id, code, &e.to_string())
            })?;

            execute_write_op(writer, op, data, request_id)?;
            Ok(build_message(MsgType::Ok, request_id, &[]))
        }
    }
}

fn execute_write_op(
    writer: &DdsWriter,
    op: WriteOp,
    data: &[u8],
    request_id: u32,
) -> Result<(), Vec<u8>> {
    let result = match op {
        WriteOp::Write => writer.write(data),
        WriteOp::Dispose => writer.dispose(data),
        WriteOp::WriteDispose => writer.write_dispose(data),
    };
    result.map_err(|e| make_error_response(request_id, ErrorCode::DdsError, &e.to_string()))
}

async fn handle_create_writer(
    bridge: &Bridge,
    session_id: u64,
    request_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let cw = protocol::parse_create_writer(payload)
        .map_err(|e| make_error_response(request_id, ErrorCode::InvalidMessage, &e.to_string()))?;

    let mut inner = bridge.lock().await;
    let topic = inner
        .participant
        .get_or_create_topic(&cw.topic_name, &cw.type_name, &cw.key_descriptors)
        .map_err(|e| make_error_response(request_id, ErrorCode::DdsError, &e.to_string()))?;

    let (_qos_handle, qos_ptr) = make_qos(&cw.qos);
    let normalized = cw.qos.normalize();
    let writer = DdsWriter::new(inner.participant.handle, &topic, qos_ptr, normalized)
        .map_err(|e| make_error_response(request_id, ErrorCode::DdsError, &e.to_string()))?;

    let writer_id = inner.writers.register(writer, session_id);

    tracing::debug!(session_id, writer_id, topic = cw.topic_name, "writer created");

    // OK response payload: writer_id as 4 bytes LE
    let ok_payload = writer_id.to_le_bytes().to_vec();
    Ok(build_message(MsgType::Ok, request_id, &ok_payload))
}

async fn handle_delete_writer(
    bridge: &Bridge,
    session_id: u64,
    request_id: u32,
    payload: &[u8],
) -> Result<Vec<u8>, Vec<u8>> {
    let dw = protocol::parse_delete_writer(payload)
        .map_err(|e| make_error_response(request_id, ErrorCode::InvalidMessage, &e.to_string()))?;

    let mut inner = bridge.lock().await;
    inner
        .writers
        .delete(dw.writer_id, session_id)
        .map_err(|e| {
            let code = match &e {
                crate::dds::DdsError::WriterNotFound(_) => ErrorCode::WriterNotFound,
                crate::dds::DdsError::WriterOwnership(_) => ErrorCode::WriterNotFound,
                _ => ErrorCode::DdsError,
            };
            make_error_response(request_id, code, &e.to_string())
        })?;

    tracing::debug!(session_id, writer_id = dw.writer_id, "writer deleted");

    Ok(build_message(MsgType::Ok, request_id, &[]))
}

fn make_error_response(request_id: u32, code: ErrorCode, message: &str) -> Vec<u8> {
    let payload = protocol::serialize_error(&ErrorPayload {
        error_code: code,
        error_message: message.to_string(),
    });
    build_message(MsgType::Error, request_id, &payload)
}

/// DDS reader polling task: periodically takes samples from all readers
/// and fans out to subscribed sessions.
pub async fn reader_poll_loop(bridge: Bridge, poll_interval: Duration) {
    let mut interval = tokio::time::interval(poll_interval);
    loop {
        interval.tick().await;

        // Take samples and collect fan-out info under the lock, then release it.
        let fan_out = {
            let inner = bridge.lock().await;

            let reader_ids: Vec<u64> =
                inner.readers.all_readers().map(|r| r.reader_id).collect();

            let mut pending: Vec<(Vec<ReceivedSample>, Vec<(u64, mpsc::Sender<OutboundMessage>)>)> =
                Vec::new();

            for reader_id in reader_ids {
                let samples = {
                    let reader = match inner.readers.get(reader_id) {
                        Some(r) => r,
                        None => continue,
                    };
                    match reader.take_samples(256) {
                        Ok(s) if !s.is_empty() => s,
                        Ok(_) => continue,
                        Err(e) => {
                            tracing::warn!(reader_id, error = %e, "failed to take samples");
                            continue;
                        }
                    }
                };

                let subscribers: Vec<(u64, mpsc::Sender<OutboundMessage>)> = inner
                    .readers
                    .get_subscribers(reader_id)
                    .map(|sids| {
                        sids.iter()
                            .filter_map(|&sid| {
                                inner
                                    .session_senders
                                    .get(&sid)
                                    .map(|tx| (sid, tx.clone()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                pending.push((samples, subscribers));
            }

            pending
            // lock released here
        };

        // Fan out messages without holding the bridge lock
        for (samples, subscribers) in &fan_out {
            for sample in samples {
                let msg = sample_to_message(sample);
                for (sid, sender) in subscribers {
                    if sender
                        .try_send(OutboundMessage { data: msg.clone() })
                        .is_err()
                    {
                        tracing::warn!(session_id = sid, "session buffer full, dropping message");
                    }
                }
            }
        }
    }
}

fn sample_to_message(sample: &ReceivedSample) -> Vec<u8> {
    match sample.instance_state {
        InstanceState::Alive => {
            let payload = protocol::serialize_data(&protocol::DataPayload {
                topic_name: sample.topic_name.clone(),
                source_timestamp: sample.source_timestamp as u64,
                data: sample.data.clone(),
            });
            build_message(MsgType::Data, 0, &payload)
        }
        InstanceState::Disposed => {
            let payload =
                protocol::serialize_data_disposed(&protocol::DataDisposedPayload {
                    topic_name: sample.topic_name.clone(),
                    source_timestamp: sample.source_timestamp as u64,
                    key_data: sample.data.clone(),
                });
            build_message(MsgType::DataDisposed, 0, &payload)
        }
        InstanceState::NoWriters => {
            let payload =
                protocol::serialize_data_no_writers(&protocol::DataNoWritersPayload {
                    topic_name: sample.topic_name.clone(),
                    key_data: sample.data.clone(),
                });
            build_message(MsgType::DataNoWriters, 0, &payload)
        }
    }
}
