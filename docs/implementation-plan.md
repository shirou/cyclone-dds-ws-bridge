# cyclone-dds-ws-bridge Implementation Plan

## Phase Overview

| Phase | Description | Deliverables |
|-------|-------------|--------------|
| 0 | DDS binding spike | bindgen PoC for Cyclone DDS C API + sertype |
| 1 | Project skeleton + protocol layer + config | Cargo.toml, protocol.rs, qos.rs, config.rs + unit tests |
| 2 | DDS layer (sertype, participant) | dds/ module |
| 3 | WebSocket server + session management | ws/ module |
| 4 | Bridge core (WS <-> DDS mediation) | bridge.rs, main.rs |
| 5 | Go client library | ddswsclient/ module (importable from Go) |
| 6 | Interop tests (Go <-> Python) | tests/interop/, docker-compose |
| 7 | Integration tests + polish | Rust integration tests, Python test client, CI, README |

---

## Phase 0: DDS Binding Spike -- COMPLETED

Validated that the Cyclone DDS C API can be used via `bindgen`.
Spike code is in `spike/` directory, runnable via `docker build spike/`.

- [x] Install Cyclone DDS (pin minimum version: **0.10.5**) and locate C headers
- [x] Set up `bindgen` to generate Rust FFI bindings for Cyclone DDS C API
- [x] Verify bindgen can parse `dds/dds.h` and `dds/ddsi/ddsi_sertype.h`
- [x] Implement minimal custom `ddsi_sertype` (identity serialize/deserialize)
- [x] Create a DomainParticipant, Topic, Writer, Reader using the generated bindings
- [x] Write a loopback test: write opaque bytes -> read them back
- [ ] Verify key field handling with a simple keyed topic (deferred to Phase 2)
- [x] Document Cyclone DDS version requirements and build constraints
- [x] Decide: bindgen approach works; `cyclors` crate not needed

### Spike Findings

**Build environment**: Cyclone DDS 0.10.5, built from source with cmake on
Debian bookworm (rust:1.90-bookworm Docker image). Requires `cmake`,
`libclang-dev`, `clang` for bindgen. Uses `pkg-config` to locate headers
and link libraries.

**Key technical details for Phase 2**:

1. **`ddsi_serdata_ref` / `ddsi_serdata_unref` are inline functions** in the
   Cyclone DDS headers -- bindgen does not generate them. Must be reimplemented
   manually using atomic operations on `ddsi_serdata.refc`
   (`ddsrt_atomic_uint32_t`, which is a simple `{ v: u32 }` struct).

2. **`DDS_DOMAIN_DEFAULT` is a C `#define`** -- not picked up by bindgen.
   Must be defined manually as `const DDS_DOMAIN_DEFAULT: u32 = 0xFFFFFFFF`.

3. **`ddsrt_msg_iovlen_t = usize`** (platform-dependent) -- the `from_ser_iov`
   callback takes `usize` for the iov count, not `u32`.

4. **`ddsrt_iovec_t = iovec`** (libc iovec) -- used in `to_ser_ref` / `to_ser_unref`.

5. **Static ops tables** (`ddsi_serdata_ops`, `ddsi_sertype_ops`) with `*mut c_void`
   fields require an `unsafe impl Sync` wrapper for Rust's safety rules since
   `*mut c_void` is not `Sync`.

6. **`ddsi_sertype_ops` fields** (Cyclone DDS 0.10.5):
   `version`, `arg`, `free`, `zero_samples`, `realloc_samples`, `free_samples`,
   `equal`, `hash`, `type_id`, `type_map`, `type_info`, `derive_sertype`,
   `get_serialized_size`, `serialize_into`.

7. **`ddsi_serdata_ops` fields** (Cyclone DDS 0.10.5):
   `eqkey`, `get_size`, `from_ser`, `from_ser_iov`, `from_keyhash`, `from_sample`,
   `to_ser`, `to_ser_ref`, `to_ser_unref`, `to_sample`, `to_untyped`,
   `untyped_to_sample`, `free`, `print`, `get_keyhash`.

