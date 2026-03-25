package ddswsclient

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"sync"
	"sync/atomic"

	"github.com/gorilla/websocket"
)

// ErrNotConnected is returned when an operation is attempted on a closed or
// disconnected connection.
var ErrNotConnected = errors.New("not connected")

// InstanceState indicates the state of a received DDS instance.
type InstanceState int

const (
	StateAlive     InstanceState = iota // DATA
	StateDisposed                       // DATA_DISPOSED
	StateNoWriters                      // DATA_NO_WRITERS
)

// DataMessage is a decoded data message from a subscribed topic.
type DataMessage struct {
	TopicName       string
	SourceTimestamp uint64 // nanoseconds since epoch; 0 = unavailable
	Data            []byte // sample data or key data
	State           InstanceState
}

// ErrorHandler is called when an unsolicited error (e.g. BUFFER_OVERFLOW)
// or an error response to an async write is received from the bridge.
// Called from the receive goroutine -- must not block.
type ErrorHandler func(code ErrorCode, message string)

// DataDelivery is called by Conn's recvLoop to deliver data messages.
// Called from the receive goroutine -- must not block.
type DataDelivery func(msg *DataMessage)

// rawMessage is an internal decoded message for request-response correlation.
type rawMessage struct {
	Header  Header
	Payload []byte
}

// Conn is a low-level WebSocket connection to the bridge.
// It handles protocol framing, request-response correlation, and data delivery.
// Use Client for auto-reconnect and subscription/writer management.
type Conn struct {
	ws *websocket.Conn

	writeMu sync.Mutex // protects writes to ws
	nextID  atomic.Uint32
	pending sync.Map // request_id (uint32) -> chan *rawMessage

	onData  DataDelivery
	onError atomic.Pointer[ErrorHandler]

	closed   atomic.Bool
	closeCh  chan struct{}
	recvDone chan struct{}
}

// dial creates a new Conn to the bridge.
func dial(ctx context.Context, addr string, onData DataDelivery) (*Conn, error) {
	dialer := websocket.Dialer{}
	ws, _, err := dialer.DialContext(ctx, addr, nil)
	if err != nil {
		return nil, fmt.Errorf("dial %s: %w", addr, err)
	}
	c := &Conn{
		ws:       ws,
		onData:   onData,
		closeCh:  make(chan struct{}),
		recvDone: make(chan struct{}),
	}
	go c.recvLoop()
	return c, nil
}

func (c *Conn) nextRequestID() uint32 {
	for {
		id := c.nextID.Add(1)
		if id != 0 {
			return id
		}
	}
}

func (c *Conn) send(msgType MsgType, requestID uint32, payload []byte) error {
	if c.closed.Load() {
		return ErrNotConnected
	}
	msg := BuildMessage(msgType, requestID, payload)
	c.writeMu.Lock()
	err := c.ws.WriteMessage(websocket.BinaryMessage, msg)
	c.writeMu.Unlock()
	return err
}

func (c *Conn) sendAndWait(ctx context.Context, msgType MsgType, payload []byte) (*rawMessage, error) {
	if c.closed.Load() {
		return nil, ErrNotConnected
	}

	reqID := c.nextRequestID()
	ch := make(chan *rawMessage, 1)
	c.pending.Store(reqID, ch)

	if err := c.send(msgType, reqID, payload); err != nil {
		c.pending.Delete(reqID)
		return nil, err
	}

	select {
	case <-ctx.Done():
		c.pending.Delete(reqID)
		return nil, ctx.Err()
	case <-c.closeCh:
		c.pending.Delete(reqID)
		return nil, ErrNotConnected
	case resp := <-ch:
		c.pending.Delete(reqID)
		if resp.Header.Type == MsgError {
			ep, parseErr := DecodeError(resp.Payload)
			if parseErr != nil {
				return nil, fmt.Errorf("error response (parse failed): %w", parseErr)
			}
			return nil, fmt.Errorf("bridge error %d: %s", ep.Code, ep.Message)
		}
		return resp, nil
	}
}

func (c *Conn) subscribe(ctx context.Context, topicName, typeName string, qos []QosPolicy, isKeyed bool, keys []KeyField) error {
	payload := EncodeSubscribe(&SubscribePayload{
		TopicName: topicName,
		TypeName:  typeName,
		Qos:       qos,
		IsKeyed:   isKeyed,
		Keys:      keys,
	})
	_, err := c.sendAndWait(ctx, MsgSubscribe, payload)
	return err
}

func (c *Conn) unsubscribe(ctx context.Context, topicName, typeName string) error {
	payload := EncodeUnsubscribe(&UnsubscribePayload{
		TopicName: topicName,
		TypeName:  typeName,
	})
	_, err := c.sendAndWait(ctx, MsgUnsubscribe, payload)
	return err
}

func (c *Conn) createWriter(ctx context.Context, topicName, typeName string, qos []QosPolicy, isKeyed bool, keys []KeyField) (uint32, error) {
	payload := EncodeCreateWriter(&CreateWriterPayload{
		TopicName: topicName,
		TypeName:  typeName,
		Qos:       qos,
		IsKeyed:   isKeyed,
		Keys:      keys,
	})
	resp, err := c.sendAndWait(ctx, MsgCreateWriter, payload)
	if err != nil {
		return 0, err
	}
	if len(resp.Payload) < 4 {
		return 0, fmt.Errorf("create_writer: OK payload too short: %d bytes", len(resp.Payload))
	}
	return binary.LittleEndian.Uint32(resp.Payload), nil
}

