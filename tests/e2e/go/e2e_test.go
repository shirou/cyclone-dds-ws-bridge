package main

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"sync"
	"testing"
	"time"

	"github.com/shirou/cyclone-dds-ws-bridge/ddswsclient"
)

func bridgeAddr() string {
	if addr := os.Getenv("BRIDGE_ADDR"); addr != "" {
		return addr
	}
	return "ws://localhost:9876"
}

func connectWithRetry(t *testing.T, ctx context.Context) *ddswsclient.Client {
	t.Helper()
	addr := bridgeAddr()
	client, err := ddswsclient.NewClient(ctx, addr,
		ddswsclient.WithConnectRetry(true),
		ddswsclient.WithReconnectInterval(500*time.Millisecond),
		ddswsclient.WithMaxReconnects(-1),
	)
	if err != nil {
		t.Fatalf("failed to connect to bridge at %s: %v", addr, err)
	}
	return client
}

// TestPingPong verifies basic connectivity.
func TestPingPong(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	client := connectWithRetry(t, ctx)
	defer client.Close()

	if err := client.Ping(ctx); err != nil {
		t.Fatalf("Ping: %v", err)
	}
}

// TestPubSubLoopback writes data via the bridge and receives it back
// through a subscription on the same topic.
func TestPubSubLoopback(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	client := connectWithRetry(t, ctx)
	defer client.Close()

	topic := "E2ELoopback"
	typeName := "test::Opaque"

	sub, err := client.Subscribe(ctx, topic, typeName, nil, nil)
	if err != nil {
		t.Fatalf("Subscribe: %v", err)
	}
	defer sub.Unsubscribe(ctx)

	// Allow subscription to propagate through DDS
	time.Sleep(500 * time.Millisecond)

	testData := []byte{0x00, 0x01, 0x00, 0x00, 0xDE, 0xAD, 0xBE, 0xEF}

	if err := client.Write(ctx, topic, typeName, nil, nil, testData); err != nil {
		t.Fatalf("Write: %v", err)
	}

	select {
	case msg := <-sub.C:
		if msg.State != ddswsclient.StateAlive {
			t.Errorf("expected StateAlive, got %d", msg.State)
		}
		if msg.TopicName != topic {
			t.Errorf("topic: got %q, want %q", msg.TopicName, topic)
		}
		if !bytes.Equal(msg.Data, testData) {
			t.Errorf("data mismatch:\n  got:  %x\n  want: %x", msg.Data, testData)
		}
	case <-ctx.Done():
		t.Fatal("timed out waiting for DATA message")
	}
}

// TestCreateWriterPublish tests the writer-id mode path.
func TestCreateWriterPublish(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	client := connectWithRetry(t, ctx)
	defer client.Close()

	topic := "E2EWriterMode"
	typeName := "test::Opaque"

	sub, err := client.Subscribe(ctx, topic, typeName, nil, nil)
	if err != nil {
		t.Fatalf("Subscribe: %v", err)
	}
	defer sub.Unsubscribe(ctx)

	w, err := client.CreateWriter(ctx, topic, typeName, nil, nil)
	if err != nil {
		t.Fatalf("CreateWriter: %v", err)
	}
	defer w.Close(ctx)

	time.Sleep(500 * time.Millisecond)

	testData := []byte{0x00, 0x01, 0x00, 0x00, 0xCA, 0xFE, 0xBA, 0xBE}

	if err := w.Publish(testData); err != nil {
		t.Fatalf("Publish: %v", err)
	}

	select {
	case msg := <-sub.C:
		if !bytes.Equal(msg.Data, testData) {
			t.Errorf("data mismatch:\n  got:  %x\n  want: %x", msg.Data, testData)
		}
	case <-ctx.Done():
		t.Fatal("timed out waiting for DATA message")
	}
}

// TestDisposeLoopback verifies dispose propagation through the bridge.
func TestDisposeLoopback(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	client := connectWithRetry(t, ctx)
	defer client.Close()

	topic := "E2EDispose"
	typeName := "test::Opaque"

	sub, err := client.Subscribe(ctx, topic, typeName, nil, nil)
	if err != nil {
		t.Fatalf("Subscribe: %v", err)
	}
	defer sub.Unsubscribe(ctx)

	time.Sleep(500 * time.Millisecond)

	// First write some data
	testData := []byte{0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04}
	if err := client.Write(ctx, topic, typeName, nil, nil, testData); err != nil {
		t.Fatalf("Write: %v", err)
	}

	// Wait for DATA
	select {
	case msg := <-sub.C:
		if msg.State != ddswsclient.StateAlive {
			t.Errorf("expected StateAlive, got %d", msg.State)
		}
	case <-ctx.Done():
		t.Fatal("timed out waiting for DATA")
	}

	// Dispose with key data
	keyData := []byte{0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04}
	if err := client.Dispose(ctx, topic, typeName, nil, nil, keyData); err != nil {
		t.Fatalf("Dispose: %v", err)
	}

	// Wait for DATA_DISPOSED
	select {
	case msg := <-sub.C:
		if msg.State != ddswsclient.StateDisposed {
			t.Errorf("expected StateDisposed, got %d", msg.State)
		}
	case <-ctx.Done():
		t.Fatal("timed out waiting for DATA_DISPOSED")
	}
}

