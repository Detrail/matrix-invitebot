# ============================================================
# BUILD STAGE
# ============================================================
FROM rust:1-bookworm AS builder

WORKDIR /app

# Cache dependency compilation separately from source changes so
# `docker build` doesn't recompile every dependency on every code
# edit. If this trick misbehaves for your project layout, just
# delete this block — the final `cargo build --release` below
# still works fine without it, just slower on rebuilds.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

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