8. **`nn_rdata` is opaque** (zero-size struct in bindings) -- the `from_ser` callback
   using fragchains is not usable directly. Use `from_ser_iov` instead (set
   `from_ser: None`).

9. **`SampleWrapper` pattern**: dds_write/dds_read use a `*const c_void` sample
   pointer. We define a `#[repr(C)] SampleWrapper { data: *const u8, len: usize }`
   that `from_sample` reads from and `to_sample` writes to.

---

## Phase 1: Project Skeleton + Protocol Layer + Config

### 1-1. Cargo.toml Setup

- [x] Create Cargo.toml with dependencies:
  - `tokio` (full) -- async runtime
  - `tokio-tungstenite` -- async WebSocket
  - `futures-util` -- Stream/Sink utilities
  - `serde`, `toml` -- configuration
  - `tracing`, `tracing-subscriber` -- logging
  - `bytes` -- buffer operations
  - `thiserror` -- error types
  - `bindgen` (build-dependency) -- Cyclone DDS C API bindings

### 1-2. config.rs -- Configuration (early, avoids hard-coded values)

- [x] `Config` struct (websocket, dds, logging sections)
- [x] Load from TOML file
- [x] Environment variable overrides:
  - [x] `DDS_BRIDGE_WS_ADDR`, `DDS_BRIDGE_WS_PORT`
  - [x] `DDS_BRIDGE_DDS_DOMAIN`
  - [x] `CYCLONEDDS_URI`
- [x] Default values
- [x] Configurable max_payload_size (default: 4 MiB)
- [x] Configurable session_buffer_size (default: 256)
- [ ] CLI argument: `--config <path>`
- [x] `config.example.toml`

### 1-3. protocol.rs -- Message Parse/Serialize

- [x] Header struct: `Header { magic, version, msg_type, request_id, payload_length }`
- [x] `MsgType` enum (SUBSCRIBE=0x01 .. PONG=0x8F)
- [x] Version check: reject messages with version > supported, return ERROR with details
- [x] Max payload size check
- [x] request_id convention: 0 reserved for unsolicited bridge messages
- [x] `parse_header(bytes) -> Result<Header>` -- parse 12-byte fixed header
- [x] `serialize_header(header) -> [u8; 12]` -- serialize header
- [x] Per-message payload parse functions:
  - [x] `SubscribePayload { topic_name, type_name, qos, key_descriptors }`
  - [x] `UnsubscribePayload { topic_name, type_name }`
  - [x] `WritePayload` (TopicMode / WriterIdMode)
  - [x] `DisposePayload` (TopicMode / WriterIdMode)
  - [x] `CreateWriterPayload`, `DeleteWriterPayload`
- [x] Response message serialization:
  - [x] `OkResponse`, `ErrorResponse`, `DataMessage`, `DataDisposedMessage`, etc.
- [x] Helpers: `read_string(buf)`, `write_string(buf, s)` (4-byte LE uint32 length + UTF-8)
- [x] `KeyDescriptors` parse/serialize (with STRING type_hint: CDR string format)
- [x] Unit test: header parse/serialize round-trip
- [x] Unit test: per-message-type parse/serialize round-trip
- [x] Unit test: malformed input error handling (bad magic, unsupported version, truncated, oversized)
- [x] Unit test: request_id=0 rejection for client messages

### 1-4. qos.rs -- QoS Binary Representation <-> DDS QoS Conversion

- [x] `QosPolicy` enum (13 QoS policy types)
- [x] `QosSet` -- policy collection with normalization (explicit defaults == implicit defaults)
- [x] `parse_qos(buf) -> Result<QosSet>` -- parse from binary
- [x] `serialize_qos(qos) -> Vec<u8>` -- serialize to binary
- [x] `to_dds_qos(qos_set) -> dds_qos_t` -- RAII `QosHandle` in `dds/qos_handle.rs` (prevents leaks)
- [x] `QosSet` equality: normalized comparison (explicitly-set default == omitted)
- [x] Unit test: empty QoS (policy_count=0)
- [x] Unit test: individual policy parse/serialize
- [x] Unit test: multiple policy combinations
- [x] Unit test: invalid policy_id error
- [x] Unit test: QoS equality with explicit defaults vs. omitted

