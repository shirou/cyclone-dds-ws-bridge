package ddswsclient_test

import (
	"context"
	"fmt"
	"time"

	"github.com/shirou/cyclone-dds-ws-bridge/ddswsclient"
)

// This example demonstrates the intended workflow for using the bridge client.
// It requires a running bridge at ws://localhost:9876.
func Example() {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()

	// Connect to the bridge (auto-reconnect enabled by default)
	client, err := ddswsclient.NewClient(ctx, "ws://localhost:9876",
		ddswsclient.WithReconnectInterval(2*time.Second),
	)
	if err != nil {
		fmt.Println("connect:", err)
		return
	}
	defer client.Close()

	// Optional: register lifecycle handlers
	client.OnConnect(func() {
		fmt.Println("connected")
	})
	client.OnDisconnect(func(err error) {
		fmt.Println("disconnected:", err)
	})

	// Subscribe to a topic
	sub, err := client.Subscribe(ctx, "SensorData", "sensor::SensorData",
		[]ddswsclient.QosPolicy{
			ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 100),
		},
		true, // isKeyed
		[]ddswsclient.KeyField{
			{Offset: 4, Size: 4, TypeHint: ddswsclient.KeyInt32},
		},
	)
	if err != nil {
		fmt.Println("subscribe:", err)
		return
	}
	defer sub.Unsubscribe(ctx)

	// Create a writer for publishing
	w, err := client.CreateWriter(ctx, "SensorData", "sensor::SensorData",
		[]ddswsclient.QosPolicy{
			ddswsclient.QosReliabilityPolicy(ddswsclient.ReliabilityReliable, 100),
		},
		true, // isKeyed
		[]ddswsclient.KeyField{
			{Offset: 4, Size: 4, TypeHint: ddswsclient.KeyInt32},
		},
	)
	if err != nil {
		fmt.Println("create writer:", err)
		return
	}
	defer w.Close(ctx)

	// Publish data (CDR-encoded payload from go-dds-idlgen)
	sampleCDR := []byte{0x00, 0x01, 0x00, 0x00, 0x2A, 0x00, 0x00, 0x00}
	if err := w.Publish(sampleCDR); err != nil {
		fmt.Println("publish:", err)
		return
	}

	// Receive data from the subscription channel
	select {
	case msg := <-sub.C:
		fmt.Printf("received: topic=%s state=%d len=%d\n",
			msg.TopicName, msg.State, len(msg.Data))
	case <-ctx.Done():
		fmt.Println("timeout waiting for data")
	}
}
