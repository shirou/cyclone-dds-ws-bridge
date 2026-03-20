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

## Status

**Work in progress.**

## Requirements

- Rust 1.75+
- [Cyclone DDS](https://github.com/eclipse-cyclonedds/cyclonedds) >= 0.10.5
- `cmake`, `clang`, `libclang-dev` (for bindgen)
- `pkg-config`

## Building

### Install Cyclone DDS

```bash
git clone --depth 1 --branch 0.10.5 https://github.com/eclipse-cyclonedds/cyclonedds.git
cmake -S cyclonedds -B cyclonedds/build \
    -DCMAKE_INSTALL_PREFIX=/usr/local \
    -DBUILD_EXAMPLES=OFF -DBUILD_TESTING=OFF -DBUILD_IDLC=OFF -DBUILD_DDSPERF=OFF
cmake --build cyclonedds/build --parallel $(nproc)
sudo cmake --install cyclonedds/build
```

### Build the bridge

```bash
cargo build --release
```

### Run tests

```bash
cargo test
```

To regenerate protocol test fixtures after a protocol change:

```bash
cargo test -- --ignored generate_fixtures
```

## Configuration

The bridge loads configuration from a TOML file and/or environment variables.

```bash
# From config file
cyclone-dds-ws-bridge --config config.toml

# Or use environment variables
DDS_BRIDGE_WS_ADDR=0.0.0.0 DDS_BRIDGE_WS_PORT=9876 cyclone-dds-ws-bridge
```

See [`config.example.toml`](config.example.toml) for all options.

### Environment variables

| Variable | Description | Default |
|----------|-------------|---------|
| `DDS_BRIDGE_WS_ADDR` | WebSocket listen address | `127.0.0.1` |
| `DDS_BRIDGE_WS_PORT` | WebSocket listen port | `9876` |
| `DDS_BRIDGE_DDS_DOMAIN` | DDS domain ID | `0` |
| `CYCLONEDDS_URI` | Cyclone DDS XML configuration | (none) |

Environment variables override values from the config file.

## Protocol

The bridge uses a custom binary protocol over WebSocket binary frames. See [`docs/protocol.md`](docs/protocol.md) for the full specification.

### Message format

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

| Direction | Messages |
|-----------|----------|
| Client -> Bridge | SUBSCRIBE, UNSUBSCRIBE, WRITE, DISPOSE, WRITE_DISPOSE, CREATE_WRITER, DELETE_WRITER, PING |
| Bridge -> Client | OK, DATA, DATA_DISPOSED, DATA_NO_WRITERS, ERROR, PONG |

### Typical workflow

1. Connect via WebSocket
2. `SUBSCRIBE` to a topic (with type name, QoS, and key descriptors)
3. Receive `DATA` messages as DDS samples arrive
4. `WRITE` data to a topic (bridge creates/reuses a DDS Writer)
5. `UNSUBSCRIBE` / disconnect (bridge cleans up DDS entities)

## Go client library

A Go client library will be available at:

```go
import "github.com/shirou/cyclone-dds-ws-bridge/ddswsclient"
```

The client works with opaque `[]byte` payloads. Use [go-dds-idlgen](https://github.com/example/go-dds-idlgen) generated types for CDR serialization.

## Docker

```bash
docker build -t cyclone-dds-ws-bridge .
docker run -p 9876:9876 -e DDS_BRIDGE_WS_ADDR=0.0.0.0 cyclone-dds-ws-bridge
```

## Project structure

```
src/
├── lib.rs          # Library root
├── main.rs         # Entry point
├── config.rs       # TOML + env var configuration
├── protocol.rs     # Wire protocol parse/serialize
├── qos.rs          # QoS binary representation
├── bridge.rs       # WS <-> DDS mediation (Phase 4)
├── dds/            # DDS layer (Phase 2)
└── ws/             # WebSocket layer (Phase 3)
tests/
├── fixtures/       # Shared .bin test vectors
└── fixtures_test.rs
docs/
├── protocol.md     # Protocol specification
└── implementation-plan.md
```

## License

Apache-2.0
