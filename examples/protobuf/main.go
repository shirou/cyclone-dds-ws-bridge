// Example: protobuf pub/sub
//
// This example demonstrates publishing and subscribing using Protocol Buffers
// for serialization instead of CDR. The bridge transports opaque byte payloads,
// so any serialization format works — protobuf, CDR, JSON, MessagePack, etc.
//
// IMPORTANT: protobuf payloads are NOT interoperable with native DDS readers.
// A native Cyclone DDS or RTI Connext subscriber expecting CDR/XCDR2 will fail
// to deserialize these messages. Use this approach only when both publisher and
// subscriber are bridge clients that agree on protobuf as the wire format.
//
// The generated types in gen/temperaturepb/ are produced by buf from the
// proto definition in proto/temperature.proto. To regenerate:
//
//	buf generate proto
//
// Prerequisites:
//   - A running cyclone-dds-ws-bridge (see: make bridge in examples/)
//
// Usage:
//
//	go run . -mode pub    # publish temperature readings
//	go run . -mode sub    # subscribe and display readings
package main

import (
	"context"
	"flag"
	"fmt"
	"log/slog"
	"math/rand/v2"
	"os"
	"os/signal"
	"time"

	"github.com/shirou/cyclone-dds-ws-bridge/ddswsclient"
	"github.com/shirou/cyclone-dds-ws-bridge/examples/protobuf/gen/temperaturepb"
	"google.golang.org/protobuf/proto"
)

const (
	// topicName identifies the DDS topic on the bridge.
	topicName = "proto::TemperatureReading"
)

func main() {
	mode := flag.String("mode", "pub", "run mode: pub or sub")
	addr := flag.String("addr", "ws://localhost:9876", "bridge WebSocket address")
	flag.Parse()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()

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
	qos := []ddswsclient.QosPolicy{
		ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 5000),
		ddswsclient.QosDurabilityPolicy(ddswsclient.DurabilityVolatile),
		ddswsclient.QosHistoryPolicy(ddswsclient.HistoryKeepLast, 1),
	}

	// Both topic and type name are set to topicName. With an opaque sertype
	// bridge the type name is only used for topic matching, not for
	// deserialization. Non-keyed, so keyExtract is nil.
	writer, err := client.CreateWriter(ctx, topicName, topicName, qos, nil)
	if err != nil {
		return fmt.Errorf("create writer: %w", err)
	}
	defer writer.Close(ctx)

	locations := []string{"office", "warehouse", "outdoor", "server-room"}

	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()

	slog.Info("publishing temperature readings (protobuf)")

	for {
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
			reading := &temperaturepb.TemperatureReading{
				SensorId:    fmt.Sprintf("sensor-%d", rand.IntN(4)+1),
				Temperature: 18.0 + rand.Float64()*20.0,
				Location:    locations[rand.IntN(len(locations))],
				TimestampNs: time.Now().UnixNano(),
			}

			// proto.Marshal produces a protobuf wire-format byte slice.
			// The bridge forwards these bytes as-is to subscribers.
			data, err := proto.Marshal(reading)
			if err != nil {
				slog.Error("marshal failed", "error", err)
				continue
			}
			if err := writer.Publish(data); err != nil {
				slog.Warn("publish failed", "error", err)
				continue
			}
			slog.Info("published",
				"sensor_id", reading.SensorId,
				"temperature", fmt.Sprintf("%.1f", reading.Temperature),
				"location", reading.Location,
			)
		}
	}
}

func runSubscriber(ctx context.Context, client *ddswsclient.Client) error {
	qos := []ddswsclient.QosPolicy{
		ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 5000),
		ddswsclient.QosDurabilityPolicy(ddswsclient.DurabilityVolatile),
		ddswsclient.QosHistoryPolicy(ddswsclient.HistoryKeepLast, 1),
	}

	// Non-keyed subscription (protobuf has no native DDS key concept).
	sub, err := client.Subscribe(ctx, topicName, topicName, qos, false, nil)
	if err != nil {
		return fmt.Errorf("subscribe: %w", err)
	}
	defer sub.Unsubscribe(ctx)

	slog.Info("waiting for temperature readings...", "topic", topicName)

	for {
		select {
		case <-ctx.Done():
			return nil
		case msg, ok := <-sub.C:
			if !ok {
				return nil
			}

			var reading temperaturepb.TemperatureReading
			if err := proto.Unmarshal(msg.Data, &reading); err != nil {
				slog.Error("unmarshal failed", "error", err)
				continue
			}

			ts := time.Unix(0, reading.TimestampNs)
			slog.Info("received",
				"sensor_id", reading.SensorId,
				"temperature", fmt.Sprintf("%.1f", reading.Temperature),
				"location", reading.Location,
				"time", ts.Format(time.TimeOnly),
			)
		}
	}
}
