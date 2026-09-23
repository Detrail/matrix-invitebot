# ============================================================
# BUILD STAGE
# ============================================================
FROM rust:1-bookworm AS builder

WORKDIR /app

# Note: this does a full rebuild of dependencies on every push,
# rather than caching them behind a dummy main.rs. That caching
# trick is fragile — Cargo's freshness check can be fooled by how
# Docker sets file timestamps on COPY, silently skipping the real
# rebuild and leaving a stale empty-main.rs binary in the image
# (which is what caused the earlier "container exits instantly
# with no output" issue). If you want fast caching back later,
# use `cargo-chef` instead — it's built specifically to do this
# correctly.
COPY . .
RUN cargo build --release --locked

# ============================================================
# RUNTIME STAGE
# ============================================================
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# IMPORTANT: adjust the binary name below if your Cargo.toml
# [package] name isn't "invite-bot" — it must match exactly, e.g.
# from `name = "invite-bot"` in Cargo.toml.
COPY --from=builder /app/target/release/invite-bot /app/invite-bot

# All persistent state — session.json, the sqlite crypto store, and
# any key export/import files — is written under /data by setting
# the corresponding env vars in docker-compose.yml. Mount a volume
# here so state survives container restarts and image rebuilds.
VOLUME ["/data"]

ENTRYPOINT ["/app/invite-bot"]