### 1-5. Protocol Conformance Test Vectors

Shared binary test fixtures (`.bin` files) for the bridge protocol messages,
used by both Rust and Go implementations to ensure they produce identical
wire formats.

- [x] Generate `.bin` fixtures for each msg_type (header + payload)
- [x] Rust test: parse each fixture, verify fields
- [x] Rust test: serialize known values, compare byte-for-byte with fixture

---

## Phase 2: DDS Layer -- COMPLETED

### 2-1. dds/sertype.rs -- Opaque Sertype Implementation

- [x] Custom `ddsi_sertype` via bindgen (based on Phase 0 spike)
- [x] serialize/deserialize as identity transform (pass bytes through unchanged)
- [x] Configure key information from `KeyDescriptors`
- [x] STRING key type: read CDR string (uint32 length + chars + NUL) for key comparison
- [x] CDR endianness detection from XCDR2 encapsulation header
- [x] Properly wrap unsafe FFI with safe Rust API
- [x] Manage sertype lifecycle (free on drop)
- [x] Track Vec capacity separately in OpaqueSerdata for correct deallocation
- [x] Unit tests for key extraction (LE/BE), key equality, hashing

### 2-2. dds/participant.rs -- DomainParticipant Management

- [x] `DdsParticipant` struct -- holds a single DomainParticipant
- [x] `new(domain_id) -> Result<Self>`
- [x] `get_or_create_topic(topic_name, type_name, key_descriptors) -> Result<Topic>`
- [x] Cache and reuse topics with same (topic_name, type_name)
- [x] Log warning when same topic is requested with different key descriptors

### 2-3. dds/writer.rs -- DataWriter Management

- [x] `DdsWriter` struct
- [x] `new(participant_handle, topic, qos, qos_normalized) -> Result<Self>`
- [x] `write(data: &[u8]) -> Result<()>` -- dds_write
- [x] `dispose(key_data: &[u8]) -> Result<()>` -- dds_dispose
- [x] `write_dispose(data: &[u8]) -> Result<()>` -- dds_writedispose
- [x] `WriterRegistry` -- writer_id -> (DdsWriter, session_id) mapping
- [x] writer_id generated by global atomic counter
- [x] Per-client ownership: only the creating session can use/delete a writer
- [x] Implicit writers (from topic-name mode WRITE) tracked per session for cleanup
- [x] `find_by_topic` matches on (topic, type, normalized QoS) -- not just topic/type
- [x] Unit tests for registry operations (ownership, QoS matching, session cleanup)

### 2-4. dds/reader.rs -- DataReader Management

- [x] `DdsReader` struct
- [x] `new(participant_handle, topic, qos, qos_normalized) -> Result<Self>`
- [x] `take_samples(max_samples)` -- poll-based sample retrieval
- [ ] Register read callback, notify via bounded channel on data receipt (deferred to Phase 3/4)
- [x] `ReaderRegistry` -- share Reader for same (topic_name, type_name, normalized_qos)
- [x] Ref-counted via `subscribers.len()`; delete DDS Reader when last client leaves
- [x] Determine DATA / DATA_DISPOSED / DATA_NO_WRITERS from SampleInfo instance_state
- [x] `session_subscriptions` index for unsubscribe without QoS (protocol only sends topic+type)
- [x] Unit tests for registry operations (shared readers, unsubscribe, session cleanup)

### 2-5. dds/qos_handle.rs -- QoS RAII Wrapper

- [x] `QosHandle` wraps `*mut dds_qos_t` with `Drop` calling `dds_delete_qos`
- [x] `from_qos_set(QosSet) -> Option<QosHandle>` converts all 13 policy types
- [x] `make_qos()` helper returns `(Option<QosHandle>, *const dds_qos_t)` for DDS API calls

### 2-6. dds/error.rs -- Shared Error Types

- [x] `DdsError` enum covering participant, topic, writer, reader errors
- [x] Writer ownership violation error

### Phase 2 Implementation Notes

