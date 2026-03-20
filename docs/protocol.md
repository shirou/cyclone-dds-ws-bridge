# WebSocket Protocol Specification

## Transport

- WebSocket **Binary frames** only (no Text frames)
- Header fields are **Little-endian** fixed
- Payload bytes are passed through transparently (no interpretation by the bridge)

## Message Structure

Every message (both directions) starts with a **12-byte fixed header**:

```
offset  size  field
──────────────────────────────────
0       2     magic: 0x44 0x42 ("DB" = DDS Bridge)
2       1     version: 0x01
3       1     msg_type
4       4     request_id (LE uint32) -- correlates requests with responses
8       4     payload_length (LE uint32)
──────────────────────────────────
12      N     payload (payload_length bytes)
```

### request_id Conventions

- **Client -> Bridge**: client assigns a unique request_id per request.
  The bridge echoes it in the corresponding OK or ERROR response.
- **Bridge -> Client (unsolicited)**: request_id is `0` for DATA, DATA_DISPOSED,
  DATA_NO_WRITERS, and BUFFER_OVERFLOW messages. Clients must not use
  request_id `0` for their own requests.

### Maximum Message Size

The bridge enforces a configurable maximum payload size (default: 4 MiB).
Messages exceeding this limit are rejected with ERROR (INVALID_MESSAGE).

## Version Negotiation

The `version` field in the header indicates the **minimum protocol version**
required to process the message. The bridge checks this field on every incoming
message:

- If `version <= bridge_max_supported_version`, the message is processed normally.
- If `version > bridge_max_supported_version`, the bridge responds with
  ERROR (INVALID_MESSAGE) including a human-readable message such as
  `"unsupported protocol version 3; bridge supports up to version 1"`,
  and the connection is maintained.

Clients should set `version` to the minimum version that defines the msg_type
they are sending (currently always `0x01`). The bridge sets `version` to the
minimum version required to interpret its response.

Future protocol versions may add new msg_types or extend existing payloads.
Messages defined in version 1 will remain valid and unchanged in later versions.

## String Encoding

All strings in payloads use a unified format: **4-byte LE length prefix + UTF-8 bytes**.
Abbreviated as `string` in definitions below.

```
offset  size  field
─────────────────────
0       4     length (LE uint32) -- byte length of UTF-8 data
4       N     UTF-8 bytes (N = length)
```

## Message Types

### Client -> Bridge

| msg_type | Name | Description |
|----------|------|-------------|
| 0x01 | SUBSCRIBE | Start topic subscription |
| 0x02 | UNSUBSCRIBE | Stop topic subscription |
| 0x03 | WRITE | Write data to a topic |
| 0x04 | DISPOSE | Dispose an instance |
| 0x05 | WRITE_DISPOSE | Write + dispose atomically |
| 0x10 | CREATE_WRITER | Pre-create a DataWriter |
| 0x11 | DELETE_WRITER | Delete a DataWriter |
| 0x0F | PING | Health check |

### Bridge -> Client

| msg_type | Name | Description |
|----------|------|-------------|
| 0x80 | OK | Success response (matched to request via request_id) |
| 0xC0 | DATA | Incoming data from subscribed topic (request_id = 0) |
| 0xC1 | DATA_DISPOSED | Instance disposed on subscribed topic (request_id = 0) |
| 0xC2 | DATA_NO_WRITERS | No writers detected on subscribed topic (request_id = 0) |
| 0xFE | ERROR | Error response |
| 0x8F | PONG | Health check response |

---

## Payload Definitions

### SUBSCRIBE (0x01)

```
topic_name: string
type_name: string         # DDS type name (e.g. "MyModule::MyType")
qos: QoS
key_descriptors: KeyDescriptors
```

If a DDS Reader already exists for the same (topic_name, type_name, normalized QoS),
it is shared. Otherwise a new Reader is created.

The bridge responds with OK. After OK, the client will begin receiving DATA
messages for this topic.

### UNSUBSCRIBE (0x02)

```
topic_name: string
type_name: string
```

Both topic_name and type_name are required to uniquely identify the subscription,
since a client may subscribe to the same topic_name with different type_names.

The bridge removes this client from the fan-out list for the matching Reader.
If no clients remain, the DDS Reader is deleted.

### WRITE (0x03)

Two modes distinguished by the first byte:

**Topic-name mode** (0x00) -- for first-time or low-frequency writes:
```
mode (1 byte): 0x00
topic_name: string
type_name: string
qos: QoS
key_descriptors: KeyDescriptors
data: remaining bytes     # opaque payload (CDR/XCDR2/etc.)
```

**Writer-ID mode** (0x01) -- for high-frequency writes (requires prior CREATE_WRITER):
```
mode (1 byte): 0x01
writer_id (4 bytes LE)
data: remaining bytes
```

