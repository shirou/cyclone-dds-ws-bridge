// Package ddswsclient implements a Go client for the cyclone-dds-ws-bridge.
//
// The client communicates with the bridge over WebSocket using a compact binary
// protocol. All payloads (CDR/XCDR2 encoded DDS samples) are treated as opaque
// byte slices -- the caller is responsible for serialization/deserialization
// (e.g. via go-dds-idlgen generated types).
package ddswsclient

import (
	"encoding/binary"
	"errors"
	"fmt"
)

// Header constants
const (
	Magic0    byte = 0x44 // 'D'
	Magic1    byte = 0x42 // 'B'
	Version   byte = 0x01
	HeaderLen      = 12

	DefaultMaxPayloadSize = 4 * 1024 * 1024 // 4 MiB
)

// MsgType identifies the type of a protocol message.
type MsgType byte

const (
	// Client -> Bridge
	MsgSubscribe    MsgType = 0x01
	MsgUnsubscribe  MsgType = 0x02
	MsgWrite        MsgType = 0x03
	MsgDispose      MsgType = 0x04
	MsgWriteDispose MsgType = 0x05
	MsgCreateWriter MsgType = 0x10
	MsgDeleteWriter MsgType = 0x11
	MsgPing         MsgType = 0x0F

	// Bridge -> Client
	MsgOK            MsgType = 0x80
	MsgData          MsgType = 0xC0
	MsgDataDisposed  MsgType = 0xC1
	MsgDataNoWriters MsgType = 0xC2
	MsgError         MsgType = 0xFE
	MsgPong          MsgType = 0x8F
)

// ErrorCode identifies the type of error in an ERROR response.
type ErrorCode uint16

const (
	ErrInvalidMessage ErrorCode = 0x0001
	ErrTopicNotFound  ErrorCode = 0x0002
	ErrDDSError       ErrorCode = 0x0003
	ErrWriterNotFound ErrorCode = 0x0004
	ErrReaderNotFound ErrorCode = 0x0005
	ErrInvalidQoS     ErrorCode = 0x0006
	ErrAlreadyExists  ErrorCode = 0x0007
	ErrInternalError  ErrorCode = 0x0008
	ErrBufferOverflow ErrorCode = 0x0009
)

// KeyTypeHint indicates how to compare a key field.
type KeyTypeHint byte

const (
	KeyOpaque KeyTypeHint = 0x00
	KeyUUID   KeyTypeHint = 0x01
	KeyInt32  KeyTypeHint = 0x02
	KeyInt64  KeyTypeHint = 0x03
	KeyString KeyTypeHint = 0x04
)

// Header is the 12-byte fixed header for every protocol message.
type Header struct {
	Version       byte
	Type          MsgType
	RequestID     uint32
	PayloadLength uint32
}

// KeyField describes one key field in a DDS keyed topic.
type KeyField struct {
	Offset   uint32
	Size     uint32
	TypeHint KeyTypeHint
}

// QosPolicyID identifies a QoS policy type.
type QosPolicyID byte

const (
	QosReliability         QosPolicyID = 0x01
	QosDurability          QosPolicyID = 0x02
	QosHistory             QosPolicyID = 0x03
	QosDeadline            QosPolicyID = 0x04
	QosLatencyBudget       QosPolicyID = 0x05
	QosOwnershipStrength   QosPolicyID = 0x06
	QosLiveliness          QosPolicyID = 0x07
	QosDestinationOrder    QosPolicyID = 0x08
	QosPresentation        QosPolicyID = 0x09
	QosPartition           QosPolicyID = 0x0A
	QosOwnership           QosPolicyID = 0x0B
	QosWriterDataLifecycle QosPolicyID = 0x0C
	QosTimeBasedFilter     QosPolicyID = 0x0D
)

// QosPolicy represents a single DDS QoS policy in binary form.
type QosPolicy struct {
	ID   QosPolicyID
	Data []byte // raw value bytes (excluding the policy_id byte)
}

// Reliability kind
const (
	ReliabilityBestEffort byte = 0
	ReliabilityReliable   byte = 1
)

// Durability kind
const (
	DurabilityVolatile       byte = 0
	DurabilityTransientLocal byte = 1
	DurabilityTransient      byte = 2
	DurabilityPersistent     byte = 3
)

