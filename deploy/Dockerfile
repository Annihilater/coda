# Multi-stage Dockerfile for Coda Coordinator + Stateless Domestic No-ICP Relay
FROM oven/bun:1-alpine AS web-builder
WORKDIR /app
COPY assets ./assets
COPY web/package.json web/bun.lock ./web/
WORKDIR /app/web
RUN bun install --frozen-lockfile
COPY web/build.ts web/index.html web/manifest.json ./
COPY web/src ./src
RUN bun run build && test -f dist/manifest.json && test -f dist/sw.js

FROM rust:bookworm AS rust-builder
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libzstd-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
COPY deploy/install-daemon.sh ./deploy/install-daemon.sh
RUN cargo build --release --locked -p coda-server -p coda-daemon

FROM debian:bookworm-slim
LABEL org.opencontainers.image.source="https://github.com/Annihilater/coda"
LABEL org.opencontainers.image.description="Coda Zero-Knowledge AI Agent Mesh Coordinator & Stateless Relay"
LABEL org.opencontainers.image.licenses="MIT"
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl git libzstd1 && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 coda \
    && useradd --system --uid 10001 --gid coda --home-dir /opt/coda --create-home --shell /usr/sbin/nologin coda
WORKDIR /opt/coda
COPY --from=rust-builder /app/target/release/coda-server /usr/local/bin/coda-server
COPY --from=rust-builder /app/target/release/coda-daemon /usr/local/bin/coda-daemon
COPY --from=web-builder /app/web/dist /opt/coda/web/dist
COPY migrations /opt/coda/migrations
COPY deploy /opt/coda/deploy
RUN chown -R coda:coda /opt/coda && chmod 0755 /opt/coda/deploy/*.sh
USER coda
EXPOSE 8080 24443
ENTRYPOINT ["coda-server"]
CMD ["--web-dist", "/opt/coda/web/dist"]
