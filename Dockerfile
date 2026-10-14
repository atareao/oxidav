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
# WHY musl (Alpine) for both builder and runtime
# ---------------------------------------------------------------------------
# `libsqlite3-sys` is built with the `bundled` feature (compiles SQLite C from
# source) and `rustls` uses the `aws-lc-rs` provider (`aws-lc-sys`, also C).
# The `rust:1.98.1-alpine3.23` image targets `x86_64-unknown-linux-musl` by
# default, so a normal `apk` C toolchain is enough: `aws-lc-sys` builds through
# the `cc` path and does **not** need a separate build-system generator. A fully
# static musl binary drops straight into `alpine:3.23`, and no glibc runtime
# shims are required.
# No OpenSSL anywhere: `jsonwebtoken` uses `rust_crypto` and `reqwest` uses
# `rustls`, so the TLS stack is pure-Rust/aws-lc and statically linked.
# ---------------------------------------------------------------------------

# ===========================================================================
# Stage 1 — build the React SPA (Vite) -> web/dist
# ===========================================================================
FROM node:22-alpine3.23 AS web

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
FROM rust:1.98.1-alpine3.23 AS builder

# `build-base` (gcc + make + strip) and `musl-dev` provide the C toolchain used
# by `aws-lc-sys` (rustls provider) and by SQLite's bundled C build;
# `pkgconfig` lets `libsqlite3-sys` probe system libs. No extra build-system
# generator is required: `aws-lc-sys` compiles via the `cc` path on musl.
RUN apk add --no-cache build-base musl-dev pkgconfig

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
# The dummy dependency-cache build runs in *release*, and `build.rs` aborts in
# release when `web/dist/index.html` is missing (or is its own development
# placeholder). Seed a *different* stub so the dummy build succeeds; the real
# SPA is copied below and `cargo:rerun-if-changed=web/dist` forces `build.rs`
# to re-run and embed the real bundle. This stub never reaches the runtime
# image (stage 3 copies only the binary).
RUN mkdir -p web/dist && printf '<!doctype html>\n' > web/dist/index.html
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --locked

# --- Real source + embedded SPA --------------------------------------------
# `migrations/` is embedded at compile time via `sqlx::migrate!("./migrations")`,
# so it must be present now (it is not needed at runtime).
COPY src ./src
COPY migrations ./migrations
# The real SPA from stage 1. Overwrites the stub seeded before the dummy build.
COPY --from=web /app/web/dist ./web/dist

# Recompile: only the oxidav crate is rebuilt; all dependencies stay cached.
# A leading `touch` guarantees cargo sees the sources as newer than the dummy.
# `strip` (from build-base) trims the shipped binary.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    touch src/lib.rs src/main.rs \
 && cargo build --release --locked \
 && strip /app/target/release/oxidav

# ===========================================================================
# Stage 3 — minimal runtime, non-root
# ===========================================================================
FROM alpine:3.23 AS runtime

# ca-certificates: needed for HTTPS to Pocket ID (OIDC discovery + JWKS).
# SQLite and the TLS stack are statically linked into the musl binary.
# The non-root account uses busybox `addgroup`/`adduser` (Alpine ships neither
# the coreutils account tools nor an interactive shell); `-H` skips the home dir
# and `-s /sbin/nologin` locks interactive login. The healthcheck below relies
# on busybox `wget`.
RUN apk add --no-cache ca-certificates \
 && addgroup -g 10001 oxidav \
 && adduser -D -u 10001 -G oxidav -H -s /sbin/nologin oxidav

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

# Readiness probe against `/healthz`. Uses the busybox `wget` shipped with
# `alpine:3.23` (there is no shell beyond busybox, and no curl in the image);
# `--spider` exits non-zero when the service does not answer, which is exactly
# what HEALTHCHECK wants. Assumes the documented default port 8080. Override the
# healthcheck in your orchestrator if you change OXIDAV_BIND_ADDR.
#
# NOTE: `HEALTHCHECK` is a Docker-specific instruction. `docker build` records
# it; Podman only does so with `--format docker` (its default OCI manifest drops
# it). The example `compose.yaml` therefore declares an equivalent healthcheck.
# If the port is customised, adjust both the probe and the published port.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD ["wget","-q","--spider","http://127.0.0.1:8080/healthz"]

ENTRYPOINT ["/usr/local/bin/oxidav"]
