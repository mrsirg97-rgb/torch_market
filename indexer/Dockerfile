# syntax=docker/dockerfile:1.6
#
# Build context: ./indexer  (the standalone crate, kept out of the Anchor
# program workspace so cargo doesn't pull yellowstone/sqlx/axum into program
# builds).
# Build command:  docker compose --profile full build indexer
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

# Standalone crate — bring in just the manifest, lockfile, and source.
# BuildKit cache mounts on the cargo registry + target dir keep incremental
# builds fast; on a clean build the mounts are empty (cold compile).
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --release --locked && \
    cp /build/target/release/torch-indexer /usr/local/bin/torch-indexer

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
