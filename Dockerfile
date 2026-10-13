# syntax=docker/dockerfile:1
#
# OxiDav — single-binary CalDAV/CardDAV/WebDAV server with the React SPA
# embedded via `rust-embed`.
#
# ---------------------------------------------------------------------------
# BUILD ORDER (critical)
# ---------------------------------------------------------------------------
# `rust-embed` reads `web/dist` **at compile time** (see `src/assets.rs` and
# `build.rs`). The SPA must therefore exist *before* `cargo build --release`.
# This Dockerfile enforces that order with three stages:
#
#   web      -> node:22 builds the SPA  => produces web/dist
#   builder  -> rust compiles the server with web/dist embedded
#   runtime  -> minimal, non-root image with just the binary
#
# Local (non-Docker) equivalent:
#
#   cd web && npm ci && npm run build   # web/dist
#   cargo build --release --locked      # embeds web/dist
#
# ---------------------------------------------------------------------------
# WHY glibc (debian) instead of musl for the builder
# ---------------------------------------------------------------------------
# `libsqlite3-sys` is built with the `bundled` feature (compiles SQLite C from
# source) and `rustls` uses the `aws-lc-rs` provider (`aws-lc-sys`, also C).
# Both compile reliably against glibc with a normal C toolchain + cmake; a
# musl-static target would need extra cross toolchains. We keep the *runtime*
# image minimal (debian-slim, ca-certificates only) while staying on glibc.
# No OpenSSL anywhere: `jsonwebtoken` uses `rust_crypto` and `reqwest` uses
# `rustls`, so the TLS stack is pure-Rust/aws-lc and statically linked.
# ---------------------------------------------------------------------------

# ===========================================================================
# Stage 1 — build the React SPA (Vite) -> web/dist
# ===========================================================================
FROM node:22-slim AS web

WORKDIR /app/web

# Install dependencies first so this layer is cached until the lockfile changes.
# `npm ci` reproduces the lockfile exactly and is required to be deterministic.
COPY web/package.json web/package-lock.json ./
RUN npm ci

# Copy the SPA sources. `.dockerignore` excludes `web/node_modules` and
# `web/dist` from the build context, so this never clobbers the installed
# `node_modules`, and the host `web/dist` is *not* pulled in (the SPA is built
# here, in-container, on purpose).
COPY web/ ./
RUN npm run build   # `tsc -b && vite build` -> /app/web/dist

# ===========================================================================
# Stage 2 — compile the Rust server, embedding web/dist
# ===========================================================================
FROM rust:1.97.0-bookworm AS builder

# `cmake` + a C toolchain are needed to build `aws-lc-sys` (rustls provider);
# the base rust image already provides gcc/make. SQLite's bundled C build uses
# the same toolchain.
RUN apt-get update \
 && apt-get install -y --no-install-recommends cmake \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# --- Dependency cache layer -------------------------------------------------
# Copy only the manifests + build script and compile a dummy crate so every
# dependency (the expensive part: aws-lc-sys, libsqlite3-sys, tokio, …) is
# built in a *separate* layer that only invalidates when Cargo.lock changes.
# The registry cache mount keeps the crates.io download cache across builds.
COPY Cargo.toml Cargo.lock build.rs ./
RUN mkdir -p src \
 && : > src/lib.rs \
 && printf 'fn main() {}\n' > src/main.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked

# --- Real source + embedded SPA --------------------------------------------
# `migrations/` is embedded at compile time via `sqlx::migrate!("./migrations")`,
# so it must be present now (it is not needed at runtime).
COPY src ./src
COPY migrations ./migrations
# The real SPA from stage 1. Overwrites the placeholder that `build.rs` may have
# created during the dummy build.
COPY --from=web /app/web/dist ./web/dist

# Recompile: only the oxidav crate is rebuilt; all dependencies stay cached.
# A leading `touch` guarantees cargo sees the sources as newer than the dummy.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    touch src/lib.rs src/main.rs \
 && cargo build --release --locked

# ===========================================================================
# Stage 3 — minimal runtime, non-root
# ===========================================================================
FROM debian:bookworm-slim AS runtime

# ca-certificates: needed for HTTPS to Pocket ID (OIDC discovery + JWKS).
# No curl/wget: the healthcheck below uses bash's /dev/tcp, keeping the image
# minimal. SQLite and the TLS stack are statically linked into the binary.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --gid 10001 oxidav \
 && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin oxidav

WORKDIR /app

# Single distributable binary (API + DAV + embedded SPA).
COPY --from=builder /app/target/release/oxidav /usr/local/bin/oxidav

# Persistent SQLite location. Owned by the non-root user so the DB file can be
# created/migrated at start-up (default `OXIDAV_DATABASE_URL=sqlite://data/oxidav.db`
# resolves to /app/data/oxidav.db).
RUN mkdir -p /app/data && chown -R oxidav:oxidav /app

# Runtime configuration (all overridable). Placeholders only — never secrets.
ENV OXIDAV_BIND_ADDR=0.0.0.0:8080 \
    OXIDAV_DATABASE_URL=sqlite://data/oxidav.db \
    RUST_LOG=info

EXPOSE 8080

# Drop privileges: never run as root.
USER 10001:10001

# Readiness probe against `/healthz`. Uses bash's /dev/tcp (no curl in the
# image); assumes the documented default port 8080. Override the healthcheck in
# your orchestrator if you change OXIDAV_BIND_ADDR.
#
# NOTE: `HEALTHCHECK` is a Docker-specific instruction. `docker build` records
# it; Podman only does so with `--format docker` (its default OCI manifest drops
# it). The example `compose.yaml` therefore declares an equivalent healthcheck.
# If the port is customised, adjust both the probe and the published port.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["bash", "-c", "exec 3<>/dev/tcp/127.0.0.1/8080 && printf 'GET /healthz HTTP/1.0\\r\\nHost: localhost\\r\\nConnection: close\\r\\n\\r\\n' >&3 && grep -q '200' <&3"]

ENTRYPOINT ["/usr/local/bin/oxidav"]
