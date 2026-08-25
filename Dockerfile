# syntax=docker/dockerfile:1.7

FROM rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
COPY catalog ./catalog

# Cargo.lock is mandatory so a clean build resolves the reviewed dependency graph.
RUN cargo build --locked --release --all-features --bin rockserver --bin import_shared_catalog

FROM debian:bookworm-slim@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home --shell /usr/sbin/nologin rockserver \
    && mkdir -p /var/log/rockserver \
    && chown rockserver:rockserver /var/log/rockserver

COPY --from=builder /build/target/release/rockserver /usr/local/bin/rockserver
COPY --from=builder /build/target/release/import_shared_catalog /usr/local/bin/import_shared_catalog

USER rockserver
EXPOSE 3000
HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=6 \
    CMD curl --fail --silent http://127.0.0.1:3000/health/live > /dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/rockserver"]
