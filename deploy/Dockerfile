# syntax=docker/dockerfile:1.7
ARG RYFRAME_BUILD_COMMIT
ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot@sha256:c31ff9abcb1910f3ab25c7957bdaf0bfe12a01eb546e8df2282f1c8f682b606c

FROM ${RUST_IMAGE} AS builder
ARG RYFRAME_BUILD_COMMIT

WORKDIR /workspace
COPY . .
RUN test -n "${RYFRAME_BUILD_COMMIT}" \
    && printf '%s\n' "${RYFRAME_BUILD_COMMIT}" | grep -Eq '^[0-9a-f]{40}$'
RUN --mount=type=cache,id=ryframe-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=ryframe-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=ryframe-target,target=/workspace/target,sharing=locked \
    RYFRAME_BUILD_COMMIT="${RYFRAME_BUILD_COMMIT}" \
    cargo build --locked --release --no-default-features -p ryframe \
    --features bin-api,runtime-swagger-ui --bin ryframe \
    && RYFRAME_BUILD_COMMIT="${RYFRAME_BUILD_COMMIT}" \
    cargo build --locked --release --no-default-features -p ryframe \
    --features bin-migrate --bin ryframe-migrate \
    && RYFRAME_BUILD_COMMIT="${RYFRAME_BUILD_COMMIT}" \
    cargo build --locked --release --no-default-features -p ryframe \
    --features bin-worker --bin ryframe-worker \
    && install --directory /workspace/out /workspace/runtime-data/uploads \
    && install --mode=0755 /workspace/target/release/ryframe /workspace/out/ryframe \
    && install --mode=0755 /workspace/target/release/ryframe-migrate /workspace/out/ryframe-migrate \
    && install --mode=0755 /workspace/target/release/ryframe-worker /workspace/out/ryframe-worker

FROM ${RUNTIME_IMAGE} AS runtime
ARG RYFRAME_BUILD_COMMIT

LABEL org.opencontainers.image.revision="${RYFRAME_BUILD_COMMIT}"

WORKDIR /opt/ryframe
COPY --from=builder /workspace/out/ryframe /usr/local/bin/ryframe
COPY --from=builder /workspace/out/ryframe-migrate /usr/local/bin/ryframe-migrate
COPY --from=builder /workspace/out/ryframe-worker /usr/local/bin/ryframe-worker
COPY --from=builder --chown=10001:10001 /workspace/runtime-data/ /var/lib/ryframe/
COPY --chown=10001:10001 config ./config
COPY --chown=10001:10001 locales ./locales

ENV APP_ENV=prod \
    APP_CONFIG_DIR=/opt/ryframe/config \
    APP_OBJECT_STORAGE_LOCAL_BASE_DIR=/var/lib/ryframe/uploads
EXPOSE 8080
USER 10001:10001

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/ryframe", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/ryframe"]