In topic-name mode, the bridge looks up a Writer **owned by this client session**
with matching (topic_name, type_name, normalized QoS). If found, it is reused.
Otherwise a new Writer is implicitly created and associated with this session.

Writers created implicitly via topic-name mode are **per-client** -- they are
not shared across client sessions and are deleted when the client disconnects.

### DISPOSE (0x04)

Same two-mode structure as WRITE:

**Topic-name mode** (0x00):
```
mode (1 byte): 0x00
topic_name: string
type_name: string
qos: QoS
key_descriptors: KeyDescriptors
key_data: remaining bytes  # opaque key-only payload
```

**Writer-ID mode** (0x01):
```
mode (1 byte): 0x01
writer_id (4 bytes LE)
key_data: remaining bytes
```

### WRITE_DISPOSE (0x05)

Identical format to WRITE. Maps to `dds_writedispose()`.

### CREATE_WRITER (0x10)

```
topic_name: string
type_name: string
qos: QoS
key_descriptors: KeyDescriptors
```

Response (OK payload):
```
writer_id (4 bytes LE)
```

The Writer is associated with the creating client session.

### DELETE_WRITER (0x11)

```
writer_id (4 bytes LE)
```

Only the client that created the Writer can delete it.

### PING (0x0F)

Empty payload (payload_length = 0). Bridge responds with PONG using the same request_id.

### OK (0x80) -- Bridge -> Client

Payload depends on the original request:
- CREATE_WRITER: `writer_id (4 bytes LE)`
- All others: empty payload

### DATA (0xC0) -- Bridge -> Client

```
topic_name: string
source_timestamp (8 bytes LE): nanoseconds since Unix epoch. 0 = unavailable.
data: remaining bytes      # opaque payload
```

request_id is always 0 (unsolicited).

### DATA_DISPOSED (0xC1) -- Bridge -> Client

```
topic_name: string
source_timestamp (8 bytes LE)
key_data: remaining bytes  # key data of the disposed instance (if available)
```

request_id is always 0 (unsolicited).

### DATA_NO_WRITERS (0xC2) -- Bridge -> Client

```
topic_name: string
key_data: remaining bytes
```

request_id is always 0 (unsolicited).

### ERROR (0xFE) -- Bridge -> Client

```
error_code (2 bytes LE):
  0x0001 = INVALID_MESSAGE
  0x0002 = TOPIC_NOT_FOUND
  0x0003 = DDS_ERROR
  0x0004 = WRITER_NOT_FOUND
  0x0005 = READER_NOT_FOUND
  0x0006 = INVALID_QOS
  0x0007 = ALREADY_EXISTS
  0x0008 = INTERNAL_ERROR
  0x0009 = BUFFER_OVERFLOW
error_message: string
```

For errors in response to a client request, request_id echoes the original request.
For unsolicited errors (e.g. BUFFER_OVERFLOW), request_id is 0.

### PONG (0x8F) -- Bridge -> Client

Empty payload (payload_length = 0). Echoes the request_id from the corresponding PING.

---

## QoS Representation

Compact binary encoding of DDS standard QoS policies.

```
QoS:
  policy_count (1 byte): number of policies to set. 0 = all defaults.
  policies: Policy[policy_count]

Policy:
  policy_id (1 byte)
  value: fixed-length fields depending on policy_id
```

| policy_id | Name | Value format | Default |
|-----------|------|-------------|---------|
| 0x01 | Reliability | kind (1 byte): 0=BEST_EFFORT, 1=RELIABLE; max_blocking_time_ms (4 bytes LE) | RELIABLE, 100ms |
| 0x02 | Durability | kind (1 byte): 0=VOLATILE, 1=TRANSIENT_LOCAL, 2=TRANSIENT, 3=PERSISTENT | VOLATILE |
| 0x03 | History | kind (1 byte): 0=KEEP_LAST, 1=KEEP_ALL; depth (4 bytes LE, effective only for KEEP_LAST) | KEEP_LAST(1) |
| 0x04 | Deadline | period_ms (4 bytes LE). 0 = infinite | infinite |
| 0x05 | LatencyBudget | duration_ms (4 bytes LE) | 0 |
| 0x06 | OwnershipStrength | value (4 bytes LE) | 0 |
| 0x07 | Liveliness | kind (1 byte): 0=AUTOMATIC, 1=MANUAL_BY_PARTICIPANT, 2=MANUAL_BY_TOPIC; lease_duration_ms (4 bytes LE) | AUTOMATIC, infinite |
| 0x08 | DestinationOrder | kind (1 byte): 0=BY_RECEPTION_TIMESTAMP, 1=BY_SOURCE_TIMESTAMP | BY_RECEPTION_TIMESTAMP |
| 0x09 | Presentation | access_scope (1 byte): 0=INSTANCE, 1=TOPIC, 2=GROUP; coherent_access (1 byte): 0/1; ordered_access (1 byte): 0/1 | INSTANCE, false, false |
| 0x0A | Partition | partition_count (1 byte); partitions: string[partition_count] | (none) |
| 0x0B | Ownership | kind (1 byte): 0=SHARED, 1=EXCLUSIVE | SHARED |
| 0x0C | WriterDataLifecycle | autodispose_unregistered_instances (1 byte): 0/1 | true |
| 0x0D | TimeBasedFilter | minimum_separation_ms (4 bytes LE) | 0 |

