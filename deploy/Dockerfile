# syntax=docker/dockerfile:1.7
ARG RYFRAME_BUILD_COMMIT

FROM rust:bookworm AS builder
ARG RYFRAME_BUILD_COMMIT

WORKDIR /workspace
COPY . .
RUN RYFRAME_BUILD_COMMIT="${RYFRAME_BUILD_COMMIT}" \
    cargo build --locked --release -p ryframe --bin ryframe

FROM debian:bookworm-slim AS runtime
ARG RYFRAME_BUILD_COMMIT

LABEL org.opencontainers.image.revision="${RYFRAME_BUILD_COMMIT}"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/ryframe ryframe \
    && install --directory --owner=ryframe --group=ryframe /var/lib/ryframe/uploads

WORKDIR /opt/ryframe
COPY --from=builder /workspace/target/release/ryframe /usr/local/bin/ryframe
COPY --chown=ryframe:ryframe config ./config
COPY --chown=ryframe:ryframe locales ./locales

ENV APP_ENV=prod \
    APP_OBJECT_STORAGE_LOCAL_BASE_DIR=/var/lib/ryframe/uploads
EXPOSE 8080
VOLUME ["/var/lib/ryframe/uploads"]
USER ryframe

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:8080/readyz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/ryframe"]
