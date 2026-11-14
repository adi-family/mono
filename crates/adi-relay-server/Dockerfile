# The webhook relay service (ADI-MONO-222): one binary, one
# relay.toml, local SQLite — the whole point being that this image runs identically on our own
# AWS boxes and a self-host customer's, with no adi-mono install assumed on either side.
#
# Built from the repo root: `docker build -f crates/adi-relay-server/Dockerfile .`
# (the build context has to be the workspace root — this crate depends on sibling workspace
# members and the workspace's own Cargo.lock, not just its own directory).

FROM rust:1-slim-bookworm AS builder
WORKDIR /build

# rusqlite's `bundled` feature compiles SQLite's own C source at build time (compiles SQLite in
# rather than linking a system library) — the one native toolchain this otherwise-pure-Rust
# binary needs.
RUN apt-get update && apt-get install -y --no-install-recommends build-essential \
    && rm -rf /var/lib/apt/lists/*

COPY . .
RUN cargo build --release --locked -p adi-relay-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --no-create-home --shell /usr/sbin/nologin relay

COPY --from=builder /build/target/release/adi-relay-server /usr/local/bin/adi-relay-server

# relay.toml's own db_path (the sample config's "/data/relay.db") lives under here; an operator
# mounts relay.toml itself at /data/relay.toml and the volume covers both in one mount.
RUN mkdir -p /data && chown relay:relay /data
VOLUME /data
USER relay
WORKDIR /data

EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s \
    CMD curl -fsS http://localhost:8080/healthz || exit 1

ENTRYPOINT ["/usr/local/bin/adi-relay-server"]
CMD ["/data/relay.toml"]
