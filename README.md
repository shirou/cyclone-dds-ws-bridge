# cyclone-dds-ws-bridge

A WebSocket bridge for [Eclipse Cyclone DDS](https://github.com/eclipse-cyclonedds/cyclonedds). Enables non-DDS applications (Go, Python, browsers, etc.) to publish and subscribe to DDS topics over WebSocket using opaque byte payloads (CDR/XCDR2).

## Overview

```
┌──────────┐   WebSocket    ┌─────────────────┐      DDS/RTPS       ┌──────────┐
│ Go app   │◄──(binary)────►│                 │◄────────────────────►│ DDS node │
└──────────┘                │  cyclone-dds-   │                      └──────────┘
┌──────────┐   WebSocket    │  ws-bridge      │      DDS/RTPS       ┌──────────┐
│ Python   │◄──(binary)────►│                 │◄────────────────────►│ DDS node │
└──────────┘                └─────────────────┘                      └──────────┘
```

The bridge:

- Accepts WebSocket binary connections from clients
- Translates SUBSCRIBE/WRITE/DISPOSE commands to DDS Reader/Writer operations
- Fans out DDS data to subscribed WebSocket clients
- Passes payloads through as opaque bytes (clients serialize/deserialize CDR themselves)

## Requirements

- Rust 1.90+
- [Cyclone DDS](https://github.com/eclipse-cyclonedds/cyclonedds) >= 0.10.5
- `cmake`, `clang`, `libclang-dev` (for bindgen)
- `pkg-config`

## Building

### Docker (recommended)

```bash
docker build -t cyclone-dds-ws-bridge .
```

The Dockerfile builds Cyclone DDS 0.10.5 from source and runs all unit tests.

### Native

```bash
# Install Cyclone DDS
git clone --depth 1 --branch 0.10.5 https://github.com/eclipse-cyclonedds/cyclonedds.git
cmake -S cyclonedds -B cyclonedds/build \
    -DCMAKE_INSTALL_PREFIX=/usr/local \
    -DBUILD_EXAMPLES=OFF -DBUILD_TESTING=OFF -DBUILD_IDLC=OFF -DBUILD_DDSPERF=OFF
cmake --build cyclonedds/build --parallel $(nproc)
sudo cmake --install cyclonedds/build

# Build the bridge
cargo build --release
```

## Running

```bash
# With defaults (127.0.0.1:9876, domain 0)
cyclone-dds-ws-bridge

# With config file
cyclone-dds-ws-bridge --config config.toml

# With environment variables
DDS_BRIDGE_WS_ADDR=0.0.0.0 DDS_BRIDGE_WS_PORT=9876 cyclone-dds-ws-bridge

# Docker
docker run -p 9876:9876 -e DDS_BRIDGE_WS_ADDR=0.0.0.0 cyclone-dds-ws-bridge
```

## Configuration

The bridge loads configuration from a TOML file and/or environment variables. See [`config.example.toml`](config.example.toml) for all options.

| Variable | Description | Default |
|----------|-------------|---------|
| `DDS_BRIDGE_WS_ADDR` | WebSocket listen address | `127.0.0.1` |
| `DDS_BRIDGE_WS_PORT` | WebSocket listen port | `9876` |
| `DDS_BRIDGE_DDS_DOMAIN` | DDS domain ID | `0` |
| `CYCLONEDDS_URI` | Cyclone DDS XML configuration | (none) |

Environment variables override values from the config file.

## Protocol

The bridge uses a compact binary protocol over WebSocket binary frames. See [`docs/protocol.md`](docs/protocol.md) for the full specification.

Every message starts with a 12-byte header:

```
offset  size  field
──────────────────────────────────
0       2     magic: 0x44 0x42 ("DB")
2       1     version: 0x01
3       1     msg_type
4       4     request_id (LE uint32)
8       4     payload_length (LE uint32)
```

### Message types

| Type | Code | Direction | Description |
|------|------|-----------|-------------|
| SUBSCRIBE | 0x01 | Client -> Bridge | Subscribe to a DDS topic |
| UNSUBSCRIBE | 0x02 | Client -> Bridge | Unsubscribe from a topic |
| WRITE | 0x03 | Client -> Bridge | Write data (topic-name or writer-id mode) |
| DISPOSE | 0x04 | Client -> Bridge | Dispose an instance |
| WRITE_DISPOSE | 0x05 | Client -> Bridge | Write and dispose |
| CREATE_WRITER | 0x10 | Client -> Bridge | Create an explicit writer (returns writer_id) |
| DELETE_WRITER | 0x11 | Client -> Bridge | Delete a writer |
| PING | 0x0F | Client -> Bridge | Connectivity check |
| OK | 0x80 | Bridge -> Client | Success response |
| DATA | 0xC0 | Bridge -> Client | Received DDS data sample |
| DATA_DISPOSED | 0xC1 | Bridge -> Client | Instance disposed notification |
| DATA_NO_WRITERS | 0xC2 | Bridge -> Client | No writers for instance |
| ERROR | 0xFE | Bridge -> Client | Error response |
| PONG | 0x8F | Bridge -> Client | Ping response |

### Typical workflow

1. Connect via WebSocket
2. `SUBSCRIBE` to a topic (with type name, QoS, and key descriptors)
3. Receive `DATA` messages as DDS samples arrive
4. `WRITE` data to a topic (bridge creates/reuses a DDS Writer)
5. `UNSUBSCRIBE` / disconnect (bridge cleans up DDS entities)

## Go Client Library

```go
import "github.com/shirou/cyclone-dds-ws-bridge/ddswsclient"

client, err := ddswsclient.NewClient(ctx, "ws://localhost:9876")

// Subscribe
sub, _ := client.Subscribe(ctx, "SensorData", "sensor::SensorData", nil, nil)
for msg := range sub.C() {
    // msg.Data contains CDR-encoded payload
}

// Publish via explicit writer
writer, _ := client.CreateWriter(ctx, "SensorData", "sensor::SensorData", nil, nil)
writer.Publish(cdrBytes)
```

Features:
- Auto-reconnect with transparent writer/subscription restoration
- Request correlation (request_id tracking)
- Thread-safe concurrent access
- Works with [`go-dds-idlgen`](https://github.com/shirou/go-dds-idlgen) for CDR serialization

## Testing

```bash
# Rust unit tests (inside Docker build)
docker build .

# Go client unit tests
cd ddswsclient && go test -v ./...

# CDR interop tests (Go <-> Python byte-exact comparison)
docker compose -f docker-compose.test.yml --profile interop up --abort-on-container-exit

# Bridge E2E tests (Go + Python clients through running bridge)
docker compose -f docker-compose.test.yml --profile e2e up --abort-on-container-exit

# Python client tests (pytest, against running bridge)
docker compose -f docker-compose.test.yml --profile client up --abort-on-container-exit
```

To regenerate protocol test fixtures after a protocol change:

```bash
cargo test -- --ignored generate_fixtures
```

## Project Structure

```
src/
├── main.rs              # Entry point (config, tracing, shutdown)
├── lib.rs               # Library root
├── config.rs            # TOML + env var configuration
├── protocol.rs          # Wire protocol parse/serialize
├── qos.rs               # QoS binary representation
├── bridge.rs            # WS <-> DDS mediation
├── dds/                 # DDS layer (sertype, participant, reader, writer)
└── ws/                  # WebSocket layer (server, session)
ddswsclient/             # Go client library
tests/
├── fixtures/            # Shared .bin protocol test vectors
├── fixtures_test.rs     # Fixture generation + verification
├── protocol_test.rs     # Protocol integration tests
├── integration_pubsub.rs # Bridge pubsub integration tests
├── client/              # Python test client (pytest)
├── e2e/                 # Bridge E2E tests (Go + Python)
└── interop/             # CDR interop tests (Go <-> Python)
docs/
├── protocol.md          # Protocol specification
└── implementation-plan.md
```

## License

Apache-2.0. See [LICENSE](LICENSE).
