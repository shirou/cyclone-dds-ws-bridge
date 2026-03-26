// Example: keyed sensor data pub/sub
//
// This example demonstrates publishing and subscribing to a keyed DDS topic.
// Multiple sensor instances are differentiated by their @key sensor_id field.
// The subscriber handles per-instance lifecycle (alive, disposed, no-writers).
//
// Key concepts shown:
//   - Keyed types: DDS uses @key fields to distinguish instances on the same topic.
//   - SerializedKeyExtractor: auto-extracts key bytes from CDR payloads on publish.
//   - Instance state: subscribers receive StateAlive / StateDisposed / StateNoWriters.
//   - TransientLocal durability: late-joining subscribers receive the last sample.
//
// The generated types in gen/sensor/ are produced by go-dds-idlgen from the
// IDL definition in idl/SensorData.idl. To regenerate:
//
//	go-dds-idlgen -o gen/ -I idl/ idl/SensorData.idl
//
// Prerequisites:
//   - A running cyclone-dds-ws-bridge (see: make bridge in examples/)
//
// Usage:
//
//	go run . -mode pub                    # publish from 3 sensors
//	go run . -mode sub                    # subscribe and track instances
//	go run . -mode pub -sensors 5         # publish from 5 sensors
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
	"github.com/shirou/go-dds-idlgen/cdr"

	"github.com/shirou/cyclone-dds-ws-bridge/examples/sensor/gen/sensor"
)

func main() {
	mode := flag.String("mode", "pub", "run mode: pub or sub")
	addr := flag.String("addr", "ws://localhost:9876", "bridge WebSocket address")
	numSensors := flag.Int("sensors", 3, "number of sensor instances to simulate (pub mode)")
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
		if err := runPublisher(ctx, client, *numSensors); err != nil {
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

func runPublisher(ctx context.Context, client *ddswsclient.Client, numSensors int) error {
	// TransientLocal + KeepLast(1): the bridge retains the latest sample per
	// instance so late-joining subscribers immediately see the current state.
	qos := []ddswsclient.QosPolicy{
		ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 5000),
		ddswsclient.QosDurabilityPolicy(ddswsclient.DurabilityTransientLocal),
		ddswsclient.QosHistoryPolicy(ddswsclient.HistoryKeepLast, 1),
	}

	// SerializedKeyExtractor uses the generated ExtractKeyFields method to
	// automatically locate and serialize the @key fields within each CDR payload.
	// This is the recommended approach — it stays correct even if the CDR layout
	// changes (e.g. optional fields added before the key).
	var sensorType sensor.SensorData
	keyExtract := cdr.SerializedKeyExtractor(&sensorType)

	writer, err := client.CreateWriter(ctx,
		sensor.SensorDataTopic, // topic name
		sensor.SensorDataTopic, // type name (opaque — used for matching only)
		qos,
		keyExtract,
	)
	if err != nil {
		return fmt.Errorf("create writer: %w", err)
	}
	defer writer.Close(ctx)

	ticker := time.NewTicker(1 * time.Second)
	defer ticker.Stop()

	slog.Info("publishing sensor data", "sensors", numSensors)

	for {
		select {
		case <-ctx.Done():
			return nil
		case <-ticker.C:
			for i := range numSensors {
				data := sensor.SensorData{
					SensorID:    int32(i + 1),
					Temperature: 20.0 + rand.Float32()*15.0,
					Humidity:    40.0 + rand.Float32()*40.0,
					Timestamp:   time.Now().UnixNano(),
				}
				buf, err := data.MarshalCDR()
				if err != nil {
					slog.Error("marshal failed", "sensor_id", data.SensorID, "error", err)
					continue
				}
				if err := writer.Publish(buf); err != nil {
					slog.Warn("publish failed", "sensor_id", data.SensorID, "error", err)
					continue
				}
				slog.Info("published",
					"sensor_id", data.SensorID,
					"temperature", fmt.Sprintf("%.1f", data.Temperature),
					"humidity", fmt.Sprintf("%.1f", data.Humidity),
				)
			}
		}
	}
}

func runSubscriber(ctx context.Context, client *ddswsclient.Client) error {
	qos := []ddswsclient.QosPolicy{
		ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 5000),
		ddswsclient.QosDurabilityPolicy(ddswsclient.DurabilityTransientLocal),
		ddswsclient.QosHistoryPolicy(ddswsclient.HistoryKeepLast, 1),
	}

	// For keyed subscriptions, pass isKeyed=true and describe where the key
	// lives inside the CDR payload so the bridge can track per-instance state.
	//
	// SensorData CDR layout (XCDR2 APPENDABLE):
	//   [0..3]  DHEADER (4 bytes, struct size)
	//   [4..7]  sensor_id (int32, @key) <-- Offset=4, Size=4
	//   [8..11] temperature (float32)
	//   [12..15] humidity (float32)
	//   [16..23] timestamp (int64)
	//
	// NOTE: the encapsulation header (4 bytes) is stripped before the bridge
	// processes key fields, so offsets are relative to the DHEADER, not the
	// beginning of the wire bytes.
	sub, err := client.Subscribe(ctx,
		sensor.SensorDataTopic,
		sensor.SensorDataTopic,
		qos,
		true, // isKeyed
		[]ddswsclient.KeyField{
			{Offset: 4, Size: 4, TypeHint: ddswsclient.KeyInt32},
		},
	)
	if err != nil {
		return fmt.Errorf("subscribe: %w", err)
	}
	defer sub.Unsubscribe(ctx)

	slog.Info("waiting for sensor data...", "topic", sensor.SensorDataTopic)

	for {
		select {
		case <-ctx.Done():
			return nil
		case msg, ok := <-sub.C:
			if !ok {
				return nil
			}

			// DDS delivers instance lifecycle events via the State field.
			switch msg.State {
			case ddswsclient.StateAlive:
				var data sensor.SensorData
				if err := data.UnmarshalCDR(msg.Data); err != nil {
					slog.Error("unmarshal failed", "error", err)
					continue
				}
				slog.Info("received",
					"sensor_id", data.SensorID,
					"temperature", fmt.Sprintf("%.1f", data.Temperature),
					"humidity", fmt.Sprintf("%.1f", data.Humidity),
				)
			case ddswsclient.StateDisposed:
				// The publisher explicitly disposed this instance.
				slog.Info("instance disposed", "topic", msg.TopicName)
			case ddswsclient.StateNoWriters:
				// All writers for this instance have disconnected.
				slog.Info("no writers for instance", "topic", msg.TopicName)
			}
		}
	}
}
