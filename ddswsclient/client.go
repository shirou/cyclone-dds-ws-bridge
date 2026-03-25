package ddswsclient

import (
	"context"
	"fmt"
	"log/slog"
	"sync"
	"sync/atomic"
	"time"
)

// ClientOption configures the Client.
type ClientOption func(*clientConfig)

type clientConfig struct {
	reconnectInterval time.Duration
	maxReconnects     int // -1 = infinite
}

// WithReconnectInterval sets the delay between reconnect attempts (default: 1s).
func WithReconnectInterval(d time.Duration) ClientOption {
	return func(c *clientConfig) { c.reconnectInterval = d }
}

// WithMaxReconnects sets the maximum number of reconnect attempts.
// -1 means infinite (default). 0 disables auto-reconnect.
func WithMaxReconnects(n int) ClientOption {
	return func(c *clientConfig) { c.maxReconnects = n }
}

// Subscription represents an active topic subscription.
// Read from C to receive data messages. The channel survives reconnections.
type Subscription struct {
	// C delivers data messages for this subscription.
	// Closed when Unsubscribe is called or the Client is closed.
	C <-chan *DataMessage

	client       *Client
	ch           chan *DataMessage
	closeOnce    sync.Once
	unsubscribed atomic.Bool

	// State for re-subscribe on reconnect
	topicName string
	typeName  string
	qos       []QosPolicy
	isKeyed   bool
	keys      []KeyField
}

// Unsubscribe stops receiving data and closes the channel.
// Safe to call multiple times.
func (s *Subscription) Unsubscribe(ctx context.Context) error {
	s.unsubscribed.Store(true)
	s.client.removeSub(s)
	s.closeOnce.Do(func() { close(s.ch) })

	// Send UNSUBSCRIBE if connected (best-effort)
	s.client.connMu.RLock()
	conn := s.client.conn
	s.client.connMu.RUnlock()
	if conn != nil {
		return conn.unsubscribe(ctx, s.topicName, s.typeName)
	}
	return nil
}

// Writer represents a DDS DataWriter that survives reconnections.
// The underlying writer_id is transparently re-created on reconnect.
type Writer struct {
	client *Client

	mu       sync.RWMutex
	writerID uint32
	closed   bool

	// State for re-create on reconnect
	topicName string
	typeName  string
	qos       []QosPolicy
	isKeyed   bool
	keys      []KeyField
}

// Publish sends a WRITE (fire-and-forget) using this writer.
// Returns ErrNotConnected if the client is currently reconnecting.
func (w *Writer) Publish(data []byte) error {
	w.mu.RLock()
	if w.closed {
		w.mu.RUnlock()
		return ErrNotConnected
	}
	id := w.writerID
	w.mu.RUnlock()

	w.client.connMu.RLock()
	conn := w.client.conn
	w.client.connMu.RUnlock()
	if conn == nil {
		return ErrNotConnected
	}
	return conn.publish(id, data)
}

// PublishDispose sends a DISPOSE (fire-and-forget) using this writer.
func (w *Writer) PublishDispose(keyData []byte) error {
	w.mu.RLock()
	if w.closed {
		w.mu.RUnlock()
		return ErrNotConnected
	}
	id := w.writerID
	w.mu.RUnlock()

	w.client.connMu.RLock()
	conn := w.client.conn
	w.client.connMu.RUnlock()
	if conn == nil {
		return ErrNotConnected
	}
	return conn.publishDispose(id, keyData)
}

// PublishWriteDispose sends a WRITE_DISPOSE (fire-and-forget) using this writer.
func (w *Writer) PublishWriteDispose(data []byte) error {
	w.mu.RLock()
	if w.closed {
		w.mu.RUnlock()
		return ErrNotConnected
	}
	id := w.writerID
	w.mu.RUnlock()

	w.client.connMu.RLock()
	conn := w.client.conn
	w.client.connMu.RUnlock()
	if conn == nil {
		return ErrNotConnected
	}
	return conn.publishWriteDispose(id, data)
}

