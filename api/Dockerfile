# syntax=docker/dockerfile:1.6
#
# torch read-API service (prompt-003 split). Build context: REPO ROOT —
# path-deps on ../indexer-core, and the manifest's dev-dependency on
# ../indexer must resolve for cargo to parse (tests aren't built here).
# Build: docker build -f api/Dockerfile -t torch-api .

FROM rust:1-slim-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        pkg-config \
        protobuf-compiler \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

COPY indexer-core ./indexer-core
COPY indexer/Cargo.toml indexer/Cargo.lock ./indexer/
COPY indexer/src ./indexer/src
COPY api/Cargo.toml api/Cargo.lock* ./api/
COPY api/src ./api/src
WORKDIR /build/api

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/build/api/target,sharing=locked \
    cargo build --release --bin torch-api && \
    cp /build/api/target/release/torch-api /usr/local/bin/torch-api

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && update-ca-certificates

COPY --from=builder /usr/local/bin/torch-api /usr/local/bin/torch-api

RUN useradd --system --create-home --shell /bin/false api
USER api
WORKDIR /home/api

EXPOSE 8081

ENTRYPOINT ["torch-api"]