- **Dockerfile**: builds with Cyclone DDS 0.10.5 from source on `rust:1.90-bookworm`.
  86 tests pass (64 from Phase 1 + 22 new from Phase 2).
- **`SampleWrapper::from_bytes` is `unsafe`**: borrows a raw pointer into the caller's
  slice. Must not be stored across await points. Documented with safety contract.
- **`OpaqueSertype` uses `#[repr(C)]`** only for first-field casting guarantee.
  The `key_fields: Vec<KeyField>` field is not C-compatible but Cyclone DDS never
  accesses memory beyond the `ddsi_sertype` base.
- **Reader callback not yet wired**: `take_samples` is poll-based. The bridge event
  loop (Phase 3/4) will use DDS waitsets or listeners to trigger reads.

---

## Phase 3: WebSocket Server + Session Management -- COMPLETED

### 3-1. ws/server.rs -- WebSocket Server

- [x] `ws::server::start(bridge, addr, port, max_connections, shutdown) -> Result<()>`
- [x] tokio accept loop
- [x] Connection limit (`max_connections`) enforcement
- [x] Create Session per new connection
- [x] WS handshake in spawned task (non-blocking accept loop)

### 3-2. ws/session.rs -- Client Session Management

- [x] `run_session(bridge, ws, session_id)` -- runs session to completion
- [x] Bounded send buffer per session (configurable via config, default: 256 messages)
- [x] Backpressure: drop on overflow + log warning
- [x] Receive binary frame -> parse Header -> dispatch message
- [x] Reject client messages with request_id=0
- [x] Bridge notifications (DATA, etc.) -> send as binary frame with request_id=0
- [x] PING/PONG handling (WebSocket-level via tokio-tungstenite + application PING/PONG)
- [ ] Idle timeout management (deferred -- requires timer per session)
- [x] Cleanup on disconnect:
  - [x] Remove all subscriptions
  - [x] Delete all Writers (explicit and implicit)
  - [x] Delete Readers not shared with other clients

---

## Phase 4: Bridge Core -- COMPLETED

### 4-1. bridge.rs -- WS <-> DDS Mediation

- [x] `BridgeInner` struct -- holds DdsParticipant, WriterRegistry, ReaderRegistry
- [x] `Bridge` type alias: `Arc<Mutex<BridgeInner>>`
- [x] Request handlers:
  - [x] `handle_subscribe` -- create or share Reader + register session for fan-out
  - [x] `handle_unsubscribe` -- requires (topic_name, type_name) to identify subscription
  - [x] `handle_write` -- topic-name mode: per-session writer lookup/create; writer-id mode: ownership check
  - [x] `handle_dispose` -- dds_dispose
  - [x] `handle_write_dispose` -- dds_writedispose
  - [x] `handle_create_writer` -> writer_id (associated with session)
  - [x] `handle_delete_writer` -- ownership check before deletion
- [x] Fan-out DDS Reader data to subscribed sessions (via bounded channels)
- [x] Correlate request_id (request -> OK/ERROR response)
- [x] Reader poll loop releases mutex before fan-out (minimizes contention)

### 4-2. main.rs

- [x] Load configuration (with `--config <path>` CLI argument)
- [x] Initialize tracing (with env filter support)
- [x] Create Bridge
- [x] Start WsServer
- [x] Graceful shutdown (SIGINT/SIGTERM via ctrl_c + broadcast channel)

---

## Phase 5: Go Client Library -- COMPLETED

A Go package importable as:

```go
import "github.com/shirou/cyclone-dds-ws-bridge/ddswsclient"
```

### 5-1. ddswsclient -- Protocol + Client

- [x] `protocol.go` -- message header and payload encode/decode
  - [x] Header struct, MsgType constants
  - [x] Version check matching bridge behavior
  - [x] Encode/decode helpers for each message type
  - [x] String encoding: 4-byte LE uint32 length + UTF-8
  - [x] QoS and KeyDescriptors binary representation
  - [x] KeyDescriptors builder helper: given field offsets and types, construct binary representation