// History kind
const (
	HistoryKeepLast byte = 0
	HistoryKeepAll  byte = 1
)

// Liveliness kind
const (
	LivelinessAutomatic            byte = 0
	LivelinessManualByParticipant  byte = 1
	LivelinessManualByTopic        byte = 2
)

// DestinationOrder kind
const (
	DestinationOrderByReceptionTimestamp byte = 0
	DestinationOrderBySourceTimestamp    byte = 1
)

// Presentation access scope
const (
	PresentationInstance byte = 0
	PresentationTopic    byte = 1
	PresentationGroup    byte = 2
)

// Ownership kind
const (
	OwnershipShared    byte = 0
	OwnershipExclusive byte = 1
)

// QoS helper constructors

func QosReliabilityPolicy(kind byte, maxBlockingTimeMs uint32) QosPolicy {
	data := make([]byte, 5)
	data[0] = kind
	binary.LittleEndian.PutUint32(data[1:], maxBlockingTimeMs)
	return QosPolicy{ID: QosReliability, Data: data}
}

func QosDurabilityPolicy(kind byte) QosPolicy {
	return QosPolicy{ID: QosDurability, Data: []byte{kind}}
}

func QosHistoryPolicy(kind byte, depth uint32) QosPolicy {
	data := make([]byte, 5)
	data[0] = kind
	binary.LittleEndian.PutUint32(data[1:], depth)
	return QosPolicy{ID: QosHistory, Data: data}
}

func QosDeadlinePolicy(periodMs uint32) QosPolicy {
	data := make([]byte, 4)
	binary.LittleEndian.PutUint32(data, periodMs)
	return QosPolicy{ID: QosDeadline, Data: data}
}

func QosLatencyBudgetPolicy(durationMs uint32) QosPolicy {
	data := make([]byte, 4)
	binary.LittleEndian.PutUint32(data, durationMs)
	return QosPolicy{ID: QosLatencyBudget, Data: data}
}

func QosOwnershipStrengthPolicy(value uint32) QosPolicy {
	data := make([]byte, 4)
	binary.LittleEndian.PutUint32(data, value)
	return QosPolicy{ID: QosOwnershipStrength, Data: data}
}

func QosLivelinessPolicy(kind byte, leaseDurationMs uint32) QosPolicy {
	data := make([]byte, 5)
	data[0] = kind
	binary.LittleEndian.PutUint32(data[1:], leaseDurationMs)
	return QosPolicy{ID: QosLiveliness, Data: data}
}

func QosDestinationOrderPolicy(kind byte) QosPolicy {
	return QosPolicy{ID: QosDestinationOrder, Data: []byte{kind}}
}

func QosPresentationPolicy(accessScope, coherentAccess, orderedAccess byte) QosPolicy {
	return QosPolicy{ID: QosPresentation, Data: []byte{accessScope, coherentAccess, orderedAccess}}
}

func QosPartitionPolicy(partitions []string) QosPolicy {
	size := 1 // partition_count
	for _, p := range partitions {
		size += 4 + len(p)
	}
	data := make([]byte, 0, size)
	data = append(data, byte(len(partitions)))
	for _, p := range partitions {
		data = appendString(data, p)
	}
	return QosPolicy{ID: QosPartition, Data: data}
}

func QosOwnershipPolicy(kind byte) QosPolicy {
	return QosPolicy{ID: QosOwnership, Data: []byte{kind}}
}

func QosWriterDataLifecyclePolicy(autodispose byte) QosPolicy {
	return QosPolicy{ID: QosWriterDataLifecycle, Data: []byte{autodispose}}
}

func QosTimeBasedFilterPolicy(minSeparationMs uint32) QosPolicy {
	data := make([]byte, 4)
	binary.LittleEndian.PutUint32(data, minSeparationMs)
	return QosPolicy{ID: QosTimeBasedFilter, Data: data}
}

// --- Encoding ---

// EncodeHeader serializes a Header into a 12-byte slice.
func EncodeHeader(h Header) [HeaderLen]byte {
	var buf [HeaderLen]byte
	buf[0] = Magic0
	buf[1] = Magic1
	buf[2] = h.Version
	buf[3] = byte(h.Type)
	binary.LittleEndian.PutUint32(buf[4:], h.RequestID)
	binary.LittleEndian.PutUint32(buf[8:], h.PayloadLength)
	return buf
}

