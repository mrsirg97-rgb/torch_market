# syntax=docker/dockerfile:1.6
#
# Build context: REPO ROOT (the crate path-depends on ../indexer-core after
# the prompt-003 split).
# Build command:  docker build -f indexer/Dockerfile -t torch-ingest .
#
# Two stages:
#   1. builder — compiles torch-indexer
#   2. runtime — minimal debian-slim image with just the binary + TLS CAs

# --------------------------------------------------------------------
# Stage 1: builder
# --------------------------------------------------------------------
FROM rust:1-slim-bookworm AS builder

# Build-time deps:
#   - build-essential: protobuf-src vendors protobuf C++ + builds via autotools
#   - protobuf-compiler: system protoc for yellowstone-grpc-proto codegen
#   - libssl-dev + pkg-config: yellowstone-grpc-client pulls openssl-sys
#     transitively (tonic's default TLS uses native-tls on Linux)
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        protobuf-compiler \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Root-context copies: the ingest crate + its path dep.
COPY indexer-core ./indexer-core
COPY indexer/Cargo.toml indexer/Cargo.lock ./indexer/
COPY indexer/src ./indexer/src
WORKDIR /build/indexer

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/build/indexer/target,sharing=locked \
    cargo build --release --locked --bin torch-indexer --bin loadseed && \
    cp /build/indexer/target/release/torch-indexer /usr/local/bin/torch-indexer

# --------------------------------------------------------------------
# Stage 2: runtime
# --------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# Runtime deps:
#   - ca-certificates: TLS trust roots for Helius Laserstream + RPC endpoints
#   - libssl3: binary is dynamically linked against OpenSSL
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

COPY --from=builder /usr/local/bin/torch-indexer /usr/local/bin/torch-indexer

# Run as non-root.
RUN useradd --system --create-home --shell /bin/false indexer
USER indexer
WORKDIR /home/indexer

EXPOSE 8080

ENTRYPOINT ["torch-indexer"]
