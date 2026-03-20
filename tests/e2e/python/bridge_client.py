"""Minimal Python client for the cyclone-dds-ws-bridge protocol."""

import struct
import threading
import time
from collections import defaultdict

import websocket

MAGIC = b"\x44\x42"
VERSION = 0x01
HEADER_LEN = 12

# Message types
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

# Instance states
STATE_ALIVE = 0
STATE_DISPOSED = 1
STATE_NO_WRITERS = 2


class DataMessage:
    def __init__(self, topic_name: str, data: bytes, state: int, source_timestamp: int = 0):
        self.topic_name = topic_name
        self.data = data
        self.state = state
        self.source_timestamp = source_timestamp


def _encode_header(msg_type: int, request_id: int, payload: bytes) -> bytes:
    return struct.pack("<2sBBII", MAGIC, VERSION, msg_type, request_id, len(payload))


def _encode_string(s: str) -> bytes:
    encoded = s.encode("utf-8")
    return struct.pack("<I", len(encoded)) + encoded


def _decode_string(data: bytes, offset: int) -> tuple[str, int]:
    length = struct.unpack_from("<I", data, offset)[0]
    offset += 4
    s = data[offset : offset + length].decode("utf-8")
    return s, offset + length


def _encode_qos(qos: list | None) -> bytes:
    if not qos:
        return b"\x00"
    raise NotImplementedError("QoS encoding not implemented for E2E tests")


def _encode_key_descriptors(keys: list | None) -> bytes:
    if not keys:
        return b"\x00"
    raise NotImplementedError("Key descriptor encoding not implemented for E2E tests")