// Close deletes this writer. Safe to call multiple times.
func (w *Writer) Close(ctx context.Context) error {
	w.mu.Lock()
	if w.closed {
		w.mu.Unlock()
		return nil
	}
	w.closed = true
	id := w.writerID
	w.mu.Unlock()

	w.client.removeWriter(w)

	w.client.connMu.RLock()
	conn := w.client.conn
	w.client.connMu.RUnlock()
	if conn != nil {
		return conn.deleteWriter(ctx, id)
	}
	return nil
}

// ConnectHandler is called when the client connects or reconnects to the bridge.
type ConnectHandler func()

// DisconnectHandler is called when the connection to the bridge is lost.
type DisconnectHandler func(err error)

// Client is a high-level DDS bridge client with auto-reconnect.
// Subscriptions and Writers survive bridge restarts transparently.
//
// Usage:
//
//	client, err := ddswsclient.NewClient(ctx, "ws://localhost:9876")
//	defer client.Close()
//
//	sub, err := client.Subscribe(ctx, "SensorData", "sensor::SensorData", nil, nil)
//	defer sub.Unsubscribe(ctx)
//
//	w, err := client.CreateWriter(ctx, "SensorData", "sensor::SensorData", nil, nil)
//	defer w.Close(ctx)
//
//	for {
//	    select {
//	    case msg := <-sub.C:
//	        // handle data
//	    case <-ctx.Done():
//	        return
//	    }
//	}
type Client struct {
	addr string
	cfg  clientConfig

	connMu sync.RWMutex
	conn   *Conn // nil during reconnect

	subsMu  sync.RWMutex
	subs    []*Subscription
	writers []*Writer

	onConnect    ConnectHandler
	onDisconnect DisconnectHandler
	onError      ErrorHandler

	closed  chan struct{}
	done    chan struct{}
	closeMu sync.Mutex
}

// NewClient connects to the bridge and returns a Client with auto-reconnect.
// The initial connection is synchronous -- returns an error if it fails.
func NewClient(ctx context.Context, addr string, opts ...ClientOption) (*Client, error) {
	cfg := clientConfig{
		reconnectInterval: time.Second,
		maxReconnects:     -1,
	}
	for _, o := range opts {
		o(&cfg)
	}

	c := &Client{
		addr:   addr,
		cfg:    cfg,
		closed: make(chan struct{}),
		done:   make(chan struct{}),
	}

	conn, err := dial(ctx, addr, c.deliverData)
	if err != nil {
		return nil, err
	}
	c.setErrorHandler(conn)
	c.conn = conn

	go c.watchLoop()

	return c, nil
}

// OnConnect registers a handler called on each successful (re)connection.
// Must be called before any reconnect occurs.
func (c *Client) OnConnect(h ConnectHandler) {
	c.onConnect = h
}

// OnDisconnect registers a handler called when the connection is lost.
func (c *Client) OnDisconnect(h DisconnectHandler) {
	c.onDisconnect = h
}

// OnError registers a handler for unsolicited errors and async write errors.
func (c *Client) OnError(h ErrorHandler) {
	c.onError = h
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()
	if conn != nil {
		c.setErrorHandler(conn)
	}
}

func (c *Client) setErrorHandler(conn *Conn) {
	if c.onError != nil {
		h := c.onError
		conn.onError.Store(&h)
	}
}

