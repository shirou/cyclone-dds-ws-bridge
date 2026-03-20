"""QoS-related E2E tests."""

import struct
import time

from conftest import Client, MSG_DATA
from test_client import (
    MSG_SUBSCRIBE,
    MSG_WRITE,
    MSG_OK,
    encode_string,
    encode_key_descriptors,
)


TYPE_NAME = "test::Opaque"


def _make_reliability_qos(kind: int, max_blocking_ms: int) -> list:
    """Build a binary QoS policy list with a single Reliability policy."""
    # policy_id=0x01, kind(u8), max_blocking_time_ms(u64)
    policy = struct.pack("<B", 0x01) + struct.pack("<B", kind) + struct.pack("<I", max_blocking_ms)
    return [policy]


def _make_durability_qos(kind: int) -> list:
    """Build a binary QoS policy list with a single Durability policy."""
    # policy_id=0x02, kind(u8)
    policy = struct.pack("<B", 0x02) + struct.pack("<B", kind)
    return [policy]


def _encode_qos_policies(policies: list) -> bytes:
    buf = struct.pack("<B", len(policies))
    for p in policies:
        buf += p
    return buf


def _make_subscribe_with_qos(topic: str, qos_policies: list) -> bytes:
    return (
        encode_string(topic)
        + encode_string(TYPE_NAME)
        + _encode_qos_policies(qos_policies)
        + encode_key_descriptors(None)
    )


def _make_write_with_qos(topic: str, data: bytes, qos_policies: list) -> bytes:
    return (
        b"\x00"
        + encode_string(topic)
        + encode_string(TYPE_NAME)
        + _encode_qos_policies(qos_policies)
        + encode_key_descriptors(None)
        + data
    )


def test_reliable_qos_pubsub(client: Client):
    topic = "PyTestQoSReliable"
    qos = _make_reliability_qos(kind=1, max_blocking_ms=100)

    sub_payload = _make_subscribe_with_qos(topic, qos)
    resp_type, _ = client.send_and_wait(MSG_SUBSCRIBE, sub_payload)
    assert resp_type == MSG_OK

    time.sleep(0.5)

    test_data = bytes([0x00, 0x01, 0x00, 0x00, 0xDE, 0xAD])
    write_payload = _make_write_with_qos(topic, test_data, qos)
    client.send_and_wait(MSG_WRITE, write_payload)

    msg = client.wait_for_data(topic)
    assert msg.data == test_data


def test_default_qos_pubsub(client: Client):
    topic = "PyTestQoSDefault"

    sub_payload = _make_subscribe_with_qos(topic, [])
    resp_type, _ = client.send_and_wait(MSG_SUBSCRIBE, sub_payload)
    assert resp_type == MSG_OK

    time.sleep(0.5)

    test_data = bytes([0x00, 0x01, 0x00, 0x00, 0xBE, 0xEF])
    write_payload = _make_write_with_qos(topic, test_data, [])
    client.send_and_wait(MSG_WRITE, write_payload)

    msg = client.wait_for_data(topic)
    assert msg.data == test_data
