# opsctl-server — single-binary image (SPA embedded via rust-embed at build time).
#
#   docker build -f deploy/Dockerfile -t opsctl-server:latest .
# Build context must be the workspace root (opsctl/).

# --- stage 1: build the Vue SPA → web/dist ---
FROM node:20-bookworm-slim AS web
WORKDIR /web
COPY web/package.json web/package-lock.json* ./
RUN npm ci || npm install
COPY web/ ./
RUN npm run build

# --- stage 2: build the Rust server (embeds web/dist in release) ---
FROM rust:1-bookworm AS build
WORKDIR /src
# workspace manifests + sources
COPY Cargo.toml Cargo.lock ./
COPY core/ core/
COPY server/ server/
COPY client/ client/
# the built SPA must sit at web/dist (rust-embed folder = "../web/dist" from server/)
COPY --from=web /web/dist web/dist
# build only the server package (client/egui is not a dependency)
RUN cargo build --release -p opsctl-server

# --- stage 3: slim runtime ---
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=build /src/target/release/opsctl-server /usr/local/bin/opsctl-server
# sqlite lives here; mount a volume to persist it
RUN mkdir -p /app/data
VOLUME ["/app/data"]

ENV OPSCTL_SERVER__BIND=0.0.0.0:8443 \
    OPSCTL_STORE__URL=sqlite://data/opsctl.db?mode=rwc \
    OPSCTL_DEV__SEED=false
# Provide these at runtime (do NOT bake secrets into the image):
#   OPSCTL_AUTH__JWT_SECRET, OPSCTL_VAULT__PASSPHRASE, OPSCTL_BOOTSTRAP__ADMIN_PASSWORD

EXPOSE 8443
CMD ["opsctl-server"]
