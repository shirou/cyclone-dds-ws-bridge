"""Pytest fixtures for bridge tests."""

import time
import threading
import pytest
import websocket

import struct

from test_client import (
    BRIDGE_ADDR,
    HEADER_LEN,
    MAGIC,
    MSG_DATA,
    MSG_DATA_DISPOSED,
    MSG_DATA_NO_WRITERS,
    MSG_ERROR,
    decode_string,
    encode_header,
)


class DataMessage:
    def __init__(
        self, topic_name: str, data: bytes, msg_type: int, source_timestamp: int = 0
    ):
        self.topic_name = topic_name
        self.data = data
        self.msg_type = msg_type
        self.source_timestamp = source_timestamp


class Client:
    """Synchronous bridge client for testing."""

    def __init__(self, addr: str = BRIDGE_ADDR):
        self.addr = addr
        self._ws = None
        self._next_id = 1
        self._lock = threading.Lock()
        self._pending: dict[int, threading.Event] = {}
        self._responses: dict[int, tuple[int, bytes]] = {}
        self._sub_lock = threading.Lock()
        self._data_messages: list[DataMessage] = []
        self._recv_thread = None
        self._closed = False

    def connect(self, timeout: float = 30.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                self._ws = websocket.create_connection(self.addr, timeout=30)
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
            try:
                self._ws.close()
            except Exception:
                pass

    def _next_request_id(self) -> int:
        with self._lock:
            rid = self._next_id
            self._next_id += 1
            if self._next_id == 0:
                self._next_id = 1
            return rid

    def send_and_wait(
        self, msg_type: int, payload: bytes, timeout: float = 10.0
    ) -> tuple[int, bytes]:
        rid = self._next_request_id()
        evt = threading.Event()
        self._pending[rid] = evt
        msg = encode_header(msg_type, rid, payload) + payload
        self._ws.send_binary(msg)
        if not evt.wait(timeout):
            self._pending.pop(rid, None)
            raise TimeoutError(f"no response for request_id={rid}")
        self._pending.pop(rid, None)
        resp_type, resp_payload = self._responses.pop(rid)
        if resp_type == MSG_ERROR:
            code = struct.unpack_from("<H", resp_payload, 0)[0]
            msg_str, _ = decode_string(resp_payload, 2)
            raise RuntimeError(f"bridge error {code}: {msg_str}")
        return resp_type, resp_payload

    def send_raw(self, data: bytes):
        self._ws.send_binary(data)

    def _recv_loop(self):
        while not self._closed:
            try:
                _, data = self._ws.recv_data()
            except websocket.WebSocketTimeoutException:
                continue
            except Exception:
                break
            if len(data) < HEADER_LEN:
                continue
            if data[0:2] != MAGIC:
                continue
            msg_type = data[3]
            request_id = struct.unpack_from("<I", data, 4)[0]
            payload_len = struct.unpack_from("<I", data, 8)[0]
            payload = data[HEADER_LEN : HEADER_LEN + payload_len]

            if request_id != 0 and request_id in self._pending:
                self._responses[request_id] = (msg_type, payload)
                self._pending[request_id].set()
                continue

            if msg_type == MSG_DATA:
                topic, pos = decode_string(payload, 0)
                ts = struct.unpack_from("<Q", payload, pos)[0]
                pos += 8
                dm = DataMessage(topic, payload[pos:], MSG_DATA, ts)
                with self._sub_lock:
                    self._data_messages.append(dm)
            elif msg_type == MSG_DATA_DISPOSED:
                topic, pos = decode_string(payload, 0)
                ts = struct.unpack_from("<Q", payload, pos)[0]
                pos += 8
                dm = DataMessage(topic, payload[pos:], MSG_DATA_DISPOSED, ts)
                with self._sub_lock:
                    self._data_messages.append(dm)
            elif msg_type == MSG_DATA_NO_WRITERS:
                topic, pos = decode_string(payload, 0)
                dm = DataMessage(topic, payload[pos:], MSG_DATA_NO_WRITERS)
                with self._sub_lock:
                    self._data_messages.append(dm)

    def wait_for_data(self, topic: str = None, timeout: float = 10.0) -> DataMessage:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self._sub_lock:
                for i, dm in enumerate(self._data_messages):
                    if topic is None or dm.topic_name == topic:
                        return self._data_messages.pop(i)
            time.sleep(0.05)
        raise TimeoutError(f"no data message for topic={topic!r} within {timeout}s")

    def data_count(self) -> int:
        with self._sub_lock:
            return len(self._data_messages)


@pytest.fixture
def client():
    c = Client()
    c.connect()
    yield c
    c.close()


@pytest.fixture
def make_client():
    clients = []

    def _make():
        c = Client()
        c.connect()
        clients.append(c)
        return c

    yield _make
    for c in clients:
        c.close()
