"""Multi-connection E2E tests."""

import struct
import time

from conftest import Client, MSG_DATA
from test_client import (
    MSG_SUBSCRIBE,
    MSG_UNSUBSCRIBE,
    MSG_WRITE,
    MSG_CREATE_WRITER,
    MSG_DELETE_WRITER,
    MSG_PING,
    MSG_OK,
    MSG_PONG,
    MSG_ERROR,
    make_subscribe_payload,
    make_unsubscribe_payload,
    make_write_topic_payload,
    make_write_writer_id_payload,
    make_create_writer_payload,
    make_delete_writer_payload,
    encode_header,
)


TYPE_NAME = "test::Opaque"


def test_fan_out_to_multiple_subscribers(make_client):
    topic = "PyTestFanOut"
    num_subs = 3

    subscribers = [make_client() for _ in range(num_subs)]
    writer = make_client()

    for sub in subscribers:
        payload = make_subscribe_payload(topic, TYPE_NAME)
        sub.send_and_wait(MSG_SUBSCRIBE, payload)

    time.sleep(1.0)

    test_data = bytes([0x00, 0x01, 0x00, 0x00, 0xFA, 0xCE])
    write_payload = make_write_topic_payload(topic, TYPE_NAME, test_data)
    writer.send_and_wait(MSG_WRITE, write_payload)

    for i, sub in enumerate(subscribers):
        msg = sub.wait_for_data(topic)
        assert msg.data == test_data, f"subscriber {i} data mismatch"


def test_unsubscribe_stops_delivery(make_client):
    topic = "PyTestUnsubMulti"

    sub1 = make_client()
    sub2 = make_client()
    writer = make_client()

    sub1.send_and_wait(MSG_SUBSCRIBE, make_subscribe_payload(topic, TYPE_NAME))
    sub2.send_and_wait(MSG_SUBSCRIBE, make_subscribe_payload(topic, TYPE_NAME))
    time.sleep(0.5)

    # Both receive first message
    data1 = bytes([0x00, 0x01, 0x00, 0x00, 0x01])
    writer.send_and_wait(MSG_WRITE, make_write_topic_payload(topic, TYPE_NAME, data1))
    sub1.wait_for_data(topic)
    sub2.wait_for_data(topic)

    # sub1 unsubscribes
    sub1.send_and_wait(MSG_UNSUBSCRIBE, make_unsubscribe_payload(topic, TYPE_NAME))
    time.sleep(0.5)

    # Write second message - only sub2 should receive
    data2 = bytes([0x00, 0x01, 0x00, 0x00, 0x02])
    writer.send_and_wait(MSG_WRITE, make_write_topic_payload(topic, TYPE_NAME, data2))
    msg = sub2.wait_for_data(topic)
    assert msg.data == data2

    # sub1 should not have any messages
    time.sleep(0.5)
    assert sub1.data_count() == 0


def test_writer_ownership_cross_session(make_client):
    topic = "PyTestOwnership"

    client_a = make_client()
    client_b = make_client()

    # Client A creates a writer
    cw_payload = make_create_writer_payload(topic, TYPE_NAME)
    _, resp = client_a.send_and_wait(MSG_CREATE_WRITER, cw_payload)
    writer_id = struct.unpack_from("<I", resp, 0)[0]

    # Client B tries to use client A's writer - should fail
    try:
        write_payload = make_write_writer_id_payload(
            writer_id, bytes([0x00, 0x01, 0x00, 0x00, 0xFF])
        )
        client_b.send_and_wait(MSG_WRITE, write_payload)
        assert False, "should have raised RuntimeError"
    except RuntimeError as e:
        assert "error" in str(e).lower()

    # Client B tries to delete client A's writer - should fail
    try:
        del_payload = make_delete_writer_payload(writer_id)
        client_b.send_and_wait(MSG_DELETE_WRITER, del_payload)
        assert False, "should have raised RuntimeError"
    except RuntimeError as e:
        assert "error" in str(e).lower()

    # Client A can still use and delete their own writer
    client_a.send_and_wait(MSG_DELETE_WRITER, make_delete_writer_payload(writer_id))


def test_writer_id_mode_pubsub(make_client):
    topic = "PyTestWriterIdMode"

    client = make_client()
    client.send_and_wait(MSG_SUBSCRIBE, make_subscribe_payload(topic, TYPE_NAME))

    cw_payload = make_create_writer_payload(topic, TYPE_NAME)
    _, resp = client.send_and_wait(MSG_CREATE_WRITER, cw_payload)
    writer_id = struct.unpack_from("<I", resp, 0)[0]

    time.sleep(0.5)

    test_data = bytes([0x00, 0x01, 0x00, 0x00, 0x11, 0x22, 0x33])
    write_payload = make_write_writer_id_payload(writer_id, test_data)
    client.send_and_wait(MSG_WRITE, write_payload)

    msg = client.wait_for_data(topic)
    assert msg.data == test_data

    client.send_and_wait(MSG_DELETE_WRITER, make_delete_writer_payload(writer_id))


def test_multiple_concurrent_pings(make_client):
    clients = [make_client() for _ in range(5)]
    for c in clients:
        resp_type, _ = c.send_and_wait(MSG_PING, b"")
        assert resp_type == MSG_PONG


def test_session_cleanup_on_disconnect(make_client):
    """Verify the bridge doesn't crash when a client disconnects abruptly."""
    topic = "PyTestCleanup"

    sub = make_client()
    sub.send_and_wait(MSG_SUBSCRIBE, make_subscribe_payload(topic, TYPE_NAME))

    writer = make_client()
    writer.send_and_wait(
        MSG_WRITE,
        make_write_topic_payload(
            topic, TYPE_NAME, bytes([0x00, 0x01, 0x00, 0x00, 0x01])
        ),
    )
    sub.wait_for_data(topic)

    # Disconnect the writer abruptly
    writer.close()
    time.sleep(1.0)

    # Bridge should still work - new writer can publish
    writer2 = make_client()
    data2 = bytes([0x00, 0x01, 0x00, 0x00, 0x02])
    writer2.send_and_wait(MSG_WRITE, make_write_topic_payload(topic, TYPE_NAME, data2))

    msg = sub.wait_for_data(topic)
    assert msg.data == data2
