FROM rust:1.90-bookworm AS builder

# Install Cyclone DDS build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    libclang-dev \
    clang \
    git \
    && rm -rf /var/lib/apt/lists/*

# Build Cyclone DDS from source (11.0.1)
ARG CYCLONEDDS_VERSION=11.0.1
RUN git clone --depth 1 --branch ${CYCLONEDDS_VERSION} \
        https://github.com/eclipse-cyclonedds/cyclonedds.git /tmp/cyclonedds && \
    cmake -S /tmp/cyclonedds -B /tmp/cyclonedds/build \
        -DCMAKE_INSTALL_PREFIX=/usr/local \
        -DBUILD_EXAMPLES=OFF \
        -DBUILD_TESTING=OFF \
        -DBUILD_IDLC=OFF \
        -DBUILD_DDSPERF=OFF && \
    cmake --build /tmp/cyclonedds/build --parallel $(nproc) && \
    cmake --install /tmp/cyclonedds/build && \
    rm -rf /tmp/cyclonedds

ENV PKG_CONFIG_PATH=/usr/local/lib/pkgconfig:/usr/local/lib/x86_64-linux-gnu/pkgconfig
ENV LD_LIBRARY_PATH=/usr/local/lib:/usr/local/lib/x86_64-linux-gnu

WORKDIR /app
COPY Cargo.toml Cargo.lock build.rs wrapper.h helper.c ./
COPY src/ src/
COPY tests/ tests/

RUN cargo build --release 2>&1

# Run all tests (loopback config required for DDS integration tests in Docker)
RUN CYCLONEDDS_URI='<CycloneDDS><Domain><General><Interfaces><NetworkInterface name="lo"/></Interfaces><AllowMulticast>false</AllowMulticast></General></Domain></CycloneDDS>' \
    cargo test --release -- --test-threads=1 2>&1

# Runtime stage
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    bash \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/lib/ /usr/local/lib/
COPY --from=builder /app/target/release/cyclone-dds-ws-bridge /usr/local/bin/cyclone-dds-ws-bridge

ENV LD_LIBRARY_PATH=/usr/local/lib:/usr/local/lib/x86_64-linux-gnu

EXPOSE 9876

HEALTHCHECK --interval=10s --timeout=5s --start-period=5s --retries=3 \
    CMD ["cyclone-dds-ws-bridge", "healthcheck"]

CMD ["cyclone-dds-ws-bridge"]
