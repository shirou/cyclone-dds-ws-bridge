"""Dispose E2E tests."""

import time

from conftest import Client, MSG_DATA, MSG_DATA_DISPOSED
from test_client import (
    MSG_SUBSCRIBE,
    MSG_WRITE,
    MSG_DISPOSE,
    MSG_WRITE_DISPOSE,
    MSG_OK,
    make_subscribe_payload,
    make_write_topic_payload,
    make_dispose_topic_payload,
)


TOPIC = "PyTestDispose"
TYPE_NAME = "test::Opaque"


def test_dispose_produces_data_disposed(client: Client):
    topic = "PyTestDisposeNotify"
    payload = make_subscribe_payload(topic, TYPE_NAME)
    client.send_and_wait(MSG_SUBSCRIBE, payload)
    time.sleep(0.5)

    # Write data first
    test_data = bytes([0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04])
    write_payload = make_write_topic_payload(topic, TYPE_NAME, test_data)
    client.send_and_wait(MSG_WRITE, write_payload)

    msg = client.wait_for_data(topic)
    assert msg.msg_type == MSG_DATA

    # Dispose
    key_data = bytes([0x00, 0x01, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04])
    dispose_payload = make_dispose_topic_payload(topic, TYPE_NAME, key_data)
    client.send_and_wait(MSG_DISPOSE, dispose_payload)

    msg = client.wait_for_data(topic)
    assert msg.msg_type == MSG_DATA_DISPOSED
    assert msg.topic_name == topic


def test_dispose_returns_ok(client: Client):
    key_data = bytes([0x00, 0x01, 0x00, 0x00, 0x01])
    dispose_payload = make_dispose_topic_payload(TOPIC, TYPE_NAME, key_data)
    resp_type, _ = client.send_and_wait(MSG_DISPOSE, dispose_payload)
    assert resp_type == MSG_OK