- [x] `client.go` / `conn.go` -- WebSocket client (two-layer design)
  - [x] `Client` struct with auto-reconnect (high-level)
  - [x] `Conn` struct for low-level WebSocket + protocol framing
  - [x] `NewClient(ctx, addr, opts...) -> (*Client, error)`
  - [x] `Subscribe(topic, typeName, qos, keyDesc) -> (*Subscription, error)` (channel-based)
  - [x] `Subscription.Unsubscribe(ctx) -> error`
  - [x] `Write(topic, typeName, qos, keyDesc, data) -> error` (topic-name mode)
  - [x] `Writer.Publish(data) -> error` (writer-id mode, fire-and-forget)
  - [x] `CreateWriter(topic, typeName, qos, keyDesc) -> (*Writer, error)`
  - [x] `Writer.Close(ctx) -> error`
  - [x] `Dispose(topic, typeName, qos, keyDesc, keyData) -> error` (topic-name mode)
  - [x] `Writer.PublishDispose(keyData) -> error` (writer-id mode)
  - [x] `WriteDispose(topic, typeName, qos, keyDesc, data) -> error` (topic-name mode)
  - [x] `Writer.PublishWriteDispose(data) -> error` (writer-id mode)
  - [x] `Ping() -> error` / `Close() -> error`
  - [x] `OnConnect` / `OnDisconnect` / `OnError` lifecycle handlers
- [x] `client_test.go` -- protocol encode/decode round-trip tests (31 tests)
- [x] `go.mod` -- module `github.com/shirou/cyclone-dds-ws-bridge/ddswsclient`
- [x] Thread-safe: mutex-protected writes, atomic request_id counter, RWMutex for subscriptions
- [x] request_id tracking: auto-assign unique IDs (never 0), correlate OK/ERROR responses via sync.Map
- [x] Protocol conformance: verify against shared `.bin` test vectors from Phase 1-5 (15 fixture tests)

### 5-2. Example

- [x] `example_test.go` -- usage example showing subscribe, create writer, publish, receive

Design notes:
- Zero dependency on `go-dds-idlgen`; the client only deals with opaque `[]byte` payloads
- Callers use `go-dds-idlgen` generated types to MarshalCDR/UnmarshalCDR the payloads themselves
- KeyDescriptors must be constructed by the caller (go-dds-idlgen does not generate them yet;
  a future go-dds-idlgen extension could auto-generate KeyDescriptors from `@key` annotations)
- Two-layer architecture: `Conn` (low-level protocol) + `Client` (auto-reconnect, subscription/writer management)
- Subscriptions and Writers survive reconnections transparently
- Backpressure via drop-oldest on bounded subscription channels (256 buffer)

### Phase 5 Implementation Notes

- **31 Go tests pass** (16 round-trip + 15 fixture conformance).
- **Auto-reconnect**: `Client` monitors the connection and transparently re-creates
  all writers and re-subscribes all subscriptions on reconnect.
- **Concurrency safety fix**: `Subscription` uses `atomic.Bool` (`unsubscribed`) to
  prevent send-on-closed-channel panic when `Unsubscribe` or `Client.Close` races with
  `deliverData`.
- **Payload length validation**: `recvLoop` validates `hdr.PayloadLength` against actual
  data length to prevent reading beyond message boundaries.

---

## Phase 6: Interop Tests (Go <-> Python)

Validates CDR byte-level compatibility between Go (`go-dds-idlgen/cdr`) and Python (`pycdr2`).
Since the bridge passes opaque byte payloads, CDR compatibility is the prerequisite for
cross-language communication through the bridge.

### 6-1. Level 1 -- CDR Byte Compatibility (no bridge required)

Fixture-based static tests using shared IDL definitions from `idl/`.
Go types are **generated by `go-dds-idlgen`** from the IDL (not hand-written)
to validate the actual generator output.