// DecodeHeader parses a 12-byte header from data.
func DecodeHeader(data []byte) (Header, error) {
	if len(data) < HeaderLen {
		return Header{}, fmt.Errorf("header too short: %d bytes", len(data))
	}
	if data[0] != Magic0 || data[1] != Magic1 {
		return Header{}, fmt.Errorf("bad magic: [%#02x, %#02x]", data[0], data[1])
	}
	v := data[2]
	if v > Version {
		return Header{}, fmt.Errorf("unsupported protocol version %d; max supported is %d", v, Version)
	}
	return Header{
		Version:       v,
		Type:          MsgType(data[3]),
		RequestID:     binary.LittleEndian.Uint32(data[4:]),
		PayloadLength: binary.LittleEndian.Uint32(data[8:]),
	}, nil
}

// BuildMessage constructs a complete message (header + payload).
func BuildMessage(msgType MsgType, requestID uint32, payload []byte) []byte {
	h := Header{
		Version:       Version,
		Type:          msgType,
		RequestID:     requestID,
		PayloadLength: uint32(len(payload)),
	}
	hdr := EncodeHeader(h)
	msg := make([]byte, HeaderLen+len(payload))
	copy(msg, hdr[:])
	copy(msg[HeaderLen:], payload)
	return msg
}

// --- String encoding: 4-byte LE length + UTF-8 ---

func appendString(dst []byte, s string) []byte {
	var buf [4]byte
	binary.LittleEndian.PutUint32(buf[:], uint32(len(s)))
	dst = append(dst, buf[:]...)
	dst = append(dst, s...)
	return dst
}

func decodeString(data []byte) (s string, n int, err error) {
	if len(data) < 4 {
		return "", 0, errors.New("string: truncated length prefix")
	}
	length := binary.LittleEndian.Uint32(data)
	if length > DefaultMaxPayloadSize {
		return "", 0, fmt.Errorf("string too long: %d bytes", length)
	}
	total := 4 + int(length)
	if len(data) < total {
		return "", 0, fmt.Errorf("string: truncated body: need %d, have %d", total, len(data))
	}
	return string(data[4:total]), total, nil
}

// --- Key Descriptors ---

func encodeKeyDescriptors(keys []KeyField) []byte {
	buf := make([]byte, 0, 1+len(keys)*9)
	buf = append(buf, byte(len(keys)))
	for _, k := range keys {
		var tmp [9]byte
		binary.LittleEndian.PutUint32(tmp[0:], k.Offset)
		binary.LittleEndian.PutUint32(tmp[4:], k.Size)
		tmp[8] = byte(k.TypeHint)
		buf = append(buf, tmp[:]...)
	}
	return buf
}

func decodeKeyDescriptors(data []byte) (keys []KeyField, n int, err error) {
	if len(data) < 1 {
		return nil, 0, errors.New("key descriptors: truncated count")
	}
	count := int(data[0])
	pos := 1
	keys = make([]KeyField, count)
	for i := range count {
		if pos+9 > len(data) {
			return nil, 0, errors.New("key descriptors: truncated key field")
		}
		keys[i] = KeyField{
			Offset:   binary.LittleEndian.Uint32(data[pos:]),
			Size:     binary.LittleEndian.Uint32(data[pos+4:]),
			TypeHint: KeyTypeHint(data[pos+8]),
		}
		pos += 9
	}
	return keys, pos, nil
}

// --- QoS ---

func encodeQos(policies []QosPolicy) []byte {
	buf := []byte{byte(len(policies))}
	for _, p := range policies {
		buf = append(buf, byte(p.ID))
		buf = append(buf, p.Data...)
	}
	return buf
}

func decodeQos(data []byte) (policies []QosPolicy, n int, err error) {
	if len(data) < 1 {
		return nil, 0, errors.New("qos: truncated count")
	}
	count := int(data[0])
	pos := 1
	policies = make([]QosPolicy, 0, count)
	for range count {
		if pos >= len(data) {
			return nil, 0, errors.New("qos: truncated policy_id")
		}
		id := QosPolicyID(data[pos])
		pos++
		size, err := qosPolicyDataSize(id, data[pos:])
		if err != nil {
			return nil, 0, err
		}
		if pos+size > len(data) {
			return nil, 0, fmt.Errorf("qos: truncated policy 0x%02x data", id)
		}
		policyData := make([]byte, size)
		copy(policyData, data[pos:pos+size])
		policies = append(policies, QosPolicy{ID: id, Data: policyData})
		pos += size
	}
	return policies, pos, nil
}

