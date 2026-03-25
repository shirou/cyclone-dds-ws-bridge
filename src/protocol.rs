use bytes::{Buf, BufMut, BytesMut};
use thiserror::Error;

use crate::qos::QosSet;

// Header constants
pub const MAGIC: [u8; 2] = [0x44, 0x42]; // "DB"
pub const VERSION: u8 = 0x01;
pub const HEADER_SIZE: usize = 12;
pub const DEFAULT_MAX_PAYLOAD_SIZE: usize = 4 * 1024 * 1024; // 4 MiB

/// Defense-in-depth limit for individual string fields.
/// Prevents multi-GB allocation if parse functions are called
/// without prior header validation.
const MAX_STRING_LENGTH: usize = 1024 * 1024; // 1 MiB

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    // Client -> Bridge
    Subscribe = 0x01,
    Unsubscribe = 0x02,
    Write = 0x03,
    Dispose = 0x04,
    WriteDispose = 0x05,
    CreateWriter = 0x10,
    DeleteWriter = 0x11,
    Ping = 0x0F,
    // Bridge -> Client
    Ok = 0x80,
    Data = 0xC0,
    DataDisposed = 0xC1,
    DataNoWriters = 0xC2,
    Error = 0xFE,
    Pong = 0x8F,
}

impl MsgType {
    pub fn from_u8(val: u8) -> Result<Self, ProtocolError> {
        match val {
            0x01 => Ok(Self::Subscribe),
            0x02 => Ok(Self::Unsubscribe),
            0x03 => Ok(Self::Write),
            0x04 => Ok(Self::Dispose),
            0x05 => Ok(Self::WriteDispose),
            0x10 => Ok(Self::CreateWriter),
            0x11 => Ok(Self::DeleteWriter),
            0x0F => Ok(Self::Ping),
            0x80 => Ok(Self::Ok),
            0xC0 => Ok(Self::Data),
            0xC1 => Ok(Self::DataDisposed),
            0xC2 => Ok(Self::DataNoWriters),
            0xFE => Ok(Self::Error),
            0x8F => Ok(Self::Pong),
            _ => Err(ProtocolError::InvalidMsgType(val)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u8,
    pub msg_type: MsgType,
    pub request_id: u32,
    pub payload_length: u32,
}

pub fn parse_header(data: &[u8]) -> Result<Header, ProtocolError> {
    if data.len() < HEADER_SIZE {
        return Err(ProtocolError::Truncated {
            expected: HEADER_SIZE,
            got: data.len(),
        });
    }
    if data[0] != MAGIC[0] || data[1] != MAGIC[1] {
        return Err(ProtocolError::BadMagic([data[0], data[1]]));
    }
    let version = data[2];
    let msg_type = MsgType::from_u8(data[3])?;
    let request_id = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    let payload_length = u32::from_le_bytes([data[8], data[9], data[10], data[11]]);

    Ok(Header {
        version,
        msg_type,
        request_id,
        payload_length,
    })
}

pub fn serialize_header(header: &Header) -> [u8; HEADER_SIZE] {
    let mut buf = [0u8; HEADER_SIZE];
    buf[0] = MAGIC[0];
    buf[1] = MAGIC[1];
    buf[2] = header.version;
    buf[3] = header.msg_type as u8;
    buf[4..8].copy_from_slice(&header.request_id.to_le_bytes());
    buf[8..12].copy_from_slice(&header.payload_length.to_le_bytes());
    buf
}

// --- String encoding helpers ---

pub fn read_bool(buf: &mut &[u8]) -> Result<bool, ProtocolError> {
    if buf.remaining() < 1 {
        return Err(ProtocolError::Truncated {
            expected: 1,
            got: 0,
        });
    }
    Ok(buf.get_u8() != 0)
}

pub fn read_string(buf: &mut &[u8]) -> Result<String, ProtocolError> {
    if buf.remaining() < 4 {
        return Err(ProtocolError::Truncated {
            expected: 4,
            got: buf.remaining(),
        });
    }
    let len = buf.get_u32_le() as usize;
    if len > MAX_STRING_LENGTH {
        return Err(ProtocolError::StringTooLong {
            length: len,
            max: MAX_STRING_LENGTH,
        });
    }
    if buf.remaining() < len {
        return Err(ProtocolError::Truncated {
            expected: len,
            got: buf.remaining(),
        });
    }
    let s = std::str::from_utf8(&buf[..len])
        .map_err(|e| ProtocolError::InvalidUtf8(e.to_string()))?
        .to_string();
    buf.advance(len);
    Ok(s)
}

pub fn write_string(buf: &mut BytesMut, s: &str) {
    buf.put_u32_le(s.len() as u32);
    buf.put_slice(s.as_bytes());
}

fn check_no_trailing(buf: &[u8]) -> Result<(), ProtocolError> {
    if !buf.is_empty() {
        return Err(ProtocolError::TrailingBytes(buf.len()));
    }
    Ok(())
}

// --- Key Descriptors ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum KeyTypeHint {
    Opaque = 0x00,
    Uuid = 0x01,
    Int32 = 0x02,
    Int64 = 0x03,
    String = 0x04,
}

impl KeyTypeHint {
    pub fn from_u8(val: u8) -> Result<Self, ProtocolError> {
        match val {
            0x00 => Ok(Self::Opaque),
            0x01 => Ok(Self::Uuid),
            0x02 => Ok(Self::Int32),
            0x03 => Ok(Self::Int64),
            0x04 => Ok(Self::String),
            _ => Err(ProtocolError::InvalidKeyTypeHint(val)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyField {
    pub offset: u32,
    pub size: u32,
    pub type_hint: KeyTypeHint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDescriptors {
    pub keys: Vec<KeyField>,
}

pub fn parse_key_descriptors(buf: &mut &[u8]) -> Result<KeyDescriptors, ProtocolError> {
    if buf.remaining() < 1 {
        return Err(ProtocolError::Truncated {
            expected: 1,
            got: 0,
        });
    }
    let key_count = buf.get_u8() as usize;
    let mut keys = Vec::with_capacity(key_count);
    for _ in 0..key_count {
        if buf.remaining() < 9 {
            return Err(ProtocolError::Truncated {
                expected: 9,
                got: buf.remaining(),
            });
        }
        let offset = buf.get_u32_le();
        let size = buf.get_u32_le();
        let type_hint = KeyTypeHint::from_u8(buf.get_u8())?;
        keys.push(KeyField {
            offset,
            size,
            type_hint,
        });
    }
    Ok(KeyDescriptors { keys })
}

pub fn serialize_key_descriptors(kd: &KeyDescriptors, buf: &mut BytesMut) {
    buf.put_u8(kd.keys.len() as u8);
    for key in &kd.keys {
        buf.put_u32_le(key.offset);
        buf.put_u32_le(key.size);
        buf.put_u8(key.type_hint as u8);
    }
}

// --- Message payloads ---

#[derive(Debug, Clone, PartialEq)]
pub struct SubscribePayload {
    pub topic_name: String,
    pub type_name: String,
    pub qos: QosSet,
    pub is_keyed: bool,
    pub key_descriptors: KeyDescriptors,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UnsubscribePayload {
    pub topic_name: String,
    pub type_name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateWriterPayload {
    pub topic_name: String,
    pub type_name: String,
    pub qos: QosSet,
    pub is_keyed: bool,
    pub key_descriptors: KeyDescriptors,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeleteWriterPayload {
    pub writer_id: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WriteMode {
    TopicName {
        topic_name: String,
        type_name: String,
        qos: QosSet,
        key_descriptors: KeyDescriptors,
        data: Vec<u8>,
    },
    WriterId {
        writer_id: u32,
        key_bytes: Vec<u8>,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WritePayload(pub WriteMode);

#[derive(Debug, Clone, PartialEq)]
pub struct DisposePayload(pub WriteMode);

#[derive(Debug, Clone, PartialEq)]
pub struct WriteDisposePayload(pub WriteMode);

// Bridge -> Client payloads

#[derive(Debug, Clone, PartialEq)]
pub struct DataPayload {
    pub topic_name: String,
    pub source_timestamp: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataDisposedPayload {
    pub topic_name: String,
    pub source_timestamp: u64,
    pub key_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DataNoWritersPayload {
    pub topic_name: String,
    pub key_data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    InvalidMessage = 0x0001,
    TopicNotFound = 0x0002,
    DdsError = 0x0003,
    WriterNotFound = 0x0004,
    ReaderNotFound = 0x0005,
    InvalidQos = 0x0006,
    AlreadyExists = 0x0007,
    InternalError = 0x0008,
    BufferOverflow = 0x0009,
}

impl ErrorCode {
    pub fn from_u16(val: u16) -> Result<Self, ProtocolError> {
        match val {
            0x0001 => Ok(Self::InvalidMessage),
            0x0002 => Ok(Self::TopicNotFound),
            0x0003 => Ok(Self::DdsError),
            0x0004 => Ok(Self::WriterNotFound),
            0x0005 => Ok(Self::ReaderNotFound),
            0x0006 => Ok(Self::InvalidQos),
            0x0007 => Ok(Self::AlreadyExists),
            0x0008 => Ok(Self::InternalError),
            0x0009 => Ok(Self::BufferOverflow),
            _ => Err(ProtocolError::InvalidErrorCode(val)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorPayload {
    pub error_code: ErrorCode,
    pub error_message: String,
}

// --- Parse functions ---

pub fn parse_subscribe(payload: &[u8]) -> Result<SubscribePayload, ProtocolError> {
    let mut buf: &[u8] = payload;
    let topic_name = read_string(&mut buf)?;
    let type_name = read_string(&mut buf)?;
    let qos = crate::qos::parse_qos(&mut buf)?;
    let is_keyed = read_bool(&mut buf)?;
    let key_descriptors = parse_key_descriptors(&mut buf)?;
    check_no_trailing(buf)?;
    Ok(SubscribePayload {
        topic_name,
        type_name,
        qos,
        is_keyed,
        key_descriptors,
    })
}

pub fn parse_unsubscribe(payload: &[u8]) -> Result<UnsubscribePayload, ProtocolError> {
    let mut buf: &[u8] = payload;
    let topic_name = read_string(&mut buf)?;
    let type_name = read_string(&mut buf)?;
    check_no_trailing(buf)?;
    Ok(UnsubscribePayload {
        topic_name,
        type_name,
    })
}

pub fn parse_write_mode(payload: &[u8]) -> Result<WriteMode, ProtocolError> {
    if payload.is_empty() {
        return Err(ProtocolError::Truncated {
            expected: 1,
            got: 0,
        });
    }
    let mode = payload[0];
    let mut buf: &[u8] = &payload[1..];
    match mode {
        0x00 => {
            let topic_name = read_string(&mut buf)?;
            let type_name = read_string(&mut buf)?;
            let qos = crate::qos::parse_qos(&mut buf)?;
            let key_descriptors = parse_key_descriptors(&mut buf)?;
            let data = buf.to_vec();
            Ok(WriteMode::TopicName {
                topic_name,
                type_name,
                qos,
                key_descriptors,
                data,
            })
        }
        0x01 => {
            if buf.remaining() < 8 {
                return Err(ProtocolError::Truncated {
                    expected: 8,
                    got: buf.remaining(),
                });
            }
            let writer_id = buf.get_u32_le();
            let key_len = buf.get_u32_le() as usize;
            if buf.remaining() < key_len {
                return Err(ProtocolError::Truncated {
                    expected: key_len,
                    got: buf.remaining(),
                });
            }
            let key_bytes = buf[..key_len].to_vec();
            buf.advance(key_len);
            let data = buf.to_vec();
            Ok(WriteMode::WriterId {
                writer_id,
                key_bytes,
                data,
            })
        }
        _ => Err(ProtocolError::InvalidWriteMode(mode)),
    }
}

pub fn parse_write(payload: &[u8]) -> Result<WritePayload, ProtocolError> {
    parse_write_mode(payload).map(WritePayload)
}

pub fn parse_dispose(payload: &[u8]) -> Result<DisposePayload, ProtocolError> {
    parse_write_mode(payload).map(DisposePayload)
}

pub fn parse_write_dispose(payload: &[u8]) -> Result<WriteDisposePayload, ProtocolError> {
    parse_write_mode(payload).map(WriteDisposePayload)
}

pub fn parse_create_writer(payload: &[u8]) -> Result<CreateWriterPayload, ProtocolError> {
    let mut buf: &[u8] = payload;
    let topic_name = read_string(&mut buf)?;
    let type_name = read_string(&mut buf)?;
    let qos = crate::qos::parse_qos(&mut buf)?;
    let is_keyed = read_bool(&mut buf)?;
    let key_descriptors = parse_key_descriptors(&mut buf)?;
    check_no_trailing(buf)?;
    Ok(CreateWriterPayload {
        topic_name,
        type_name,
        qos,
        is_keyed,
        key_descriptors,
    })
}

pub fn parse_delete_writer(payload: &[u8]) -> Result<DeleteWriterPayload, ProtocolError> {
    if payload.len() < 4 {
        return Err(ProtocolError::Truncated {
            expected: 4,
            got: payload.len(),
        });
    }
    let writer_id = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    check_no_trailing(&payload[4..])?;
    Ok(DeleteWriterPayload { writer_id })
}

pub fn parse_error(payload: &[u8]) -> Result<ErrorPayload, ProtocolError> {
    let mut buf: &[u8] = payload;
    if buf.remaining() < 2 {
        return Err(ProtocolError::Truncated {
            expected: 2,
            got: buf.remaining(),
        });
    }
    let error_code = ErrorCode::from_u16(buf.get_u16_le())?;
    let error_message = read_string(&mut buf)?;
    check_no_trailing(buf)?;
    Ok(ErrorPayload {
        error_code,
        error_message,
    })
}

pub fn parse_data(payload: &[u8]) -> Result<DataPayload, ProtocolError> {
    let mut buf: &[u8] = payload;
    let topic_name = read_string(&mut buf)?;
    if buf.remaining() < 8 {
        return Err(ProtocolError::Truncated {
            expected: 8,
            got: buf.remaining(),
        });
    }
    let source_timestamp = buf.get_u64_le();
    let data = buf.to_vec();
    Ok(DataPayload {
        topic_name,
        source_timestamp,
        data,
    })
}

pub fn parse_data_disposed(payload: &[u8]) -> Result<DataDisposedPayload, ProtocolError> {
    let mut buf: &[u8] = payload;
    let topic_name = read_string(&mut buf)?;
    if buf.remaining() < 8 {
        return Err(ProtocolError::Truncated {
            expected: 8,
            got: buf.remaining(),
        });
    }
    let source_timestamp = buf.get_u64_le();
    let key_data = buf.to_vec();
    Ok(DataDisposedPayload {
        topic_name,
        source_timestamp,
        key_data,
    })
}

pub fn parse_data_no_writers(payload: &[u8]) -> Result<DataNoWritersPayload, ProtocolError> {
    let mut buf: &[u8] = payload;
    let topic_name = read_string(&mut buf)?;
    let key_data = buf.to_vec();
    Ok(DataNoWritersPayload {
        topic_name,
        key_data,
    })
}

// --- Serialize functions ---

pub fn serialize_subscribe(p: &SubscribePayload) -> BytesMut {
    let mut buf = BytesMut::new();
    write_string(&mut buf, &p.topic_name);
    write_string(&mut buf, &p.type_name);
    crate::qos::serialize_qos(&p.qos, &mut buf);
    buf.put_u8(if p.is_keyed { 1 } else { 0 });
    serialize_key_descriptors(&p.key_descriptors, &mut buf);
    buf
}

pub fn serialize_unsubscribe(p: &UnsubscribePayload) -> BytesMut {
    let mut buf = BytesMut::new();
    write_string(&mut buf, &p.topic_name);
    write_string(&mut buf, &p.type_name);
    buf
}

pub fn serialize_write_mode(mode: &WriteMode) -> BytesMut {
    let mut buf = BytesMut::new();
    match mode {
        WriteMode::TopicName {
            topic_name,
            type_name,
            qos,
            key_descriptors,
            data,
        } => {
            buf.put_u8(0x00);
            write_string(&mut buf, topic_name);
            write_string(&mut buf, type_name);
            crate::qos::serialize_qos(qos, &mut buf);
            serialize_key_descriptors(key_descriptors, &mut buf);
            buf.put_slice(data);
        }
        WriteMode::WriterId {
            writer_id,
            key_bytes,
            data,
        } => {
            buf.put_u8(0x01);
            buf.put_u32_le(*writer_id);
            buf.put_u32_le(key_bytes.len() as u32);
            buf.put_slice(key_bytes);
            buf.put_slice(data);
        }
    }
    buf
}

pub fn serialize_write(p: &WritePayload) -> BytesMut {
    serialize_write_mode(&p.0)
}

pub fn serialize_dispose(p: &DisposePayload) -> BytesMut {
    serialize_write_mode(&p.0)
}

pub fn serialize_write_dispose(p: &WriteDisposePayload) -> BytesMut {
    serialize_write_mode(&p.0)
}

pub fn serialize_create_writer(p: &CreateWriterPayload) -> BytesMut {
    let mut buf = BytesMut::new();
    write_string(&mut buf, &p.topic_name);
    write_string(&mut buf, &p.type_name);
    crate::qos::serialize_qos(&p.qos, &mut buf);
    buf.put_u8(if p.is_keyed { 1 } else { 0 });
    serialize_key_descriptors(&p.key_descriptors, &mut buf);
    buf
}

pub fn serialize_delete_writer(p: &DeleteWriterPayload) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.put_u32_le(p.writer_id);
    buf
}

pub fn serialize_error(p: &ErrorPayload) -> BytesMut {
    let mut buf = BytesMut::new();
    buf.put_u16_le(p.error_code as u16);
    write_string(&mut buf, &p.error_message);
    buf
}

pub fn serialize_data(p: &DataPayload) -> BytesMut {
    let mut buf = BytesMut::new();
    write_string(&mut buf, &p.topic_name);
    buf.put_u64_le(p.source_timestamp);
    buf.put_slice(&p.data);
    buf
}

pub fn serialize_data_disposed(p: &DataDisposedPayload) -> BytesMut {
    let mut buf = BytesMut::new();
    write_string(&mut buf, &p.topic_name);
    buf.put_u64_le(p.source_timestamp);
    buf.put_slice(&p.key_data);
    buf
}

pub fn serialize_data_no_writers(p: &DataNoWritersPayload) -> BytesMut {
    let mut buf = BytesMut::new();
    write_string(&mut buf, &p.topic_name);
    buf.put_slice(&p.key_data);
    buf
}

/// Build a complete message (header + payload).
pub fn build_message(msg_type: MsgType, request_id: u32, payload: &[u8]) -> Vec<u8> {
    let header = Header {
        version: VERSION,
        msg_type,
        request_id,
        payload_length: payload.len() as u32,
    };
    let mut msg = Vec::with_capacity(HEADER_SIZE + payload.len());
    msg.extend_from_slice(&serialize_header(&header));
    msg.extend_from_slice(payload);
    msg
}

/// Validate an incoming client message header.
pub fn validate_client_header(
    header: &Header,
    max_version: u8,
    max_payload_size: usize,
) -> Result<(), ProtocolError> {
    if header.version > max_version {
        return Err(ProtocolError::UnsupportedVersion {
            got: header.version,
            max: max_version,
        });
    }
    if header.payload_length as usize > max_payload_size {
        return Err(ProtocolError::PayloadTooLarge {
            size: header.payload_length as usize,
            max: max_payload_size,
        });
    }
    if header.request_id == 0 {
        return Err(ProtocolError::ReservedRequestId);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("bad magic: expected [0x44, 0x42], got [{:#04x}, {:#04x}]", .0[0], .0[1])]
    BadMagic([u8; 2]),
    #[error("unsupported protocol version {got}; bridge supports up to version {max}")]
    UnsupportedVersion { got: u8, max: u8 },
    #[error("invalid msg_type: {0:#04x}")]
    InvalidMsgType(u8),
    #[error("truncated message: expected {expected} bytes, got {got}")]
    Truncated { expected: usize, got: usize },
    #[error("payload too large: {size} bytes exceeds maximum {max}")]
    PayloadTooLarge { size: usize, max: usize },
    #[error("request_id 0 is reserved for bridge-initiated messages")]
    ReservedRequestId,
    #[error("invalid write mode: {0:#04x}")]
    InvalidWriteMode(u8),
    #[error("invalid UTF-8: {0}")]
    InvalidUtf8(String),
    #[error("invalid key type hint: {0:#04x}")]
    InvalidKeyTypeHint(u8),
    #[error("invalid error code: {0:#06x}")]
    InvalidErrorCode(u16),
    #[error("invalid QoS policy id: {0:#04x}")]
    InvalidQosPolicyId(u8),
    #[error("invalid QoS value: {0}")]
    InvalidQosValue(String),
    #[error("duplicate QoS policy id: {0:#04x}")]
    DuplicateQosPolicy(u8),
    #[error("string too long: {length} bytes exceeds maximum {max}")]
    StringTooLong { length: usize, max: usize },
    #[error("unexpected {0} trailing bytes")]
    TrailingBytes(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_round_trip() {
        let header = Header {
            version: VERSION,
            msg_type: MsgType::Subscribe,
            request_id: 42,
            payload_length: 100,
        };
        let bytes = serialize_header(&header);
        let parsed = parse_header(&bytes).unwrap();
        assert_eq!(header, parsed);
    }

    #[test]
    fn test_header_all_msg_types() {
        let types = [
            MsgType::Subscribe,
            MsgType::Unsubscribe,
            MsgType::Write,
            MsgType::Dispose,
            MsgType::WriteDispose,
            MsgType::CreateWriter,
            MsgType::DeleteWriter,
            MsgType::Ping,
            MsgType::Ok,
            MsgType::Data,
            MsgType::DataDisposed,
            MsgType::DataNoWriters,
            MsgType::Error,
            MsgType::Pong,
        ];
        for msg_type in types {
            let header = Header {
                version: VERSION,
                msg_type,
                request_id: 1,
                payload_length: 0,
            };
            let bytes = serialize_header(&header);
            let parsed = parse_header(&bytes).unwrap();
            assert_eq!(parsed.msg_type, msg_type);
        }
    }

    #[test]
    fn test_bad_magic() {
        let mut bytes = serialize_header(&Header {
            version: VERSION,
            msg_type: MsgType::Ping,
            request_id: 1,
            payload_length: 0,
        });
        bytes[0] = 0xFF;
        assert!(matches!(
            parse_header(&bytes),
            Err(ProtocolError::BadMagic(_))
        ));
    }

    #[test]
    fn test_truncated_header() {
        let err = parse_header(&[0x44, 0x42, 0x01]).unwrap_err();
        assert!(matches!(err, ProtocolError::Truncated { .. }));
    }

    #[test]
    fn test_invalid_msg_type() {
        let mut bytes = serialize_header(&Header {
            version: VERSION,
            msg_type: MsgType::Ping,
            request_id: 1,
            payload_length: 0,
        });
        bytes[3] = 0xFF;
        assert!(matches!(
            parse_header(&bytes),
            Err(ProtocolError::InvalidMsgType(0xFF))
        ));
    }

    #[test]
    fn test_unsupported_version() {
        let header = Header {
            version: 99,
            msg_type: MsgType::Ping,
            request_id: 1,
            payload_length: 0,
        };
        let err = validate_client_header(&header, VERSION, DEFAULT_MAX_PAYLOAD_SIZE).unwrap_err();
        assert!(matches!(
            err,
            ProtocolError::UnsupportedVersion { got: 99, max: 1 }
        ));
    }

    #[test]
    fn test_payload_too_large() {
        let header = Header {
            version: VERSION,
            msg_type: MsgType::Write,
            request_id: 1,
            payload_length: 10_000_000,
        };
        let err = validate_client_header(&header, VERSION, DEFAULT_MAX_PAYLOAD_SIZE).unwrap_err();
        assert!(matches!(err, ProtocolError::PayloadTooLarge { .. }));
    }

    #[test]
    fn test_reserved_request_id() {
        let header = Header {
            version: VERSION,
            msg_type: MsgType::Subscribe,
            request_id: 0,
            payload_length: 0,
        };
        let err = validate_client_header(&header, VERSION, DEFAULT_MAX_PAYLOAD_SIZE).unwrap_err();
        assert!(matches!(err, ProtocolError::ReservedRequestId));
    }

    #[test]
    fn test_string_round_trip() {
        let mut buf = BytesMut::new();
        write_string(&mut buf, "hello");
        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        let s = read_string(&mut slice).unwrap();
        assert_eq!(s, "hello");
        assert!(slice.is_empty());
    }

    #[test]
    fn test_string_empty() {
        let mut buf = BytesMut::new();
        write_string(&mut buf, "");
        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        let s = read_string(&mut slice).unwrap();
        assert_eq!(s, "");
    }

    #[test]
    fn test_string_too_long() {
        // Craft a buffer with a length prefix exceeding MAX_STRING_LENGTH
        let mut buf = BytesMut::new();
        buf.put_u32_le((MAX_STRING_LENGTH + 1) as u32);
        buf.put_slice(&vec![0x41; MAX_STRING_LENGTH + 1]);
        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        assert!(matches!(
            read_string(&mut slice),
            Err(ProtocolError::StringTooLong { .. })
        ));
    }

    #[test]
    fn test_subscribe_round_trip() {
        let payload = SubscribePayload {
            topic_name: "test_topic".into(),
            type_name: "TestType".into(),
            qos: QosSet::default(),
            is_keyed: true,
            key_descriptors: KeyDescriptors { keys: vec![] },
        };
        let bytes = serialize_subscribe(&payload);
        let parsed = parse_subscribe(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_unsubscribe_round_trip() {
        let payload = UnsubscribePayload {
            topic_name: "test_topic".into(),
            type_name: "TestType".into(),
        };
        let bytes = serialize_unsubscribe(&payload);
        let parsed = parse_unsubscribe(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_write_topic_mode_round_trip() {
        let payload = WritePayload(WriteMode::TopicName {
            topic_name: "t".into(),
            type_name: "T".into(),
            qos: QosSet::default(),
            key_descriptors: KeyDescriptors { keys: vec![] },
            data: vec![1, 2, 3, 4],
        });
        let bytes = serialize_write(&payload);
        let parsed = parse_write(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_write_writer_id_mode_round_trip() {
        let payload = WritePayload(WriteMode::WriterId {
            writer_id: 42,
            key_bytes: vec![],
            data: vec![0xDE, 0xAD],
        });
        let bytes = serialize_write(&payload);
        let parsed = parse_write(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_dispose_round_trip() {
        let payload = DisposePayload(WriteMode::WriterId {
            writer_id: 7,
            key_bytes: vec![],
            data: vec![0xFF],
        });
        let bytes = serialize_dispose(&payload);
        let parsed = parse_dispose(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_create_writer_round_trip() {
        let payload = CreateWriterPayload {
            topic_name: "my_topic".into(),
            type_name: "MyType".into(),
            qos: QosSet::default(),
            is_keyed: true,
            key_descriptors: KeyDescriptors {
                keys: vec![KeyField {
                    offset: 4,
                    size: 8,
                    type_hint: KeyTypeHint::Int64,
                }],
            },
        };
        let bytes = serialize_create_writer(&payload);
        let parsed = parse_create_writer(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_delete_writer_round_trip() {
        let payload = DeleteWriterPayload { writer_id: 123 };
        let bytes = serialize_delete_writer(&payload);
        let parsed = parse_delete_writer(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_error_round_trip() {
        let payload = ErrorPayload {
            error_code: ErrorCode::DdsError,
            error_message: "something went wrong".into(),
        };
        let bytes = serialize_error(&payload);
        let parsed = parse_error(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_data_round_trip() {
        let payload = DataPayload {
            topic_name: "sensor".into(),
            source_timestamp: 1234567890_000_000_000,
            data: vec![10, 20, 30],
        };
        let bytes = serialize_data(&payload);
        let parsed = parse_data(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_data_disposed_round_trip() {
        let payload = DataDisposedPayload {
            topic_name: "sensor".into(),
            source_timestamp: 999,
            key_data: vec![1, 2],
        };
        let bytes = serialize_data_disposed(&payload);
        let parsed = parse_data_disposed(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_data_no_writers_round_trip() {
        let payload = DataNoWritersPayload {
            topic_name: "sensor".into(),
            key_data: vec![5, 6, 7],
        };
        let bytes = serialize_data_no_writers(&payload);
        let parsed = parse_data_no_writers(&bytes).unwrap();
        assert_eq!(payload, parsed);
    }

    #[test]
    fn test_key_descriptors_round_trip() {
        let kd = KeyDescriptors {
            keys: vec![
                KeyField {
                    offset: 0,
                    size: 4,
                    type_hint: KeyTypeHint::Int32,
                },
                KeyField {
                    offset: 8,
                    size: 0,
                    type_hint: KeyTypeHint::String,
                },
            ],
        };
        let mut buf = BytesMut::new();
        serialize_key_descriptors(&kd, &mut buf);
        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        let parsed = parse_key_descriptors(&mut slice).unwrap();
        assert_eq!(kd, parsed);
    }

    #[test]
    fn test_key_descriptors_empty() {
        let kd = KeyDescriptors { keys: vec![] };
        let mut buf = BytesMut::new();
        serialize_key_descriptors(&kd, &mut buf);
        let bytes = buf.freeze();
        let mut slice: &[u8] = &bytes;
        let parsed = parse_key_descriptors(&mut slice).unwrap();
        assert_eq!(kd, parsed);
    }

    #[test]
    fn test_build_message() {
        let payload = serialize_error(&ErrorPayload {
            error_code: ErrorCode::InvalidMessage,
            error_message: "bad".into(),
        });
        let msg = build_message(MsgType::Error, 5, &payload);
        let header = parse_header(&msg).unwrap();
        assert_eq!(header.msg_type, MsgType::Error);
        assert_eq!(header.request_id, 5);
        assert_eq!(header.payload_length as usize, payload.len());
        let parsed = parse_error(&msg[HEADER_SIZE..]).unwrap();
        assert_eq!(parsed.error_code, ErrorCode::InvalidMessage);
        assert_eq!(parsed.error_message, "bad");
    }

    #[test]
    fn test_invalid_write_mode() {
        let data = [0x02]; // invalid mode
        assert!(matches!(
            parse_write(&data),
            Err(ProtocolError::InvalidWriteMode(0x02))
        ));
    }

    #[test]
    fn test_trailing_bytes_unsubscribe() {
        let mut bytes = serialize_unsubscribe(&UnsubscribePayload {
            topic_name: "t".into(),
            type_name: "T".into(),
        });
        bytes.put_u8(0xFF); // trailing garbage
        assert!(matches!(
            parse_unsubscribe(&bytes),
            Err(ProtocolError::TrailingBytes(1))
        ));
    }

    #[test]
    fn test_trailing_bytes_delete_writer() {
        let mut bytes = serialize_delete_writer(&DeleteWriterPayload { writer_id: 1 });
        bytes.put_u8(0xFF);
        assert!(matches!(
            parse_delete_writer(&bytes),
            Err(ProtocolError::TrailingBytes(1))
        ));
    }

    #[test]
    fn test_trailing_bytes_subscribe() {
        let mut bytes = serialize_subscribe(&SubscribePayload {
            topic_name: "t".into(),
            type_name: "T".into(),
            qos: QosSet::default(),
            is_keyed: false,
            key_descriptors: KeyDescriptors { keys: vec![] },
        });
        bytes.put_u8(0xFF);
        assert!(matches!(
            parse_subscribe(&bytes),
            Err(ProtocolError::TrailingBytes(1))
        ));
    }

    #[test]
    fn test_trailing_bytes_error() {
        let mut bytes = serialize_error(&ErrorPayload {
            error_code: ErrorCode::DdsError,
            error_message: "oops".into(),
        });
        bytes.put_u8(0xFF);
        assert!(matches!(
            parse_error(&bytes),
            Err(ProtocolError::TrailingBytes(1))
        ));
    }
}