- [x] `tests/interop/python/generate_fixtures.py` -- serialize test data with pycdr2
- [x] `tests/interop/python/verify_go_fixtures.py` -- deserialize Go-generated .bin with pycdr2
- [x] `tests/interop/python/requirements.txt`
- [x] `tests/interop/python/idl_types.py` -- shared IDL type definitions
- [x] `tests/interop/python/Dockerfile` -- Python test runner image
- [x] `tests/interop/go/generate.go` -- go:generate directive to run go-dds-idlgen on IDL
- [x] `tests/interop/go/interop_test.go` -- decode Python fixtures, encode Go fixtures, byte-exact comparison
- [x] `tests/interop/go/Dockerfile` -- Go test runner image
- [x] `tests/interop/run_interop_tests.sh` -- orchestrates the full pipeline
- [x] `docker-compose.test.yml` -- docker-compose orchestration for CI
- [x] Test data: `sensor::SensorData`
- [x] Test data: `interop::PrimitiveTypes` (boundary values, zero values)
- [x] Test data: `interop::KeyedMessage`
- [x] Test data: `interop::WithSequence` (non-empty and empty)
- [x] Test data: `interop::WithEnum`

### 6-2. Level 2 -- Bridge E2E via docker-compose

End-to-end tests through the running bridge, orchestrated by docker-compose.

Services:

```yaml
# docker-compose.test.yml
services:
  bridge:
    build: .
    # Cyclone DDS configured for localhost-only (no multicast)
    environment:
      - CYCLONEDDS_URI=<inline XML: localhost-only, no multicast>
      - DDS_BRIDGE_WS_ADDR=0.0.0.0
      - DDS_BRIDGE_WS_PORT=9876
    ports:
      - "9876:9876"

  go-test:
    build:
      context: .
      dockerfile: tests/interop/go/Dockerfile
    depends_on:
      - bridge
    environment:
      - BRIDGE_ADDR=ws://bridge:9876

  python-test:
    build:
      context: .
      dockerfile: tests/interop/python/Dockerfile
    depends_on:
      - bridge
    environment:
      - BRIDGE_ADDR=ws://bridge:9876
```

- [x] `docker-compose.test.yml` with bridge, go-e2e, py-e2e services (profile: `e2e`)
- [x] Cyclone DDS XML config for localhost-only (inline in docker-compose, disables multicast)
- [x] `tests/e2e/go/Dockerfile` -- Go E2E test runner image
- [x] `tests/e2e/python/Dockerfile` -- Python E2E test runner image
- [x] `tests/e2e/python/bridge_client.py` -- minimal Python bridge protocol client
- [x] Test: Go client pub/sub loopback through bridge (WRITE -> DATA)
- [x] Test: Python client pub/sub loopback through bridge (WRITE -> DATA)
- [x] Test: Writer-id mode (CREATE_WRITER -> Publish -> DATA)
- [x] Test: Dispose propagation (WRITE -> DATA, DISPOSE -> DATA_DISPOSED)
- [x] Test: Multiple concurrent clients with fan-out
- [x] Test: Unsubscribe stops data delivery (verified with control subscriber)
- [x] Test: PING/PONG connectivity
- [x] Orchestration script: `tests/e2e/run_e2e_tests.sh`
- [x] Dockerfile runtime stage (multi-stage build for bridge binary)

Depends on: Phase 4 (bridge core) + Phase 5 (Go client library)

---

## Phase 7: Integration Tests + Polish -- COMPLETED

### 7-1. Rust Integration Tests

- [x] `tests/protocol_test.rs` -- protocol integration test (full message round-trips, all error codes, QoS, Unicode, edge cases)
- [x] `tests/integration_pubsub.rs` -- E2E with Cyclone DDS loopback (in-process bridge)
  - [x] WRITE -> SUBSCRIBE -> DATA round-trip
  - [x] DISPOSE propagation
  - [x] Multiple concurrent client connections with fan-out
  - [x] Writer-id mode (CREATE_WRITER -> write -> DATA)
  - [x] Writer ownership: client A cannot use client B's writer_id
  - [x] Unsubscribe stops delivery (verified with control subscriber)
  - [x] Invalid message returns ERROR
  - [x] request_id=0 rejected

### 7-2. Python Test Client