Unspecified policies use their default values.

### QoS Normalization

For Reader sharing and Writer reuse, the bridge normalizes QoS before comparison:
explicitly-specified default values are treated as equivalent to omitted values.
For example, `{Reliability: RELIABLE, 100ms}` (explicit) equals `{}` (omitted,
which defaults to RELIABLE 100ms).

### QoS Example

Reliability=RELIABLE + Durability=TRANSIENT_LOCAL:

```
policy_count: 0x02
policy[0]: policy_id=0x01, kind=0x01, max_blocking_time_ms=100
policy[1]: policy_id=0x02, kind=0x01
```

---

## Key Descriptors

Layout information for key fields in DDS keyed topics. Provided by the client
so the bridge can configure Cyclone DDS sertype for correct instance management
(dispose, unregister, instance lookup).

```
KeyDescriptors:
  key_count (1 byte): number of key fields. 0 = keyless topic.
  keys: KeyField[key_count]

KeyField:
  offset (4 bytes LE): byte offset from payload start
  size (4 bytes LE): byte size of the key field
  type_hint (1 byte):
    0x00 = OPAQUE   (compare as raw bytes)
    0x01 = UUID      (16 bytes, compare as raw bytes)
    0x02 = INT32     (4 bytes LE)
    0x03 = INT64     (8 bytes LE)
    0x04 = STRING    (CDR string: 4-byte LE uint32 length + chars + NUL terminator)
```

### Key field size semantics

- For fixed-size types (OPAQUE, UUID, INT32, INT64): `size` is the exact byte size.
- For STRING: `size` is ignored by the bridge. The bridge reads the CDR string
  length prefix at `offset` to determine actual size.

### XCDR2 offset note

For XCDR2, the encapsulation header (4 bytes) precedes the payload.
The `offset` values must account for this -- clients are responsible for
setting correct offsets relative to the start of the serialized data
(including the encapsulation header).

---

## Backpressure and Flow Control

Each client session has a **bounded send buffer** (configurable, default: 256
messages). When the bridge receives data from DDS faster than a client can
consume via WebSocket:

1. The oldest undelivered message in the session buffer is dropped.
2. The bridge sends an ERROR (BUFFER_OVERFLOW, request_id=0) to the client
   with a message indicating how many messages were dropped and for which topic.
3. Delivery continues with the remaining buffered messages.

This ensures that a slow client does not block fan-out to other clients or
cause unbounded memory growth in the bridge.

Clients that cannot tolerate drops should ensure their WebSocket consumer
keeps up, or use a RELIABLE + KEEP_ALL QoS on the DDS side combined with
application-level flow control.

---

## Error Handling

| Condition | Behavior |
|-----------|----------|
| Malformed message | ERROR (INVALID_MESSAGE) returned; connection maintained |
| Unsupported version | ERROR (INVALID_MESSAGE) with version info; connection maintained |
| Payload exceeds max size | ERROR (INVALID_MESSAGE) returned; connection maintained |
| DDS operation failure | ERROR (DDS_ERROR) returned; connection maintained |
| Unknown writer_id | ERROR (WRITER_NOT_FOUND) returned |
| Unknown reader_id | ERROR (READER_NOT_FOUND) returned |
| Session buffer overflow | ERROR (BUFFER_OVERFLOW) warning (request_id=0); oldest messages dropped |
| Fatal DDS Participant error | ERROR sent to all clients; process exits (expects supervisor restart) |

---

## Connection Lifecycle

### Client Connect

1. WebSocket handshake completes
2. Bridge creates internal client session (assigns unique session_id)
3. Connection info logged

### Client Disconnect

1. All subscriptions created by this client are removed
2. All Writers created by this client are deleted (both explicit and implicit)
3. Readers not shared with other clients are deleted
4. Writer `autodispose_unregistered_instances` QoS is honored (instances disposed on DDS if set)

### Health Check

- Application-level PING (0x0F) / PONG (0x8F)
- WebSocket-level ping/pong frames also used
- Configurable idle timeout (default: 30 seconds); connection closed on timeout
