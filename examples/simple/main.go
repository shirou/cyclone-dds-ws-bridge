// Example: simple pub/sub
//
// This example demonstrates the simplest possible publish/subscribe pattern
// using the DDS WebSocket bridge. A publisher sends HelloWorld messages and
// a subscriber receives them on the same topic.
//
// The generated types in gen/example/ are produced by go-dds-idlgen from the
// IDL definition in idl/HelloWorld.idl. To regenerate:
//
//	go-dds-idlgen -o gen/ -I idl/ idl/HelloWorld.idl
//
// Prerequisites:
//   - A running cyclone-dds-ws-bridge (see: make bridge in examples/)
//
// Usage:
//
//	go run . -mode pub    # in one terminal
//	go run . -mode sub    # in another terminal
package main

import (
	"context"
	"flag"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"time"

	"github.com/shirou/cyclone-dds-ws-bridge/ddswsclient"
	"github.com/shirou/cyclone-dds-ws-bridge/examples/simple/gen/example"
)

func main() {
	mode := flag.String("mode", "pub", "run mode: pub or sub")
	addr := flag.String("addr", "ws://localhost:9876", "bridge WebSocket address")
	flag.Parse()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()

	// Connect to the bridge. WithConnectRetry retries until the bridge becomes
	// available, which is useful when the bridge and the app start concurrently.
	client, err := ddswsclient.NewClient(ctx, *addr,
		ddswsclient.WithReconnectInterval(2*time.Second),
		ddswsclient.WithConnectRetry(true),
	)
	if err != nil {
		slog.Error("failed to connect", "error", err)
		os.Exit(1)
	}
	defer client.Close()

	client.OnConnect(func() { slog.Info("connected to bridge") })
	client.OnDisconnect(func(err error) { slog.Warn("disconnected", "error", err) })

	switch *mode {
	case "pub":
		if err := runPublisher(ctx, client); err != nil {
			slog.Error("publisher error", "error", err)
			os.Exit(1)
		}
	case "sub":
		if err := runSubscriber(ctx, client); err != nil {
			slog.Error("subscriber error", "error", err)
			os.Exit(1)
		}
	default:
		fmt.Fprintf(os.Stderr, "unknown mode: %s (use pub or sub)\n", *mode)
		os.Exit(1)
	}
}

func runPublisher(ctx context.Context, client *ddswsclient.Client) error {
	// QoS must be compatible between publisher and subscriber.
	//   Reliable: retransmit lost samples (maxBlockingTime=5000ms)
	//   Volatile: no historical data for late joiners
	//   KeepLast(1): only the most recent sample is stored
	qos := []ddswsclient.QosPolicy{
		ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 5000),
		ddswsclient.QosDurabilityPolicy(ddswsclient.DurabilityVolatile),
		ddswsclient.QosHistoryPolicy(ddswsclient.HistoryKeepLast, 1),
	}

	// CreateWriter args: topic name, DDS type name, QoS, key extractor.
	// The bridge uses an opaque sertype, so the type name is used only for
	// topic matching. Here we reuse the topic name as the type name for simplicity.
	// keyExtract is nil because HelloWorld is not a keyed type.
	writer, err := client.CreateWriter(ctx,
		example.HelloWorldTopic, // topic name
		example.HelloWorldTopic, // type name (opaque — used for matching only)
		qos,
		nil, // keyExtract: nil for non-keyed types
	)
	if err != nil {
		return fmt.Errorf("create writer: %w", err)
	}
	defer writer.Close(ctx)

	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()

	var count int32
	for {
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
			msg := example.HelloWorld{
				Message: fmt.Sprintf("Hello, DDS! #%d", count),
				Count:   count,
			}

			// MarshalCDR produces an XCDR2-encoded byte slice with encapsulation header.
			data, err := msg.MarshalCDR()
			if err != nil {
				slog.Error("marshal failed", "error", err)
				continue
			}
			if err := writer.Publish(data); err != nil {
				slog.Warn("publish failed", "error", err)
				continue
			}
			slog.Info("published", "message", msg.Message, "count", msg.Count)
			count++
		}
	}
}

func runSubscriber(ctx context.Context, client *ddswsclient.Client) error {
	qos := []ddswsclient.QosPolicy{
		ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 5000),
		ddswsclient.QosDurabilityPolicy(ddswsclient.DurabilityVolatile),
		ddswsclient.QosHistoryPolicy(ddswsclient.HistoryKeepLast, 1),
	}

	// Subscribe args: topic name, type name, QoS, isKeyed, keyFields.
	// isKeyed=false and keyFields=nil because HelloWorld has no @key fields.
	sub, err := client.Subscribe(ctx,
		example.HelloWorldTopic, // topic name
		example.HelloWorldTopic, // type name
		qos,
		false, // isKeyed
		nil,   // keyFields
	)
	if err != nil {
		return fmt.Errorf("subscribe: %w", err)
	}
	defer sub.Unsubscribe(ctx)

	slog.Info("waiting for messages...", "topic", example.HelloWorldTopic)

	for {
		select {
		case <-ctx.Done():
			return nil
		case msg, ok := <-sub.C:
			if !ok {
				return nil // channel closed on disconnect
			}
			var hello example.HelloWorld
			if err := hello.UnmarshalCDR(msg.Data); err != nil {
				slog.Error("unmarshal failed", "error", err)
				continue
			}
			slog.Info("received", "message", hello.Message, "count", hello.Count)
		}
	}
}