func qosPolicyDataSize(id QosPolicyID, data []byte) (int, error) {
	switch id {
	case QosReliability:
		return 5, nil // kind(1) + max_blocking_time_ms(4)
	case QosDurability:
		return 1, nil
	case QosHistory:
		return 5, nil // kind(1) + depth(4)
	case QosDeadline:
		return 4, nil
	case QosLatencyBudget:
		return 4, nil
	case QosOwnershipStrength:
		return 4, nil
	case QosLiveliness:
		return 5, nil // kind(1) + lease_duration_ms(4)
	case QosDestinationOrder:
		return 1, nil
	case QosPresentation:
		return 3, nil // access_scope(1) + coherent(1) + ordered(1)
	case QosPartition:
		return partitionDataSize(data)
	case QosOwnership:
		return 1, nil
	case QosWriterDataLifecycle:
		return 1, nil
	case QosTimeBasedFilter:
		return 4, nil
	default:
		return 0, fmt.Errorf("unknown qos policy id: 0x%02x", id)
	}
}

func partitionDataSize(data []byte) (int, error) {
	if len(data) < 1 {
		return 0, errors.New("partition: truncated count")
	}
	count := int(data[0])
	pos := 1
	for range count {
		if pos+4 > len(data) {
			return 0, errors.New("partition: truncated string length")
		}
		slen := int(binary.LittleEndian.Uint32(data[pos:]))
		pos += 4 + slen
		if pos > len(data) {
			return 0, errors.New("partition: truncated string data")
		}
	}
	return pos, nil
}

// --- Payload types ---

// SubscribePayload is the SUBSCRIBE message payload.
type SubscribePayload struct {
	TopicName string
	TypeName  string
	Qos       []QosPolicy
	Keys      []KeyField
}

// UnsubscribePayload is the UNSUBSCRIBE message payload.
type UnsubscribePayload struct {
	TopicName string
	TypeName  string
}

// CreateWriterPayload is the CREATE_WRITER message payload.
type CreateWriterPayload struct {
	TopicName string
	TypeName  string
	Qos       []QosPolicy
	Keys      []KeyField
}

// DeleteWriterPayload is the DELETE_WRITER message payload.
type DeleteWriterPayload struct {
	WriterID uint32
}

// WriteMode indicates topic-name mode (0x00) or writer-id mode (0x01).
type WriteMode byte

const (
	WriteTopicMode  WriteMode = 0x00
	WriteWriterMode WriteMode = 0x01
)

// WritePayload represents WRITE, DISPOSE, or WRITE_DISPOSE payloads.
type WritePayload struct {
	Mode WriteMode
	// TopicName mode fields
	TopicName string
	TypeName  string
	Qos       []QosPolicy
	Keys      []KeyField
	// WriterID mode field
	WriterID uint32
	// Data/KeyData (remaining bytes)
	Data []byte
}

// DataPayload is the DATA message payload (bridge -> client).
type DataPayload struct {
	TopicName       string
	SourceTimestamp uint64
	Data            []byte
}

// DataDisposedPayload is the DATA_DISPOSED message payload.
type DataDisposedPayload struct {
	TopicName       string
	SourceTimestamp uint64
	KeyData         []byte
}

// DataNoWritersPayload is the DATA_NO_WRITERS message payload.
type DataNoWritersPayload struct {
	TopicName string
	KeyData   []byte
}

// ErrorPayload is the ERROR message payload.
type ErrorPayload struct {
	Code    ErrorCode
	Message string
}

// --- Payload encoding ---

func EncodeSubscribe(p *SubscribePayload) []byte {
	var buf []byte
	buf = appendString(buf, p.TopicName)
	buf = appendString(buf, p.TypeName)
	buf = append(buf, encodeQos(p.Qos)...)
	buf = append(buf, encodeKeyDescriptors(p.Keys)...)
	return buf
}

func EncodeUnsubscribe(p *UnsubscribePayload) []byte {
	var buf []byte
	buf = appendString(buf, p.TopicName)
	buf = appendString(buf, p.TypeName)
	return buf
}