// Subscribe sends a SUBSCRIBE request and returns a Subscription.
// The channel survives reconnections -- data resumes after reconnect.
// Messages during the reconnect gap are lost (the bridge does not buffer).
func (c *Client) Subscribe(ctx context.Context, topicName, typeName string, qos []QosPolicy, isKeyed bool, keys []KeyField) (*Subscription, error) {
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()
	if conn == nil {
		return nil, ErrNotConnected
	}

	if err := conn.subscribe(ctx, topicName, typeName, qos, isKeyed, keys); err != nil {
		return nil, err
	}

	ch := make(chan *DataMessage, 256)
	sub := &Subscription{
		C:         ch,
		client:    c,
		ch:        ch,
		topicName: topicName,
		typeName:  typeName,
		qos:       qos,
		isKeyed:   isKeyed,
		keys:      keys,
	}

	c.subsMu.Lock()
	c.subs = append(c.subs, sub)
	c.subsMu.Unlock()

	return sub, nil
}

// CreateWriter creates a DDS DataWriter that survives reconnections.
func (c *Client) CreateWriter(ctx context.Context, topicName, typeName string, qos []QosPolicy, isKeyed bool, keys []KeyField) (*Writer, error) {
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()
	if conn == nil {
		return nil, ErrNotConnected
	}

	writerID, err := conn.createWriter(ctx, topicName, typeName, qos, isKeyed, keys)
	if err != nil {
		return nil, err
	}

	w := &Writer{
		client:    c,
		writerID:  writerID,
		topicName: topicName,
		typeName:  typeName,
		qos:       qos,
		isKeyed:   isKeyed,
		keys:      keys,
	}

	c.subsMu.Lock()
	c.writers = append(c.writers, w)
	c.subsMu.Unlock()

	return w, nil
}

// Write sends a WRITE in topic-name mode (synchronous, creates implicit writer).
// For high-frequency writes, use CreateWriter + Writer.Publish instead.
func (c *Client) Write(ctx context.Context, topicName, typeName string, qos []QosPolicy, keys []KeyField, data []byte) error {
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()
	if conn == nil {
		return ErrNotConnected
	}
	return conn.write(ctx, topicName, typeName, qos, keys, data)
}

// Dispose sends a DISPOSE in topic-name mode (synchronous).
func (c *Client) Dispose(ctx context.Context, topicName, typeName string, qos []QosPolicy, keys []KeyField, keyData []byte) error {
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()
	if conn == nil {
		return ErrNotConnected
	}
	return conn.dispose(ctx, topicName, typeName, qos, keys, keyData)
}

// WriteDispose sends a WRITE_DISPOSE in topic-name mode (synchronous).
func (c *Client) WriteDispose(ctx context.Context, topicName, typeName string, qos []QosPolicy, keys []KeyField, data []byte) error {
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()
	if conn == nil {
		return ErrNotConnected
	}
	return conn.writeDispose(ctx, topicName, typeName, qos, keys, data)
}

// Ping sends a PING and waits for the PONG response.
func (c *Client) Ping(ctx context.Context) error {
	c.connMu.RLock()
	conn := c.conn
	c.connMu.RUnlock()
	if conn == nil {
		return ErrNotConnected
	}
	return conn.ping(ctx)
}

// Close stops the client, closes the connection and all subscription channels.
func (c *Client) Close() error {
	c.closeMu.Lock()
	select {
	case <-c.closed:
		c.closeMu.Unlock()
		return nil
	default:
		close(c.closed)
	}
	c.closeMu.Unlock()

	<-c.done // wait for watchLoop to exit

	c.connMu.Lock()
	conn := c.conn
	c.conn = nil
	c.connMu.Unlock()

	var err error
	if conn != nil {
		err = conn.close()
	}

	// Close all subscription channels
	c.subsMu.Lock()
	for _, sub := range c.subs {
		sub.unsubscribed.Store(true)
		sub.closeOnce.Do(func() { close(sub.ch) })
	}
	c.subs = nil
	c.writers = nil
	c.subsMu.Unlock()

	return err
}

// --- Internal ---

func (c *Client) removeSub(sub *Subscription) {
	c.subsMu.Lock()
	defer c.subsMu.Unlock()
	for i, s := range c.subs {
		if s == sub {
			c.subs = append(c.subs[:i], c.subs[i+1:]...)
			return
		}
	}
}

