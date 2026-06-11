# syntax=docker/dockerfile:1.6
#
# torch UI — Next.js standalone. Build context: REPO ROOT (pnpm workspace:
# the app consumes packages/sdk by path). NEXT_PUBLIC_* bake at build time;
# api.<domain> is known a priori, so this builds once, first try.
# Build: docker build -f packages/app/Dockerfile \
#          --build-arg NEXT_PUBLIC_INDEXER_URL=https://api.torchmarket.dev \
#          --build-arg NEXT_PUBLIC_NETWORK=devnet -t torch-ui .

FROM node:22-slim AS builder

# node-gyp toolchain for the native deps (bufferutil, utf-8-validate, keccak,
# secp256k1): slim images ship without python3/make/g++. Builder stage only —
# the runtime stage stays toolchain-free.
RUN apt-get update && apt-get install -y --no-install-recommends \
        python3 \
        make \
        g++ \
    && rm -rf /var/lib/apt/lists/*

ARG NEXT_PUBLIC_INDEXER_URL
ARG NEXT_PUBLIC_NETWORK=devnet
ENV NEXT_PUBLIC_INDEXER_URL=${NEXT_PUBLIC_INDEXER_URL} \
    NEXT_PUBLIC_NETWORK=${NEXT_PUBLIC_NETWORK} \
    NEXT_TELEMETRY_DISABLED=1

# Pin pnpm to the version that WROTE the lockfile (local dev runs 9.15).
# Unpinned corepack grabs latest (11.x), whose supply-chain policy engine
# blocks native build scripts under different rules — reproducible builds
# want the same package manager, full stop.
RUN corepack enable && corepack prepare pnpm@9.15.0 --activate

WORKDIR /build
# No root package.json — the workspace is pnpm-workspace.yaml + lockfile only.
COPY pnpm-lock.yaml pnpm-workspace.yaml ./
COPY packages/sdk/package.json packages/sdk/package.json
COPY packages/app/package.json packages/app/package.json
RUN pnpm install --frozen-lockfile

COPY packages/sdk packages/sdk
COPY packages/app packages/app
RUN pnpm --filter ./packages/sdk build && pnpm --filter ./packages/app build

FROM node:22-slim AS runtime
ENV NODE_ENV=production \
    NEXT_TELEMETRY_DISABLED=1 \
    PORT=3000 \
    HOSTNAME=0.0.0.0

WORKDIR /app
# Next standalone layout: server.js + minimal node_modules, plus static assets.
COPY --from=builder /build/packages/app/.next/standalone ./
COPY --from=builder /build/packages/app/.next/static ./packages/app/.next/static
COPY --from=builder /build/packages/app/public ./packages/app/public

RUN useradd --system --create-home --shell /bin/false ui
USER ui

EXPOSE 3000
CMD ["node", "packages/app/server.js"]