class BridgeClient:
    """Synchronous bridge client for E2E testing."""

    def __init__(self, addr: str):
        self.addr = addr
        self._ws = None
        self._next_id = 1
        self._lock = threading.Lock()
        self._pending: dict[int, threading.Event] = {}
        self._responses: dict[int, tuple[int, bytes]] = {}
        self._sub_lock = threading.Lock()
        self._subscriptions: dict[str, list] = defaultdict(list)
        self._recv_thread = None
        self._closed = False

    def connect(self, timeout: float = 30.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                self._ws = websocket.create_connection(self.addr, timeout=5)
                break
            except Exception:
                time.sleep(0.5)
        else:
            raise ConnectionError(f"failed to connect to {self.addr} within {timeout}s")

        self._recv_thread = threading.Thread(target=self._recv_loop, daemon=True)
        self._recv_thread.start()

    def close(self):
        self._closed = True
        if self._ws:
            self._ws.close()

    def _next_request_id(self) -> int:
        with self._lock:
            rid = self._next_id
            self._next_id += 1
            if self._next_id == 0:
                self._next_id = 1
            return rid

    def _send_and_wait(self, msg_type: int, payload: bytes, timeout: float = 10.0) -> tuple[int, bytes]:
        rid = self._next_request_id()
        evt = threading.Event()
        self._pending[rid] = evt

        msg = _encode_header(msg_type, rid, payload) + payload
        self._ws.send_binary(msg)

        if not evt.wait(timeout):
            self._pending.pop(rid, None)
            raise TimeoutError(f"no response for request_id={rid}")

        self._pending.pop(rid, None)
        resp_type, resp_payload = self._responses.pop(rid)
        if resp_type == MSG_ERROR:
            code = struct.unpack_from("<H", resp_payload, 0)[0]
            msg_str, _ = _decode_string(resp_payload, 2)
            raise RuntimeError(f"bridge error {code}: {msg_str}")
        return resp_type, resp_payload

    def _recv_loop(self):
        while not self._closed:
            try:
                _, data = self._ws.recv_data()
            except Exception:
                break
            if len(data) < HEADER_LEN:
                continue
            magic = data[0:2]
            if magic != MAGIC:
                continue
            msg_type = data[3]
            request_id = struct.unpack_from("<I", data, 4)[0]
            payload_len = struct.unpack_from("<I", data, 8)[0]
            payload = data[HEADER_LEN : HEADER_LEN + payload_len]

            # Correlated response
            if request_id != 0 and request_id in self._pending:
                self._responses[request_id] = (msg_type, payload)
                self._pending[request_id].set()
                continue

            # Unsolicited data
            if msg_type == MSG_DATA:
                topic, pos = _decode_string(payload, 0)
                ts = struct.unpack_from("<Q", payload, pos)[0]
                pos += 8
                sample_data = payload[pos:]
                dm = DataMessage(topic, sample_data, STATE_ALIVE, ts)
                with self._sub_lock:
                    for ch in self._subscriptions.get(topic, []):
                        ch.append(dm)
            elif msg_type == MSG_DATA_DISPOSED:
                topic, pos = _decode_string(payload, 0)
                ts = struct.unpack_from("<Q", payload, pos)[0]
                pos += 8
                key_data = payload[pos:]
                dm = DataMessage(topic, key_data, STATE_DISPOSED, ts)
                with self._sub_lock:
                    for ch in self._subscriptions.get(topic, []):
                        ch.append(dm)
            elif msg_type == MSG_DATA_NO_WRITERS:
                topic, pos = _decode_string(payload, 0)
                key_data = payload[pos:]
                dm = DataMessage(topic, key_data, STATE_NO_WRITERS)
                with self._sub_lock:
                    for ch in self._subscriptions.get(topic, []):
                        ch.append(dm)

    def ping(self):
        resp_type, _ = self._send_and_wait(MSG_PING, b"")
        assert resp_type == MSG_PONG, f"expected PONG, got 0x{resp_type:02x}"

    def subscribe(self, topic: str, type_name: str) -> list:
        payload = _encode_string(topic) + _encode_string(type_name) + _encode_qos(None) + _encode_key_descriptors(None)
        self._send_and_wait(MSG_SUBSCRIBE, payload)
        ch: list[DataMessage] = []
        with self._sub_lock:
            self._subscriptions[topic].append(ch)
        return ch

    def unsubscribe(self, topic: str, type_name: str):
        payload = _encode_string(topic) + _encode_string(type_name)
        self._send_and_wait(MSG_UNSUBSCRIBE, payload)
        with self._sub_lock:
            self._subscriptions.pop(topic, None)

    def write(self, topic: str, type_name: str, data: bytes):
        payload = (
            b"\x00"  # topic-name mode
            + _encode_string(topic)
            + _encode_string(type_name)
            + _encode_qos(None)
            + _encode_key_descriptors(None)
            + data
        )
        self._send_and_wait(MSG_WRITE, payload)

    def dispose(self, topic: str, type_name: str, key_data: bytes):
        payload = (
            b"\x00"  # topic-name mode
            + _encode_string(topic)
            + _encode_string(type_name)
            + _encode_qos(None)
            + _encode_key_descriptors(None)
            + key_data
        )
        self._send_and_wait(MSG_DISPOSE, payload)

    def create_writer(self, topic: str, type_name: str) -> int:
        payload = _encode_string(topic) + _encode_string(type_name) + _encode_qos(None) + _encode_key_descriptors(None)
        _, resp = self._send_and_wait(MSG_CREATE_WRITER, payload)
        return struct.unpack_from("<I", resp, 0)[0]

    def delete_writer(self, writer_id: int):
        payload = struct.pack("<I", writer_id)
        self._send_and_wait(MSG_DELETE_WRITER, payload)

    def write_with_writer(self, writer_id: int, data: bytes):
        """Fire-and-forget write via writer-id mode."""
        rid = self._next_request_id()
        payload = b"\x01" + struct.pack("<I", writer_id) + data
        msg = _encode_header(MSG_WRITE, rid, payload) + payload
        self._ws.send_binary(msg)


def wait_for_message(ch: list, timeout: float = 10.0) -> DataMessage:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if ch:
            return ch.pop(0)
        time.sleep(0.05)
    raise TimeoutError("no message received within timeout")
