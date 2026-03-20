"""Pub/sub E2E tests."""

import time

from conftest import Client, MSG_DATA
from test_client import (
    MSG_SUBSCRIBE,
    MSG_WRITE,
    MSG_OK,
    make_subscribe_payload,
    make_write_topic_payload,
)


TOPIC = "PyTestPubSub"
TYPE_NAME = "test::Opaque"


def test_subscribe_returns_ok(client: Client):
    payload = make_subscribe_payload(TOPIC, TYPE_NAME)
    resp_type, _ = client.send_and_wait(MSG_SUBSCRIBE, payload)
    assert resp_type == MSG_OK


def test_write_returns_ok(client: Client):
    data = bytes([0x00, 0x01, 0x00, 0x00, 0x42])
    payload = make_write_topic_payload(TOPIC, TYPE_NAME, data)
    resp_type, _ = client.send_and_wait(MSG_WRITE, payload)
    assert resp_type == MSG_OK


def test_pubsub_loopback(client: Client):
    topic = "PyTestLoopback"
    payload = make_subscribe_payload(topic, TYPE_NAME)
    client.send_and_wait(MSG_SUBSCRIBE, payload)
    time.sleep(0.5)

    test_data = bytes([0x00, 0x01, 0x00, 0x00, 0xAA, 0xBB, 0xCC, 0xDD])
    write_payload = make_write_topic_payload(topic, TYPE_NAME, test_data)
    client.send_and_wait(MSG_WRITE, write_payload)

    msg = client.wait_for_data(topic)
    assert msg.msg_type == MSG_DATA
    assert msg.topic_name == topic
    assert msg.data == test_data


def test_pubsub_multiple_messages(client: Client):
    topic = "PyTestMultiMsg"
    payload = make_subscribe_payload(topic, TYPE_NAME)
    client.send_and_wait(MSG_SUBSCRIBE, payload)
    time.sleep(0.5)

    messages = []
    for i in range(5):
        data = bytes([0x00, 0x01, 0x00, 0x00]) + bytes([i])
        write_payload = make_write_topic_payload(topic, TYPE_NAME, data)
        client.send_and_wait(MSG_WRITE, write_payload)
        messages.append(data)

    for expected in messages:
        msg = client.wait_for_data(topic)
        assert msg.data == expected


def test_pubsub_large_payload(client: Client):
    topic = "PyTestLargePayload"
    payload = make_subscribe_payload(topic, TYPE_NAME)
    client.send_and_wait(MSG_SUBSCRIBE, payload)
    time.sleep(0.5)

    # XCDR2 header + large payload
    large_data = bytes([0x00, 0x01, 0x00, 0x00]) + bytes(range(256)) * 100
    write_payload = make_write_topic_payload(topic, TYPE_NAME, large_data)
    client.send_and_wait(MSG_WRITE, write_payload)

    msg = client.wait_for_data(topic)
    assert msg.data == large_data