func (c *Client) removeWriter(w *Writer) {
	c.subsMu.Lock()
	defer c.subsMu.Unlock()
	for i, wr := range c.writers {
		if wr == w {
			c.writers = append(c.writers[:i], c.writers[i+1:]...)
			return
		}
	}
}

func (c *Client) deliverData(msg *DataMessage) {
	c.subsMu.RLock()
	var targets []*Subscription
	for _, sub := range c.subs {
		if sub.topicName == msg.TopicName {
			targets = append(targets, sub)
		}
	}
	c.subsMu.RUnlock()

	for _, sub := range targets {
		if sub.unsubscribed.Load() {
			continue
		}
		select {
		case sub.ch <- msg:
		default:
			// Drop oldest to make room (backpressure)
			select {
			case <-sub.ch:
			default:
			}
			select {
			case sub.ch <- msg:
			default:
			}
		}
	}
}

// watchLoop monitors the connection and reconnects on failure.
func (c *Client) watchLoop() {
	defer close(c.done)

	for {
		c.connMu.RLock()
		conn := c.conn
		c.connMu.RUnlock()

		if conn == nil {
			return
		}

		// Wait for recvLoop to exit (connection lost)
		select {
		case <-c.closed:
			return
		case <-conn.recvDone:
		}

		// Connection lost
		if c.onDisconnect != nil {
			c.onDisconnect(ErrNotConnected)
		}

		// Set conn = nil so Publish returns ErrNotConnected during reconnect
		c.connMu.Lock()
		c.conn = nil
		c.connMu.Unlock()

		conn.close()

		// Reconnect loop
		if !c.reconnect() {
			return // closed or max retries exhausted
		}
	}
}

func (c *Client) reconnect() bool {
	attempts := 0
	for {
		select {
		case <-c.closed:
			return false
		default:
		}

		if c.cfg.maxReconnects >= 0 && attempts >= c.cfg.maxReconnects {
			return false
		}

		// Wait before retrying
		timer := time.NewTimer(c.cfg.reconnectInterval)
		select {
		case <-c.closed:
			timer.Stop()
			return false
		case <-timer.C:
		}

		attempts++

		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		newConn, err := dial(ctx, c.addr, c.deliverData)
		cancel()
		if err != nil {
			slog.Debug("reconnect dial failed", "attempt", attempts, "error", err)
			continue
		}
		c.setErrorHandler(newConn)

		// Re-create all writers and subscriptions (all-or-nothing)
		if err := c.restoreState(newConn); err != nil {
			slog.Debug("reconnect restore failed", "attempt", attempts, "error", err)
			newConn.close()
			continue
		}

		// Atomically swap connection
		c.connMu.Lock()
		c.conn = newConn
		c.connMu.Unlock()

		if c.onConnect != nil {
			c.onConnect()
		}

		return true
	}
}

func (c *Client) restoreState(conn *Conn) error {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	c.subsMu.Lock()
	writers := make([]*Writer, len(c.writers))
	copy(writers, c.writers)
	subs := make([]*Subscription, len(c.subs))
	copy(subs, c.subs)
	c.subsMu.Unlock()

	// Re-create all writers
	for _, w := range writers {
		w.mu.Lock()
		if w.closed {
			w.mu.Unlock()
			continue
		}
		newID, err := conn.createWriter(ctx, w.topicName, w.typeName, w.qos, w.isKeyed, w.keys)
		if err != nil {
			w.mu.Unlock()
			return fmt.Errorf("re-create writer for topic %q: %w", w.topicName, err)
		}
		w.writerID = newID
		w.mu.Unlock()
	}

	// Re-subscribe all subscriptions
	for _, sub := range subs {
		if err := conn.subscribe(ctx, sub.topicName, sub.typeName, sub.qos, sub.isKeyed, sub.keys); err != nil {
			return fmt.Errorf("re-subscribe topic %q: %w", sub.topicName, err)
		}
	}

	return nil
}
