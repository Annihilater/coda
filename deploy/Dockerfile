# Multi-stage Dockerfile for Coda Coordinator + Stateless Domestic No-ICP Relay
FROM oven/bun:1-alpine AS web-builder
WORKDIR /app/web
COPY web/package.json ./
COPY web/src ./src
COPY web/index.html ./
RUN mkdir -p dist && bun build ./src/app.ts --outdir ./dist --target browser --minify && cp index.html dist/index.html

FROM rust:1.85-bookworm AS rust-builder
RUN apt-get update && apt-get install -y pkg-config libzstd-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY migrations ./migrations
RUN cargo build --release -p coda-server -p coda-daemon

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates curl git libzstd1 && rm -rf /var/lib/apt/lists/*
WORKDIR /opt/coda
COPY --from=rust-builder /app/target/release/coda-server /usr/local/bin/coda-server
COPY --from=rust-builder /app/target/release/coda-daemon /usr/local/bin/coda-daemon
COPY --from=web-builder /app/web/dist /opt/coda/web/dist
COPY deploy/install-daemon.sh /opt/coda/deploy/install-daemon.sh
EXPOSE 8080 24443
ENTRYPOINT ["coda-server"]
CMD ["--http-bind", "0.0.0.0:8080", "--raw-tcp-bind", "0.0.0.0:24443", "--web-dist", "/opt/coda/web/dist"]