- [x] `tests/client/test_client.py` -- shared utilities (connect, message build/parse)
- [x] `tests/client/conftest.py` -- pytest fixtures (Client class, make_client factory)
- [x] `tests/client/test_pubsub.py` -- pub/sub E2E (loopback, multiple messages, large payload)
- [x] `tests/client/test_dispose.py` -- dispose test (DATA_DISPOSED notification)
- [x] `tests/client/test_qos.py` -- QoS test (reliable, default)
- [x] `tests/client/test_multiconn.py` -- multi-connection test (fan-out, unsubscribe, writer ownership, cleanup)
- [x] `tests/client/Dockerfile` -- Python test runner image
- [x] `tests/client/requirements.txt`

### 7-3. CI

- [x] `Dockerfile` -- bridge build image with unit + integration tests
- [x] `docker-compose.test.yml` -- full test orchestration (interop, e2e, client profiles)
- [x] `.github/workflows/ci.yml` -- GitHub Actions workflow:
  - [x] Rust build + unit tests (Docker)
  - [x] Go client unit tests
  - [x] CDR interop tests (Go <-> Python)
  - [x] Bridge E2E tests (Go + Python clients)
  - [x] Python client tests (pytest)

### 7-4. Documentation

- [x] `README.md` -- build instructions, usage, protocol overview, Go client example, testing
- [x] `LICENSE` (Apache-2.0)
- [x] `docs/protocol.md` -- WebSocket protocol specification
- [x] `docs/implementation-plan.md` -- this file

---

## Risks and Mitigations

### Cyclone DDS bindgen feasibility (HIGHEST RISK)
- `ddsi_sertype` is an internal Cyclone DDS struct; its layout may not be
  stable across versions and may require specific header paths for bindgen.
- Different distro packages may ship different header sets.
- **Mitigation**: Phase 0 spike validates this before any other work begins.
  Pin Cyclone DDS >= 0.10.x. If bindgen fails, fall back to `cyclors` crate.

### KeyDescriptors construction burden on clients
- Clients must manually compute byte offsets for key fields, accounting for
  CDR alignment and XCDR2 encapsulation header.
- For STRING keys, offset/size semantics differ from fixed types.
- **Mitigation**: provide helper functions in Go client library (Phase 5).
  Document offset calculation rules clearly in protocol.md. Future:
  extend go-dds-idlgen to auto-generate KeyDescriptors from `@key` annotations.

### DDS networking in Docker
- Cyclone DDS defaults to multicast for RTPS discovery, which doesn't work
  in standard Docker bridge networks.
- **Mitigation**: use Cyclone DDS XML config with localhost-only / shared-memory
  transport for testing. Document in docker-compose setup.

### Multi-client fan-out performance
- Potential bottleneck with many clients + high-frequency data.
- **Mitigation**: bounded channel per session with drop-oldest policy;
  start simple, optimize after profiling.

### Writer reuse in topic-name mode
- Correct equality check for (topic, type, qos) with per-session scoping.
- **Mitigation**: normalized QoS comparison (Phase 1-4); writers are per-session,
  never shared across clients.

### Protocol divergence between Rust and Go
- Two implementations of the same binary protocol can drift.
- **Mitigation**: shared `.bin` test vectors (Phase 1-5) that both
  implementations must pass; any protocol change requires updating fixtures first.

---

## Recommended Implementation Order

```
Phase 0   (bindgen spike)    --verified-->     ✅ DONE
Phase 1-2 (config)           --done-->         ✅ DONE
Phase 1-3 (protocol.rs)     --tests pass-->    ✅ DONE
Phase 1-4 (qos.rs)          --tests pass-->    ✅ DONE
Phase 1-5 (test vectors)    --generated-->     ✅ DONE
Phase 2   (DDS layer)       --86 tests pass--> ✅ DONE
Phase 3   (WS layer)        --compiles-->      ✅ DONE
Phase 4   (bridge + main)   --compiles-->      ✅ DONE
Phase 5   (Go client lib)   --31 tests pass--> ✅ DONE
Phase 6-1 (interop L1)      --32 Go + 8 Py tests pass-->  ✅ DONE
Phase 6-2 (interop L2)      --6 Go + 6 Py E2E tests-->    ✅ DONE
Phase 7   (integration + CI + polish)   ✅ DONE
```