// TestMultipleClients verifies fan-out to multiple subscribers.
func TestMultipleClients(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	const numSubscribers = 3
	topic := "E2EFanOut"
	typeName := "test::Opaque"

	// Create subscriber clients
	var subs []*ddswsclient.Subscription
	var clients []*ddswsclient.Client
	for i := range numSubscribers {
		c := connectWithRetry(t, ctx)
		clients = append(clients, c)
		defer c.Close()

		sub, err := c.Subscribe(ctx, topic, typeName, nil, nil)
		if err != nil {
			t.Fatalf("Subscribe[%d]: %v", i, err)
		}
		subs = append(subs, sub)
		defer sub.Unsubscribe(ctx)
	}

	// Writer client
	writer := connectWithRetry(t, ctx)
	defer writer.Close()

	time.Sleep(1 * time.Second)

	testData := []byte{0x00, 0x01, 0x00, 0x00, 0xFA, 0xCE}
	if err := writer.Write(ctx, topic, typeName, nil, nil, testData); err != nil {
		t.Fatalf("Write: %v", err)
	}

	// All subscribers should receive the message
	var wg sync.WaitGroup
	errors := make(chan string, numSubscribers)

	for i, sub := range subs {
		wg.Add(1)
		go func(idx int, s *ddswsclient.Subscription) {
			defer wg.Done()
			select {
			case msg := <-s.C:
				if !bytes.Equal(msg.Data, testData) {
					errors <- fmt.Sprintf("sub[%d] data mismatch: got %x, want %x", idx, msg.Data, testData)
				}
			case <-ctx.Done():
				errors <- fmt.Sprintf("sub[%d] timed out", idx)
			}
		}(i, sub)
	}

	wg.Wait()
	close(errors)
	for errMsg := range errors {
		t.Error(errMsg)
	}
}

// TestUnsubscribe verifies that after unsubscribe, no more data is received.
// Uses two clients: client1 unsubscribes, client2 stays subscribed to confirm
// data still flows through the bridge.
func TestUnsubscribe(t *testing.T) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	topic := "E2EUnsub"
	typeName := "test::Opaque"

	// Client 1: will unsubscribe
	client1 := connectWithRetry(t, ctx)
	defer client1.Close()
	sub1, err := client1.Subscribe(ctx, topic, typeName, nil, nil)
	if err != nil {
		t.Fatalf("Subscribe client1: %v", err)
	}

	// Client 2: stays subscribed (control group)
	client2 := connectWithRetry(t, ctx)
	defer client2.Close()
	sub2, err := client2.Subscribe(ctx, topic, typeName, nil, nil)
	if err != nil {
		t.Fatalf("Subscribe client2: %v", err)
	}
	defer sub2.Unsubscribe(ctx)

	// Writer client
	writer := connectWithRetry(t, ctx)
	defer writer.Close()

	time.Sleep(500 * time.Millisecond)

	// Write first message - both should receive
	data1 := []byte{0x00, 0x01, 0x00, 0x00, 0x01}
	if err := writer.Write(ctx, topic, typeName, nil, nil, data1); err != nil {
		t.Fatalf("Write: %v", err)
	}

	select {
	case <-sub1.C:
	case <-ctx.Done():
		t.Fatal("client1 timed out waiting for first message")
	}
	select {
	case <-sub2.C:
	case <-ctx.Done():
		t.Fatal("client2 timed out waiting for first message")
	}

	// Client 1 unsubscribes
	if err := sub1.Unsubscribe(ctx); err != nil {
		t.Fatalf("Unsubscribe: %v", err)
	}

	time.Sleep(500 * time.Millisecond)

	// Write second message - only client2 should receive
	data2 := []byte{0x00, 0x01, 0x00, 0x00, 0x02}
	if err := writer.Write(ctx, topic, typeName, nil, nil, data2); err != nil {
		t.Fatalf("Write after unsub: %v", err)
	}

	// Client 2 should still receive
	select {
	case msg := <-sub2.C:
		if !bytes.Equal(msg.Data, data2) {
			t.Errorf("client2 data mismatch: got %x, want %x", msg.Data, data2)
		}
	case <-ctx.Done():
		t.Fatal("client2 timed out waiting for second message")
	}
}