func EncodeCreateWriter(p *CreateWriterPayload) []byte {
	var buf []byte
	buf = appendString(buf, p.TopicName)
	buf = appendString(buf, p.TypeName)
	buf = append(buf, encodeQos(p.Qos)...)
	buf = append(buf, encodeKeyDescriptors(p.Keys)...)
	return buf
}

func EncodeDeleteWriter(p *DeleteWriterPayload) []byte {
	var buf [4]byte
	binary.LittleEndian.PutUint32(buf[:], p.WriterID)
	return buf[:]
}

func EncodeWrite(p *WritePayload) []byte {
	return encodeWriteMode(p)
}

func EncodeDispose(p *WritePayload) []byte {
	return encodeWriteMode(p)
}

func EncodeWriteDispose(p *WritePayload) []byte {
	return encodeWriteMode(p)
}

func encodeWriteMode(p *WritePayload) []byte {
	var buf []byte
	buf = append(buf, byte(p.Mode))
	switch p.Mode {
	case WriteTopicMode:
		buf = appendString(buf, p.TopicName)
		buf = appendString(buf, p.TypeName)
		buf = append(buf, encodeQos(p.Qos)...)
		buf = append(buf, encodeKeyDescriptors(p.Keys)...)
		buf = append(buf, p.Data...)
	case WriteWriterMode:
		var id [4]byte
		binary.LittleEndian.PutUint32(id[:], p.WriterID)
		buf = append(buf, id[:]...)
		buf = append(buf, p.Data...)
	}
	return buf
}

func EncodeError(p *ErrorPayload) []byte {
	var buf [2]byte
	binary.LittleEndian.PutUint16(buf[:], uint16(p.Code))
	out := append([]byte(nil), buf[:]...)
	out = appendString(out, p.Message)
	return out
}

func EncodeData(p *DataPayload) []byte {
	var buf []byte
	buf = appendString(buf, p.TopicName)
	var ts [8]byte
	binary.LittleEndian.PutUint64(ts[:], p.SourceTimestamp)
	buf = append(buf, ts[:]...)
	buf = append(buf, p.Data...)
	return buf
}

func EncodeDataDisposed(p *DataDisposedPayload) []byte {
	var buf []byte
	buf = appendString(buf, p.TopicName)
	var ts [8]byte
	binary.LittleEndian.PutUint64(ts[:], p.SourceTimestamp)
	buf = append(buf, ts[:]...)
	buf = append(buf, p.KeyData...)
	return buf
}

func EncodeDataNoWriters(p *DataNoWritersPayload) []byte {
	var buf []byte
	buf = appendString(buf, p.TopicName)
	buf = append(buf, p.KeyData...)
	return buf
}

// --- Payload decoding ---