Phases 0-5 are complete (86 Rust + 31 Go unit tests pass).
Phase 6-1 is complete (32 Go + 8 Python interop tests pass, byte-exact CDR match verified).
Phase 6-2 is complete (6 Go + 6 Python E2E tests via docker-compose).
Phase 7 is complete (Rust integration tests, Python pytest client, GitHub Actions CI, README).

---

## Design Decisions Log

### Removed: CREATE_READER / DELETE_READER

The original design had CREATE_READER (0x12) and DELETE_READER (0x13) to
separate reader creation from subscription start. This was removed because:

1. SUBSCRIBE already creates a reader (or reuses a shared one).
2. There was no way to associate a SUBSCRIBE with a pre-created reader
   (SUBSCRIBE doesn't take a reader_id).
3. Matching by (topic, type, qos) makes CREATE_READER + SUBSCRIBE equivalent
   to just SUBSCRIBE.

If a "pre-warm reader" use case arises, it can be added in a future protocol
version with a clear reader_id-based SUBSCRIBE variant.

### Writers are per-session, Readers are shared

- **Writers**: each client session owns its writers (both explicit via
  CREATE_WRITER and implicit via topic-name-mode WRITE). Writers are never
  shared across sessions. Cleanup on disconnect deletes all writers.
- **Readers**: shared across sessions by (topic_name, type_name, normalized_qos).
  Reference-counted. This avoids duplicate DDS readers for the same data.

### UNSUBSCRIBE requires type_name

UNSUBSCRIBE takes both topic_name and type_name (not just topic_name) to
uniquely identify the subscription. A client may subscribe to the same
topic_name with different type_names (e.g., different IDL versions).

---

## Repository Structure

```
cyclone-dds-ws-bridge/
├── Cargo.toml
├── build.rs                    # bindgen for Cyclone DDS C API
├── wrapper.h                   # C header wrapper for bindgen
├── LICENSE                     # Apache-2.0
├── README.md
├── docker-compose.test.yml     # test orchestration (Phase 6)
├── Dockerfile                  # bridge build (Cyclone DDS 0.10.5)
├── docs/
│   ├── implementation-plan.md  # this file
│   └── protocol.md             # WebSocket protocol specification
├── idl/                        # shared IDL definitions
│   ├── sensor.idl
│   └── interop.idl
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── config.rs
│   ├── protocol.rs
│   ├── qos.rs
│   ├── bridge.rs               # Phase 4
│   ├── dds/
│   │   ├── mod.rs
│   │   ├── error.rs
│   │   ├── participant.rs
│   │   ├── sertype.rs
│   │   ├── writer.rs
│   │   ├── reader.rs
│   │   └── qos_handle.rs
│   └── ws/                     # Phase 3
│       ├── mod.rs
│       ├── server.rs
│       └── session.rs
├── ddswsclient/                # Go client library (Phase 5)
│   ├── go.mod
│   ├── protocol.go
│   ├── client.go
│   └── client_test.go
├── tests/
│   ├── fixtures/               # shared protocol test vectors (.bin)
│   ├── protocol_test.rs
│   ├── integration/
│   │   └── pubsub_test.rs
│   ├── client/                 # Python test client
│   │   ├── test_client.py
│   │   └── README.md
│   ├── e2e/                    # Bridge E2E tests (Phase 6-2)
│   │   ├── run_e2e_tests.sh
│   │   ├── go/
│   │   │   ├── Dockerfile
│   │   │   ├── go.mod
│   │   │   └── e2e_test.go
│   │   └── python/
│   │       ├── Dockerfile
│   │       ├── requirements.txt
│   │       ├── bridge_client.py
│   │       └── e2e_test.py
│   └── interop/                # Go <-> Python CDR interop tests
│       ├── run_interop_tests.sh
│       ├── python/
│       │   ├── Dockerfile
│       │   ├── requirements.txt
│       │   ├── generate_fixtures.py
│       │   └── verify_go_fixtures.py
│       └── go/
│           ├── Dockerfile
│           ├── go.mod
│           ├── generate.go     # go:generate -> go-dds-idlgen
│           └── interop_test.go
└── config.example.toml
```