func (c *Conn) deleteWriter(ctx context.Context, writerID uint32) error {
	payload := EncodeDeleteWriter(&DeleteWriterPayload{WriterID: writerID})
	_, err := c.sendAndWait(ctx, MsgDeleteWriter, payload)
	return err
}

func (c *Conn) publish(writerID uint32, keyBytes, data []byte) error {
	payload := EncodeWrite(&WritePayload{
		Mode:     WriteWriterMode,
		WriterID: writerID,
		KeyBytes: keyBytes,
		Data:     data,
	})
	return c.send(MsgWrite, c.nextRequestID(), payload)
}

func (c *Conn) publishDispose(writerID uint32, keyBytes, keyData []byte) error {
	payload := EncodeDispose(&WritePayload{
		Mode:     WriteWriterMode,
		WriterID: writerID,
		KeyBytes: keyBytes,
		Data:     keyData,
	})
	return c.send(MsgDispose, c.nextRequestID(), payload)
}

func (c *Conn) dispose(ctx context.Context, topicName, typeName string, qos []QosPolicy, keys []KeyField, keyData []byte) error {
	payload := EncodeDispose(&WritePayload{
		Mode:      WriteTopicMode,
		TopicName: topicName,
		TypeName:  typeName,
		Qos:       qos,
		Keys:      keys,
		Data:      keyData,
	})
	_, err := c.sendAndWait(ctx, MsgDispose, payload)
	return err
}

func (c *Conn) writeDispose(ctx context.Context, topicName, typeName string, qos []QosPolicy, keys []KeyField, data []byte) error {
	payload := EncodeWriteDispose(&WritePayload{
		Mode:      WriteTopicMode,
		TopicName: topicName,
		TypeName:  typeName,
		Qos:       qos,
		Keys:      keys,
		Data:      data,
	})
	_, err := c.sendAndWait(ctx, MsgWriteDispose, payload)
	return err
}

func (c *Conn) publishWriteDispose(writerID uint32, keyBytes, data []byte) error {
	payload := EncodeWriteDispose(&WritePayload{
		Mode:     WriteWriterMode,
		WriterID: writerID,
		KeyBytes: keyBytes,
		Data:     data,
	})
	return c.send(MsgWriteDispose, c.nextRequestID(), payload)
}

func (c *Conn) write(ctx context.Context, topicName, typeName string, qos []QosPolicy, keys []KeyField, data []byte) error {
	payload := EncodeWrite(&WritePayload{
		Mode:      WriteTopicMode,
		TopicName: topicName,
		TypeName:  typeName,
		Qos:       qos,
		Keys:      keys,
		Data:      data,
	})
	_, err := c.sendAndWait(ctx, MsgWrite, payload)
	return err
}

func (c *Conn) ping(ctx context.Context) error {
	_, err := c.sendAndWait(ctx, MsgPing, nil)
	return err
}

func (c *Conn) close() error {
	if c.closed.Swap(true) {
		return nil
	}
	close(c.closeCh)
	err := c.ws.Close()
	<-c.recvDone
	return err
}

func (c *Conn) recvLoop() {
	defer close(c.recvDone)
	for {
		_, data, err := c.ws.ReadMessage()
		if err != nil {
			return
		}
		if len(data) < HeaderLen {
			continue
		}
		hdr, err := DecodeHeader(data)
		if err != nil {
			continue
		}
		if int(hdr.PayloadLength) > len(data)-HeaderLen {
			continue
		}
		payload := data[HeaderLen : HeaderLen+int(hdr.PayloadLength)]

		// Correlated response
		if hdr.RequestID != 0 {
			if ch, ok := c.pending.Load(hdr.RequestID); ok {
				ch.(chan *rawMessage) <- &rawMessage{Header: hdr, Payload: payload}
				continue
			}
			if hdr.Type == MsgOK {
				continue // discard unmatched OK from fire-and-forget
			}
			if hdr.Type == MsgError {
				c.deliverError(payload)
				continue
			}
		}

		// Unsolicited data messages
		switch hdr.Type {
		case MsgData:
			dp, err := DecodeData(payload)
			if err != nil {
				continue
			}
			c.onData(&DataMessage{
				TopicName:       dp.TopicName,
				SourceTimestamp: dp.SourceTimestamp,
				Data:            dp.Data,
				State:           StateAlive,
			})
		case MsgDataDisposed:
			dp, err := DecodeDataDisposed(payload)
			if err != nil {
				continue
			}
			c.onData(&DataMessage{
				TopicName:       dp.TopicName,
				SourceTimestamp: dp.SourceTimestamp,
				Data:            dp.KeyData,
				State:           StateDisposed,
			})
		case MsgDataNoWriters:
			dp, err := DecodeDataNoWriters(payload)
			if err != nil {
				continue
			}
			c.onData(&DataMessage{
				TopicName: dp.TopicName,
				Data:      dp.KeyData,
				State:     StateNoWriters,
			})
		case MsgError:
			c.deliverError(payload)
		}
	}
}

func (c *Conn) deliverError(payload []byte) {
	if hp := c.onError.Load(); hp != nil {
		ep, err := DecodeError(payload)
		if err == nil {
			(*hp)(ep.Code, ep.Message)
		}
	}
}
