# syntax=docker/dockerfile:1

# ---- Stage 1: frontend bundle (bun) --------------------------------------------------------
FROM oven/bun:1 AS frontend
WORKDIR /app/frontend
# Lockfile-first for layer caching.
COPY frontend/package.json frontend/bun.lock ./
RUN bun install --frozen-lockfile
COPY frontend/ ./
RUN bun run build

# ---- Stage 2: Rust build (glibc, cargo-chef dependency cache) ------------------------------
FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Compile just the dependencies (cached until Cargo.toml/Cargo.lock change).
RUN cargo chef cook --release -p photon-server --recipe-path recipe.json
# Now the real sources...
COPY . .
# ...and the built frontend where rust-embed expects it (../../frontend/dist from photon-api).
COPY --from=frontend /app/frontend/dist ./frontend/dist
RUN cargo build --release -p photon-server
# Hand an empty, correctly-owned data dir to the runtime image (fresh named volumes inherit it).
RUN mkdir -p /var/lib/photon

# ---- Stage 2b: the agent, on the OLDEST glibc we can build against --------------------------
# The agent ships INSIDE the server image: `GET /api/agent/download` and the SSH bootstrap serve
# the `photon-agent` beside `photon-server` (docs/subsystems/deploy.md), so an air-gapped site
# never needs the internet to install it. But that binary runs on the CUSTOMER's hosts, not in
# this image — and a glibc binary needs the glibc it was built against or newer. Bookworm's 2.36
# fails on Ubuntu 22.04 (2.35) with "GLIBC_2.36 not found"; bullseye's 2.31 runs on Ubuntu
# 20.04+, Debian 11+, RHEL 9+. Dynamic glibc is deliberate — GPU sampling dlopen()s NVML, which a
# static musl build cannot do — so the baseline is the lever, and it lives here.
FROM rust:1-bullseye AS agent-builder
WORKDIR /app
COPY . .
RUN cargo build --release -p photon-agent && strip target/release/photon-agent

# ---- Stage 3: runtime (distroless glibc, non-root) -----------------------------------------
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime
COPY --from=builder /app/target/release/photon-server /usr/local/bin/photon-server
COPY --from=agent-builder /app/target/release/photon-agent /usr/local/bin/photon-agent
COPY --from=builder --chown=65532:65532 /var/lib/photon /var/lib/photon
ENV PHOTON_STORAGE_HOT_DIR=/var/lib/photon/hot \
    PHOTON_STORAGE_DB_PATH=/var/lib/photon/photon.db \
    PHOTON_API_ADDR=0.0.0.0:8080
EXPOSE 8080 4317 4318
VOLUME /var/lib/photon
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/photon-server"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=15s \
  CMD ["/usr/local/bin/photon-server", "healthcheck"]
