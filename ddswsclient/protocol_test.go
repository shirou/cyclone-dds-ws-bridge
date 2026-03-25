package ddswsclient

import (
	"bytes"
	"encoding/binary"
	"os"
	"path/filepath"
	"runtime"
	"testing"
)

func fixturesDir() string {
	_, file, _, _ := runtime.Caller(0)
	return filepath.Join(filepath.Dir(file), "..", "tests", "fixtures")
}

// --- Header round-trip ---

func TestHeaderRoundTrip(t *testing.T) {
	h := Header{
		Version:       Version,
		Type:          MsgSubscribe,
		RequestID:     42,
		PayloadLength: 100,
	}
	buf := EncodeHeader(h)
	got, err := DecodeHeader(buf[:])
	if err != nil {
		t.Fatal(err)
	}
	if got != h {
		t.Fatalf("got %+v, want %+v", got, h)
	}
}

func TestHeaderBadMagic(t *testing.T) {
	buf := EncodeHeader(Header{Version: Version, Type: MsgPing, RequestID: 1})
	buf[0] = 0xFF
	_, err := DecodeHeader(buf[:])
	if err == nil {
		t.Fatal("expected error for bad magic")
	}
}

func TestHeaderUnsupportedVersion(t *testing.T) {
	buf := EncodeHeader(Header{Version: Version, Type: MsgPing, RequestID: 1})
	buf[2] = Version + 1
	_, err := DecodeHeader(buf[:])
	if err == nil {
		t.Fatal("expected error for unsupported version")
	}
}

func TestHeaderTruncated(t *testing.T) {
	_, err := DecodeHeader([]byte{0x44, 0x42, 0x01})
	if err == nil {
		t.Fatal("expected error for truncated header")
	}
}

// --- String round-trip ---

func TestStringRoundTrip(t *testing.T) {
	for _, s := range []string{"", "hello", "sensor::SensorData"} {
		buf := appendString(nil, s)
		got, n, err := decodeString(buf)
		if err != nil {
			t.Fatalf("decodeString(%q): %v", s, err)
		}
		if got != s {
			t.Fatalf("got %q, want %q", got, s)
		}
		if n != len(buf) {
			t.Fatalf("consumed %d, want %d", n, len(buf))
		}
	}
}

// --- Subscribe round-trip ---

