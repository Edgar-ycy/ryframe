# syntax=docker/dockerfile:1.7
ARG RYFRAME_BUILD_COMMIT
ARG RUST_IMAGE=rust:1.97.1-bookworm@sha256:0e2bcaef56d041a486784e54104a81aebe0da44bd03019bd70bc0401e42e4a97
ARG DEBIAN_IMAGE=debian:12-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241

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
    --bin ryframe --bin ryframe-migrate --bin ryframe-worker \
    && install --directory /workspace/out \
    && install --mode=0755 /workspace/target/release/ryframe /workspace/out/ryframe \
    && install --mode=0755 /workspace/target/release/ryframe-migrate /workspace/out/ryframe-migrate \
    && install --mode=0755 /workspace/target/release/ryframe-worker /workspace/out/ryframe-worker

FROM ${DEBIAN_IMAGE} AS runtime
ARG RYFRAME_BUILD_COMMIT

LABEL org.opencontainers.image.revision="${RYFRAME_BUILD_COMMIT}"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /var/lib/ryframe ryframe \
    && install --directory --owner=ryframe --group=ryframe /var/lib/ryframe/uploads

WORKDIR /opt/ryframe
COPY --from=builder /workspace/out/ryframe /usr/local/bin/ryframe
COPY --from=builder /workspace/out/ryframe-migrate /usr/local/bin/ryframe-migrate
COPY --from=builder /workspace/out/ryframe-worker /usr/local/bin/ryframe-worker
COPY --chown=ryframe:ryframe config ./config
COPY --chown=ryframe:ryframe locales ./locales

ENV APP_ENV=prod \
    APP_CONFIG_DIR=/opt/ryframe/config \
    APP_OBJECT_STORAGE_LOCAL_BASE_DIR=/var/lib/ryframe/uploads
EXPOSE 8080
USER ryframe

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:8080/readyz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/ryframe"]
