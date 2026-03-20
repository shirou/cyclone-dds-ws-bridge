FROM rust:1.90-bookworm AS builder

# Install Cyclone DDS build deps
RUN apt-get update && apt-get install -y --no-install-recommends \
    cmake \
    libclang-dev \
    clang \
    git \
    && rm -rf /var/lib/apt/lists/*

# Build Cyclone DDS from source (0.10.5)
ARG CYCLONEDDS_VERSION=0.10.5
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
COPY Cargo.toml Cargo.lock build.rs wrapper.h ./
COPY src/ src/

RUN cargo build --release 2>&1

# Run unit tests
RUN cargo test --release -- --test-threads=1 2>&1