func DecodeSubscribe(data []byte) (*SubscribePayload, error) {
	pos := 0
	topicName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	typeName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	qos, n, err := decodeQos(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	keys, n, err := decodeKeyDescriptors(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	if pos != len(data) {
		return nil, fmt.Errorf("subscribe: %d trailing bytes", len(data)-pos)
	}
	return &SubscribePayload{
		TopicName: topicName,
		TypeName:  typeName,
		Qos:       qos,
		Keys:      keys,
	}, nil
}

func DecodeUnsubscribe(data []byte) (*UnsubscribePayload, error) {
	pos := 0
	topicName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	typeName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	if pos != len(data) {
		return nil, fmt.Errorf("unsubscribe: %d trailing bytes", len(data)-pos)
	}
	return &UnsubscribePayload{
		TopicName: topicName,
		TypeName:  typeName,
	}, nil
}

func DecodeCreateWriter(data []byte) (*CreateWriterPayload, error) {
	pos := 0
	topicName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	typeName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	qos, n, err := decodeQos(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	keys, n, err := decodeKeyDescriptors(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	if pos != len(data) {
		return nil, fmt.Errorf("create_writer: %d trailing bytes", len(data)-pos)
	}
	return &CreateWriterPayload{
		TopicName: topicName,
		TypeName:  typeName,
		Qos:       qos,
		Keys:      keys,
	}, nil
}

func DecodeDeleteWriter(data []byte) (*DeleteWriterPayload, error) {
	if len(data) < 4 {
		return nil, fmt.Errorf("delete_writer: need 4 bytes, got %d", len(data))
	}
	if len(data) > 4 {
		return nil, fmt.Errorf("delete_writer: %d trailing bytes", len(data)-4)
	}
	return &DeleteWriterPayload{
		WriterID: binary.LittleEndian.Uint32(data),
	}, nil
}

func DecodeWrite(data []byte) (*WritePayload, error) {
	return decodeWriteMode(data)
}

func DecodeDispose(data []byte) (*WritePayload, error) {
	return decodeWriteMode(data)
}

func DecodeWriteDispose(data []byte) (*WritePayload, error) {
	return decodeWriteMode(data)
}

func decodeWriteMode(data []byte) (*WritePayload, error) {
	if len(data) < 1 {
		return nil, errors.New("write: empty payload")
	}
	mode := WriteMode(data[0])
	rest := data[1:]
	switch mode {
	case WriteTopicMode:
		pos := 0
		topicName, n, err := decodeString(rest[pos:])
		if err != nil {
			return nil, err
		}
		pos += n
		typeName, n, err := decodeString(rest[pos:])
		if err != nil {
			return nil, err
		}
		pos += n
		qos, n, err := decodeQos(rest[pos:])
		if err != nil {
			return nil, err
		}
		pos += n
		keys, n, err := decodeKeyDescriptors(rest[pos:])
		if err != nil {
			return nil, err
		}
		pos += n
		remaining := make([]byte, len(rest)-pos)
		copy(remaining, rest[pos:])
		return &WritePayload{
			Mode:      WriteTopicMode,
			TopicName: topicName,
			TypeName:  typeName,
			Qos:       qos,
			Keys:      keys,
			Data:      remaining,
		}, nil
	case WriteWriterMode:
		if len(rest) < 4 {
			return nil, fmt.Errorf("write writer-id mode: need 4 bytes for writer_id, got %d", len(rest))
		}
		writerID := binary.LittleEndian.Uint32(rest)
		remaining := make([]byte, len(rest)-4)
		copy(remaining, rest[4:])
		return &WritePayload{
			Mode:     WriteWriterMode,
			WriterID: writerID,
			Data:     remaining,
		}, nil
	default:
		return nil, fmt.Errorf("write: invalid mode 0x%02x", mode)
	}
}

func DecodeError(data []byte) (*ErrorPayload, error) {
	if len(data) < 2 {
		return nil, fmt.Errorf("error: need at least 2 bytes, got %d", len(data))
	}
	code := ErrorCode(binary.LittleEndian.Uint16(data))
	msg, n, err := decodeString(data[2:])
	if err != nil {
		return nil, err
	}
	if 2+n != len(data) {
		return nil, fmt.Errorf("error: %d trailing bytes", len(data)-2-n)
	}
	return &ErrorPayload{Code: code, Message: msg}, nil
}

func DecodeData(data []byte) (*DataPayload, error) {
	pos := 0
	topicName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	if pos+8 > len(data) {
		return nil, fmt.Errorf("data: need 8 bytes for timestamp, got %d", len(data)-pos)
	}
	ts := binary.LittleEndian.Uint64(data[pos:])
	pos += 8
	payload := make([]byte, len(data)-pos)
	copy(payload, data[pos:])
	return &DataPayload{
		TopicName:       topicName,
		SourceTimestamp: ts,
		Data:            payload,
	}, nil
}

func DecodeDataDisposed(data []byte) (*DataDisposedPayload, error) {
	pos := 0
	topicName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	if pos+8 > len(data) {
		return nil, fmt.Errorf("data_disposed: need 8 bytes for timestamp, got %d", len(data)-pos)
	}
	ts := binary.LittleEndian.Uint64(data[pos:])
	pos += 8
	keyData := make([]byte, len(data)-pos)
	copy(keyData, data[pos:])
	return &DataDisposedPayload{
		TopicName:       topicName,
		SourceTimestamp: ts,
		KeyData:         keyData,
	}, nil
}

func DecodeDataNoWriters(data []byte) (*DataNoWritersPayload, error) {
	pos := 0
	topicName, n, err := decodeString(data[pos:])
	if err != nil {
		return nil, err
	}
	pos += n
	keyData := make([]byte, len(data)-pos)
	copy(keyData, data[pos:])
	return &DataNoWritersPayload{
		TopicName: topicName,
		KeyData:   keyData,
	}, nil
}
