# Multi-stage build for the OVIS single-binary image.
#
# The UI is compiled first and embedded into the Rust binary by `rust-embed`, so
# the runtime image is one static-ish executable plus CA certificates.

# ---------------------------------------------------------------------------
# Stage 1 — the web UI
# ---------------------------------------------------------------------------
FROM node:22-bookworm-slim AS ui-builder
WORKDIR /app/ui

COPY ui/package*.json ./
RUN npm ci || npm install

COPY ui/ ./
RUN npm run build

# ---------------------------------------------------------------------------
# Stage 2 — the Rust binary
# ---------------------------------------------------------------------------
FROM rust:1-slim-bookworm AS rust-builder
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY ops ./ops
COPY tests ./tests

# rust-embed reads ui/dist at compile time, so it must exist before the build.
COPY --from=ui-builder /app/ui/dist ./ui/dist

# Build info comes from the environment here: the build context carries no git
# metadata, and `build.rs` falls back to "unknown" rather than failing.
ARG OVIS_GIT_SHA=unknown
ENV OVIS_GIT_SHA=${OVIS_GIT_SHA}

# Both binaries: the server, and the CLI (which also embeds the UI).
RUN cargo build --release --locked --bin ovis-backend --bin ovis

# ---------------------------------------------------------------------------
# Stage 3 — runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime
WORKDIR /app

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Run unprivileged: nothing here needs root.
RUN useradd --system --create-home --shell /usr/sbin/nologin ovis

COPY --from=rust-builder /app/target/release/ovis-backend /usr/local/bin/ovis-backend
COPY --from=rust-builder /app/target/release/ovis /usr/local/bin/ovis
# The index migration ships with the image so it can be applied from here.
COPY --from=rust-builder /app/ops /app/ops

# No credentials are baked in. DATABASE_URL and OPENSEARCH_URL are *required* and
# the server exits non-zero with a clear message if they are unset — a default
# would only ever point at the wrong database.
ENV OVIS_HOST=0.0.0.0
ENV OVIS_PORT=8080

USER ovis
EXPOSE 8080

# /api/v1/system/health answers 503 when a dependency is down, so this check is
# meaningful. The previous one hit an endpoint that returned 200 even when
# Postgres was dead.
HEALTHCHECK --interval=15s --timeout=5s --start-period=20s --retries=3 \
  CMD curl -fsS http://localhost:8080/api/v1/system/health >/dev/null || exit 1

# SIGTERM is honoured: in-flight requests drain, bounded by
# OVIS_SHUTDOWN_GRACE_SECS.
STOPSIGNAL SIGTERM

ENTRYPOINT ["ovis-backend"]
