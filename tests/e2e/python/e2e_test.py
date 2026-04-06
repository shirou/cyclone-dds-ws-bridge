"""E2E tests for cyclone-dds-ws-bridge via Python client."""

import os
import sys
import time

from bridge_client import (
    BridgeClient,
    STATE_ALIVE,
    STATE_DISPOSED,
    wait_for_message,
)

BRIDGE_ADDR = os.environ.get("BRIDGE_ADDR", "ws://localhost:9876")


def test_ping_pong():
    """Test basic connectivity."""
    client = BridgeClient(BRIDGE_ADDR)
    client.connect()
    try:
        client.ping()
        print("PASS: test_ping_pong")
    finally:
        client.close()


def test_pubsub_loopback():
    """Test write -> subscribe -> receive through the bridge."""
    client = BridgeClient(BRIDGE_ADDR)
    client.connect()
    try:
        topic = "PyE2ELoopback"
        type_name = "test::Opaque"

        ch = client.subscribe(topic, type_name)
        time.sleep(0.5)

        test_data = bytes([0x00, 0x01, 0x00, 0x00, 0xAA, 0xBB, 0xCC, 0xDD])
        client.write(topic, type_name, test_data)

        msg = wait_for_message(ch)
        assert msg.state == STATE_ALIVE, f"expected STATE_ALIVE, got {msg.state}"
        assert msg.topic_name == topic, f"topic: got {msg.topic_name!r}, want {topic!r}"
        assert (
            msg.data == test_data
        ), f"data mismatch: got {msg.data.hex()}, want {test_data.hex()}"
        print("PASS: test_pubsub_loopback")
    finally:
        client.close()


def test_create_writer_publish():
    """Test writer-id mode write path."""
    client = BridgeClient(BRIDGE_ADDR)
    client.connect()
    try:
        topic = "PyE2EWriterMode"
        type_name = "test::Opaque"

        ch = client.subscribe(topic, type_name)
        writer_id = client.create_writer(topic, type_name)
        time.sleep(0.5)

        test_data = bytes([0x00, 0x01, 0x00, 0x00, 0x11, 0x22, 0x33, 0x44])
        client.write_with_writer(writer_id, test_data)

        msg = wait_for_message(ch)
        assert (
            msg.data == test_data
        ), f"data mismatch: got {msg.data.hex()}, want {test_data.hex()}"

        client.delete_writer(writer_id)
        print("PASS: test_create_writer_publish")
    finally:
        client.close()


def test_dispose_loopback():
    """Test dispose propagation through the bridge."""
    client = BridgeClient(BRIDGE_ADDR)
    client.connect()
    try:
        topic = "PyE2EDispose"
        type_name = "test::Opaque"

        ch = client.subscribe(topic, type_name)
        time.sleep(0.5)

        # Write data first
        test_data = bytes([0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04])
        client.write(topic, type_name, test_data)

        msg = wait_for_message(ch)
        assert msg.state == STATE_ALIVE, f"expected STATE_ALIVE, got {msg.state}"

        # Dispose
        key_data = bytes([0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04])
        client.dispose(topic, type_name, key_data)

        msg = wait_for_message(ch)
        assert msg.state == STATE_DISPOSED, f"expected STATE_DISPOSED, got {msg.state}"
        print("PASS: test_dispose_loopback")
    finally:
        client.close()


def test_multiple_clients():
    """Test fan-out to multiple subscribers."""
    topic = "PyE2EFanOut"
    type_name = "test::Opaque"
    num_subscribers = 3

    subscribers = []
    channels = []
    try:
        for _ in range(num_subscribers):
            c = BridgeClient(BRIDGE_ADDR)
            c.connect()
            ch = c.subscribe(topic, type_name)
            subscribers.append(c)
            channels.append(ch)

        writer = BridgeClient(BRIDGE_ADDR)
        writer.connect()
        subscribers.append(writer)

        time.sleep(1.0)

        test_data = bytes([0x00, 0x01, 0x00, 0x00, 0xFA, 0xCE])
        writer.write(topic, type_name, test_data)

        for i, ch in enumerate(channels):
            msg = wait_for_message(ch)
            assert (
                msg.data == test_data
            ), f"sub[{i}] data mismatch: got {msg.data.hex()}, want {test_data.hex()}"

        print("PASS: test_multiple_clients")
    finally:
        for c in subscribers:
            c.close()


def test_unsubscribe():
    """Test that unsubscribe stops data delivery.

    Uses two clients: client1 unsubscribes, client2 stays subscribed
    to confirm data still flows through the bridge.
    """
    topic = "PyE2EUnsub"
    type_name = "test::Opaque"

    client1 = BridgeClient(BRIDGE_ADDR)
    client1.connect()
    client2 = BridgeClient(BRIDGE_ADDR)
    client2.connect()
    writer = BridgeClient(BRIDGE_ADDR)
    writer.connect()
    try:
        ch1 = client1.subscribe(topic, type_name)
        ch2 = client2.subscribe(topic, type_name)
        time.sleep(0.5)

        # Write first message - both should receive
        data1 = bytes([0x00, 0x01, 0x00, 0x00, 0x01])
        writer.write(topic, type_name, data1)
        msg1 = wait_for_message(ch1)
        assert msg1.data == data1
        msg2 = wait_for_message(ch2)
        assert msg2.data == data1

        # Client 1 unsubscribes
        client1.unsubscribe(topic, type_name)
        time.sleep(0.5)

        # Write second message - only client2 should receive
        data2 = bytes([0x00, 0x01, 0x00, 0x00, 0x02])
        writer.write(topic, type_name, data2)

        msg2 = wait_for_message(ch2)
        assert (
            msg2.data == data2
        ), f"client2 data mismatch: got {msg2.data.hex()}, want {data2.hex()}"

        # client1 should NOT have received
        time.sleep(0.5)
        assert len(ch1) == 0, f"client1 received {len(ch1)} messages after unsubscribe"
        print("PASS: test_unsubscribe")
    finally:
        client1.close()
        client2.close()
        writer.close()


def main():
    tests = [
        test_ping_pong,
        test_pubsub_loopback,
        test_create_writer_publish,
        test_dispose_loopback,
        test_multiple_clients,
        test_unsubscribe,
    ]

    passed = 0
    failed = 0
    for test in tests:
        try:
            test()
            passed += 1
        except Exception as e:
            print(f"FAIL: {test.__name__}: {e}")
            failed += 1

    print(f"\n{passed} passed, {failed} failed")
    if failed > 0:
        sys.exit(1)


if __name__ == "__main__":
    main()
