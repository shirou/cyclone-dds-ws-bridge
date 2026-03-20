"""Shared test utilities for bridge Python tests.

Provides BridgeClient, message helpers, and test fixtures.
Reuses the E2E bridge_client module.
"""

import os
import sys
import struct
import time

# Bridge address from environment or default
BRIDGE_ADDR = os.environ.get("BRIDGE_ADDR", "ws://localhost:9876")

# Protocol constants
MAGIC = b"\x44\x42"
VERSION = 0x01
HEADER_LEN = 12

MSG_SUBSCRIBE = 0x01
MSG_UNSUBSCRIBE = 0x02
MSG_WRITE = 0x03
MSG_DISPOSE = 0x04
MSG_WRITE_DISPOSE = 0x05
MSG_CREATE_WRITER = 0x10
MSG_DELETE_WRITER = 0x11
MSG_PING = 0x0F

MSG_OK = 0x80
MSG_DATA = 0xC0
MSG_DATA_DISPOSED = 0xC1
MSG_DATA_NO_WRITERS = 0xC2
MSG_ERROR = 0xFE
MSG_PONG = 0x8F


def encode_header(msg_type: int, request_id: int, payload: bytes) -> bytes:
    return struct.pack("<2sBBII", MAGIC, VERSION, msg_type, request_id, len(payload))


def decode_header(data: bytes) -> tuple[int, int, int, int]:
    """Returns (version, msg_type, request_id, payload_length)."""
    _, version, msg_type, request_id, payload_length = struct.unpack_from("<2sBBII", data, 0)
    return version, msg_type, request_id, payload_length


def encode_string(s: str) -> bytes:
    encoded = s.encode("utf-8")
    return struct.pack("<I", len(encoded)) + encoded


def decode_string(data: bytes, offset: int) -> tuple[str, int]:
    length = struct.unpack_from("<I", data, offset)[0]
    offset += 4
    s = data[offset : offset + length].decode("utf-8")
    return s, offset + length


def encode_qos(qos: list | None = None) -> bytes:
    if not qos:
        return b"\x00"
    # policy_count + policy data
    buf = struct.pack("<B", len(qos))
    for policy in qos:
        buf += policy
    return buf


def encode_key_descriptors(keys: list | None = None) -> bytes:
    if not keys:
        return b"\x00"
    buf = struct.pack("<B", len(keys))
    for offset, size, type_hint in keys:
        buf += struct.pack("<IIB", offset, size, type_hint)
    return buf


def make_subscribe_payload(topic: str, type_name: str, qos=None, keys=None) -> bytes:
    return encode_string(topic) + encode_string(type_name) + encode_qos(qos) + encode_key_descriptors(keys)


def make_unsubscribe_payload(topic: str, type_name: str) -> bytes:
    return encode_string(topic) + encode_string(type_name)


def make_write_topic_payload(topic: str, type_name: str, data: bytes, qos=None, keys=None) -> bytes:
    return (
        b"\x00"
        + encode_string(topic)
        + encode_string(type_name)
        + encode_qos(qos)
        + encode_key_descriptors(keys)
        + data
    )


def make_write_writer_id_payload(writer_id: int, data: bytes) -> bytes:
    return b"\x01" + struct.pack("<I", writer_id) + data


def make_dispose_topic_payload(topic: str, type_name: str, key_data: bytes, qos=None, keys=None) -> bytes:
    return (
        b"\x00"
        + encode_string(topic)
        + encode_string(type_name)
        + encode_qos(qos)
        + encode_key_descriptors(keys)
        + key_data
    )


def make_create_writer_payload(topic: str, type_name: str, qos=None, keys=None) -> bytes:
    return encode_string(topic) + encode_string(type_name) + encode_qos(qos) + encode_key_descriptors(keys)


def make_delete_writer_payload(writer_id: int) -> bytes:
    return struct.pack("<I", writer_id)


def build_message(msg_type: int, request_id: int, payload: bytes) -> bytes:
    return encode_header(msg_type, request_id, payload) + payload