func TestSubscribeRoundTrip(t *testing.T) {
	p := &SubscribePayload{
		TopicName: "test_topic",
		TypeName:  "TestType",
		Qos:       nil,
		Keys:      nil,
	}
	data := EncodeSubscribe(p)
	got, err := DecodeSubscribe(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.TopicName != p.TopicName || got.TypeName != p.TypeName {
		t.Fatalf("got %+v, want %+v", got, p)
	}
}

// --- Unsubscribe round-trip ---

func TestUnsubscribeRoundTrip(t *testing.T) {
	p := &UnsubscribePayload{TopicName: "t", TypeName: "T"}
	data := EncodeUnsubscribe(p)
	got, err := DecodeUnsubscribe(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.TopicName != p.TopicName || got.TypeName != p.TypeName {
		t.Fatalf("got %+v", got)
	}
}

// --- Write round-trip (topic mode) ---

func TestWriteTopicModeRoundTrip(t *testing.T) {
	p := &WritePayload{
		Mode:      WriteTopicMode,
		TopicName: "t",
		TypeName:  "T",
		Data:      []byte{1, 2, 3, 4},
	}
	data := EncodeWrite(p)
	got, err := DecodeWrite(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.Mode != WriteTopicMode || got.TopicName != "t" || !bytes.Equal(got.Data, p.Data) {
		t.Fatalf("got %+v", got)
	}
}

// --- Write round-trip (writer-id mode) ---

func TestWriteWriterIDModeRoundTrip(t *testing.T) {
	p := &WritePayload{
		Mode:     WriteWriterMode,
		WriterID: 42,
		Data:     []byte{0xDE, 0xAD},
	}
	data := EncodeWrite(p)
	got, err := DecodeWrite(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.Mode != WriteWriterMode || got.WriterID != 42 || !bytes.Equal(got.Data, p.Data) {
		t.Fatalf("got %+v", got)
	}
}

// --- Dispose round-trip ---

func TestDisposeRoundTrip(t *testing.T) {
	p := &WritePayload{Mode: WriteWriterMode, WriterID: 7, Data: []byte{0xFF}}
	data := EncodeDispose(p)
	got, err := DecodeDispose(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.WriterID != 7 || !bytes.Equal(got.Data, p.Data) {
		t.Fatalf("got %+v", got)
	}
}

// --- CreateWriter round-trip ---

func TestCreateWriterRoundTrip(t *testing.T) {
	p := &CreateWriterPayload{
		TopicName: "my_topic",
		TypeName:  "MyType",
		Keys:      []KeyField{{Offset: 4, Size: 8, TypeHint: KeyInt64}},
	}
	data := EncodeCreateWriter(p)
	got, err := DecodeCreateWriter(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.TopicName != "my_topic" || len(got.Keys) != 1 || got.Keys[0].TypeHint != KeyInt64 {
		t.Fatalf("got %+v", got)
	}
}

// --- DeleteWriter round-trip ---

func TestDeleteWriterRoundTrip(t *testing.T) {
	p := &DeleteWriterPayload{WriterID: 123}
	data := EncodeDeleteWriter(p)
	got, err := DecodeDeleteWriter(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.WriterID != 123 {
		t.Fatalf("got %d", got.WriterID)
	}
}

// --- Error round-trip ---

func TestErrorRoundTrip(t *testing.T) {
	p := &ErrorPayload{Code: ErrDDSError, Message: "something went wrong"}
	data := EncodeError(p)
	got, err := DecodeError(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.Code != p.Code || got.Message != p.Message {
		t.Fatalf("got %+v", got)
	}
}

// --- Data round-trip ---

func TestDataRoundTrip(t *testing.T) {
	p := &DataPayload{
		TopicName:       "sensor",
		SourceTimestamp: 1234567890_000_000_000,
		Data:            []byte{10, 20, 30},
	}
	data := EncodeData(p)
	got, err := DecodeData(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.TopicName != p.TopicName || got.SourceTimestamp != p.SourceTimestamp || !bytes.Equal(got.Data, p.Data) {
		t.Fatalf("got %+v", got)
	}
}

// --- DataDisposed round-trip ---

func TestDataDisposedRoundTrip(t *testing.T) {
	p := &DataDisposedPayload{
		TopicName:       "sensor",
		SourceTimestamp: 999,
		KeyData:         []byte{1, 2},
	}
	data := EncodeDataDisposed(p)
	got, err := DecodeDataDisposed(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.TopicName != "sensor" || got.SourceTimestamp != 999 || !bytes.Equal(got.KeyData, p.KeyData) {
		t.Fatalf("got %+v", got)
	}
}

// --- DataNoWriters round-trip ---

func TestDataNoWritersRoundTrip(t *testing.T) {
	p := &DataNoWritersPayload{TopicName: "sensor", KeyData: []byte{5, 6, 7}}
	data := EncodeDataNoWriters(p)
	got, err := DecodeDataNoWriters(data)
	if err != nil {
		t.Fatal(err)
	}
	if got.TopicName != "sensor" || !bytes.Equal(got.KeyData, p.KeyData) {
		t.Fatalf("got %+v", got)
	}
}

// --- KeyDescriptors round-trip ---

func TestKeyDescriptorsRoundTrip(t *testing.T) {
	keys := []KeyField{
		{Offset: 0, Size: 4, TypeHint: KeyInt32},
		{Offset: 8, Size: 0, TypeHint: KeyString},
	}
	data := encodeKeyDescriptors(keys)
	got, n, err := decodeKeyDescriptors(data)
	if err != nil {
		t.Fatal(err)
	}
	if n != len(data) {
		t.Fatalf("consumed %d, want %d", n, len(data))
	}
	if len(got) != 2 || got[0].TypeHint != KeyInt32 || got[1].TypeHint != KeyString {
		t.Fatalf("got %+v", got)
	}
}

func TestKeyDescriptorsEmpty(t *testing.T) {
	data := encodeKeyDescriptors(nil)
	got, n, err := decodeKeyDescriptors(data)
	if err != nil {
		t.Fatal(err)
	}
	if n != 1 || len(got) != 0 {
		t.Fatalf("got %+v, n=%d", got, n)
	}
}

// --- QoS round-trip ---

func TestQosRoundTrip(t *testing.T) {
	policies := []QosPolicy{
		QosReliabilityPolicy(ReliabilityReliable, 100),
		QosDurabilityPolicy(DurabilityTransientLocal),
		QosHistoryPolicy(HistoryKeepLast, 10),
	}
	data := encodeQos(policies)
	got, n, err := decodeQos(data)
	if err != nil {
		t.Fatal(err)
	}
	if n != len(data) {
		t.Fatalf("consumed %d, want %d", n, len(data))
	}
	if len(got) != 3 {
		t.Fatalf("got %d policies", len(got))
	}
	for i, p := range got {
		if p.ID != policies[i].ID || !bytes.Equal(p.Data, policies[i].Data) {
			t.Fatalf("policy %d: got %+v, want %+v", i, p, policies[i])
		}
	}
}

func TestQosEmpty(t *testing.T) {
	data := encodeQos(nil)
	got, n, err := decodeQos(data)
	if err != nil {
		t.Fatal(err)
	}
	if n != 1 || len(got) != 0 {
		t.Fatalf("got %+v, n=%d", got, n)
	}
}

// --- BuildMessage ---

func TestBuildMessage(t *testing.T) {
	payload := EncodeError(&ErrorPayload{Code: ErrInvalidMessage, Message: "bad"})
	msg := BuildMessage(MsgError, 5, payload)
	hdr, err := DecodeHeader(msg)
	if err != nil {
		t.Fatal(err)
	}
	if hdr.Type != MsgError || hdr.RequestID != 5 || hdr.PayloadLength != uint32(len(payload)) {
		t.Fatalf("got %+v", hdr)
	}
	ep, err := DecodeError(msg[HeaderLen:])
	if err != nil {
		t.Fatal(err)
	}
	if ep.Code != ErrInvalidMessage || ep.Message != "bad" {
		t.Fatalf("got %+v", ep)
	}
}

// --- Invalid write mode ---

func TestInvalidWriteMode(t *testing.T) {
	_, err := DecodeWrite([]byte{0x02})
	if err == nil {
		t.Fatal("expected error for invalid write mode")
	}
}

// --- Trailing bytes ---

func TestTrailingBytesUnsubscribe(t *testing.T) {
	data := EncodeUnsubscribe(&UnsubscribePayload{TopicName: "t", TypeName: "T"})
	data = append(data, 0xFF)
	_, err := DecodeUnsubscribe(data)
	if err == nil {
		t.Fatal("expected error for trailing bytes")
	}
}

func TestTrailingBytesDeleteWriter(t *testing.T) {
	data := EncodeDeleteWriter(&DeleteWriterPayload{WriterID: 1})
	data = append(data, 0xFF)
	_, err := DecodeDeleteWriter(data)
	if err == nil {
		t.Fatal("expected error for trailing bytes")
	}
}

// --- Fixture conformance tests ---

func readFixture(t *testing.T, name string) []byte {
	t.Helper()
	path := filepath.Join(fixturesDir(), name+".bin")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("failed to read fixture %s: %v\nRun: cargo test -- --ignored generate_fixtures", name, err)
	}
	return data
}

func TestFixtureSubscribe(t *testing.T) {
	data := readFixture(t, "subscribe")

	// Verify Go-generated bytes match
	p := &SubscribePayload{
		TopicName: "SensorData",
		TypeName:  "sensor::SensorData",
		Qos: []QosPolicy{
			QosReliabilityPolicy(ReliabilityReliable, 100),
			QosDurabilityPolicy(DurabilityTransientLocal),
		},
		IsKeyed: true,
		Keys:    []KeyField{{Offset: 4, Size: 4, TypeHint: KeyInt32}},
	}
	goMsg := BuildMessage(MsgSubscribe, 1, EncodeSubscribe(p))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("subscribe: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}

	// Verify parse
	hdr, err := DecodeHeader(data)
	if err != nil {
		t.Fatal(err)
	}
	if hdr.Type != MsgSubscribe || hdr.RequestID != 1 {
		t.Fatalf("header: %+v", hdr)
	}
	sp, err := DecodeSubscribe(data[HeaderLen:])
	if err != nil {
		t.Fatal(err)
	}
	if sp.TopicName != "SensorData" || sp.TypeName != "sensor::SensorData" {
		t.Fatalf("payload: %+v", sp)
	}
	if len(sp.Qos) != 2 || len(sp.Keys) != 1 {
		t.Fatalf("qos=%d keys=%d", len(sp.Qos), len(sp.Keys))
	}
}

func TestFixtureUnsubscribe(t *testing.T) {
	data := readFixture(t, "unsubscribe")
	goMsg := BuildMessage(MsgUnsubscribe, 2, EncodeUnsubscribe(&UnsubscribePayload{
		TopicName: "SensorData",
		TypeName:  "sensor::SensorData",
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("unsubscribe: Go output differs from fixture")
	}
}

func TestFixtureWriteTopicMode(t *testing.T) {
	data := readFixture(t, "write_topic_mode")
	goMsg := BuildMessage(MsgWrite, 3, EncodeWrite(&WritePayload{
		Mode:      WriteTopicMode,
		TopicName: "SensorData",
		TypeName:  "sensor::SensorData",
		Data:      []byte{0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00},
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("write_topic_mode: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}
}

func TestFixtureWriteWriterIDMode(t *testing.T) {
	data := readFixture(t, "write_writer_id_mode")
	goMsg := BuildMessage(MsgWrite, 4, EncodeWrite(&WritePayload{
		Mode:     WriteWriterMode,
		WriterID: 42,
		Data:     []byte{0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00},
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("write_writer_id_mode: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}
}

func TestFixtureDispose(t *testing.T) {
	data := readFixture(t, "dispose")
	goMsg := BuildMessage(MsgDispose, 5, EncodeDispose(&WritePayload{
		Mode:     WriteWriterMode,
		WriterID: 42,
		Data:     []byte{0x2A, 0x00, 0x00, 0x00},
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("dispose: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}
}

func TestFixtureCreateWriter(t *testing.T) {
	data := readFixture(t, "create_writer")
	goMsg := BuildMessage(MsgCreateWriter, 6, EncodeCreateWriter(&CreateWriterPayload{
		TopicName: "SensorData",
		TypeName:  "sensor::SensorData",
		Qos:       []QosPolicy{QosReliabilityPolicy(ReliabilityReliable, 100)},
		IsKeyed:   true,
		Keys:      []KeyField{{Offset: 4, Size: 4, TypeHint: KeyInt32}},
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("create_writer: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}
}

func TestFixtureDeleteWriter(t *testing.T) {
	data := readFixture(t, "delete_writer")
	goMsg := BuildMessage(MsgDeleteWriter, 7, EncodeDeleteWriter(&DeleteWriterPayload{WriterID: 42}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("delete_writer: Go output differs from fixture")
	}
}

func TestFixturePing(t *testing.T) {
	data := readFixture(t, "ping")
	goMsg := BuildMessage(MsgPing, 8, nil)
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("ping: Go output differs from fixture")
	}
}

func TestFixtureOKEmpty(t *testing.T) {
	data := readFixture(t, "ok_empty")
	goMsg := BuildMessage(MsgOK, 6, nil)
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("ok_empty: Go output differs from fixture")
	}
}

func TestFixtureOKWriterID(t *testing.T) {
	data := readFixture(t, "ok_writer_id")
	var payload [4]byte
	binary.LittleEndian.PutUint32(payload[:], 42)
	goMsg := BuildMessage(MsgOK, 6, payload[:])
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("ok_writer_id: Go output differs from fixture")
	}
}

func TestFixtureData(t *testing.T) {
	data := readFixture(t, "data")
	goMsg := BuildMessage(MsgData, 0, EncodeData(&DataPayload{
		TopicName:       "SensorData",
		SourceTimestamp: 1700000000_000_000_000,
		Data:            []byte{0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00},
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("data: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}
}

func TestFixtureDataDisposed(t *testing.T) {
	data := readFixture(t, "data_disposed")
	goMsg := BuildMessage(MsgDataDisposed, 0, EncodeDataDisposed(&DataDisposedPayload{
		TopicName:       "SensorData",
		SourceTimestamp: 1700000000_000_000_000,
		KeyData:         []byte{0x2A, 0x00, 0x00, 0x00},
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("data_disposed: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}
}

func TestFixtureDataNoWriters(t *testing.T) {
	data := readFixture(t, "data_no_writers")
	goMsg := BuildMessage(MsgDataNoWriters, 0, EncodeDataNoWriters(&DataNoWritersPayload{
		TopicName: "SensorData",
		KeyData:   []byte{0x2A, 0x00, 0x00, 0x00},
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("data_no_writers: Go output differs from fixture")
	}
}

func TestFixtureError(t *testing.T) {
	data := readFixture(t, "error")
	goMsg := BuildMessage(MsgError, 1, EncodeError(&ErrorPayload{
		Code:    ErrInvalidMessage,
		Message: "unsupported protocol version 3; bridge supports up to version 1",
	}))
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("error: Go output differs from fixture\nGo:  %x\nBin: %x", goMsg, data)
	}
}

func TestFixturePong(t *testing.T) {
	data := readFixture(t, "pong")
	goMsg := BuildMessage(MsgPong, 8, nil)
	if !bytes.Equal(goMsg, data) {
		t.Fatalf("pong: Go output differs from fixture")
	}
}

// --- All QoS policy constructors round-trip ---

func TestAllQosPoliciesRoundTrip(t *testing.T) {
	policies := []QosPolicy{
		QosReliabilityPolicy(ReliabilityBestEffort, 200),
		QosDurabilityPolicy(DurabilityTransient),
		QosHistoryPolicy(HistoryKeepAll, 0),
		QosDeadlinePolicy(500),
		QosLatencyBudgetPolicy(50),
		QosOwnershipStrengthPolicy(42),
		QosLivelinessPolicy(LivelinessManualByTopic, 1000),
		QosDestinationOrderPolicy(DestinationOrderBySourceTimestamp),
		QosPresentationPolicy(PresentationTopic, 1, 0),
		QosPartitionPolicy([]string{"part_a", "part_b"}),
		QosOwnershipPolicy(OwnershipExclusive),
		QosWriterDataLifecyclePolicy(0),
		QosTimeBasedFilterPolicy(100),
	}
	data := encodeQos(policies)
	got, n, err := decodeQos(data)
	if err != nil {
		t.Fatal(err)
	}
	if n != len(data) {
		t.Fatalf("consumed %d, want %d", n, len(data))
	}
	if len(got) != len(policies) {
		t.Fatalf("got %d policies, want %d", len(got), len(policies))
	}
	for i, p := range got {
		if p.ID != policies[i].ID {
			t.Fatalf("policy %d: id %x, want %x", i, p.ID, policies[i].ID)
		}
		if !bytes.Equal(p.Data, policies[i].Data) {
			t.Fatalf("policy %d: data mismatch", i)
		}
	}
}
