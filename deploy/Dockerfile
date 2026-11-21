# syntax=docker/dockerfile:1.7
ARG SOURCE_COMMIT
ARG RUST_IMAGE=rust:1.100.0-bookworm@sha256:ade39a710ef85f3506cffcfec3e8a24ba090badef1dff2fb798f71d878833fea
ARG RUNTIME_IMAGE=gcr.io/distroless/base-nossl-debian13:nonroot@sha256:8c563c1fb5e120606f0d85733049775faed6192e2bd2223ef283a5393eec22b9

FROM ${RUST_IMAGE} AS builder
ARG SOURCE_COMMIT

WORKDIR /workspace
COPY . .
RUN test -n "${SOURCE_COMMIT}" \
    && printf '%s\n' "${SOURCE_COMMIT}" | grep -Eq '^[0-9a-f]{40}$' \
    && install --directory /workspace/.local-tests/build \
    && printf '%s\n' "${SOURCE_COMMIT}" > /workspace/.local-tests/build/source-commit
RUN --mount=type=cache,id=ryframe-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=ryframe-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=ryframe-target,target=/workspace/target,sharing=locked \
    cargo build --locked --release --no-default-features -p ryframe \
    --features bin-api,runtime-swagger-ui --bin ryframe \
    && cargo build --locked --release --no-default-features -p ryframe \
    --features bin-migrate --bin ryframe-migrate \
    && cargo build --locked --release --no-default-features -p ryframe \
    --features bin-worker --bin ryframe-worker \
    && rm /workspace/.local-tests/build/source-commit \
    && install --directory /workspace/out /workspace/runtime-data/uploads \
    && install --mode=0755 /workspace/target/release/ryframe /workspace/out/ryframe \
    && install --mode=0755 /workspace/target/release/ryframe-migrate /workspace/out/ryframe-migrate \
    && install --mode=0755 /workspace/target/release/ryframe-worker /workspace/out/ryframe-worker

FROM ${RUNTIME_IMAGE} AS runtime
ARG SOURCE_COMMIT

LABEL org.opencontainers.image.revision="${SOURCE_COMMIT}"

WORKDIR /opt/ryframe
COPY --from=builder /workspace/out/ryframe /usr/local/bin/ryframe
COPY --from=builder /workspace/out/ryframe-migrate /usr/local/bin/ryframe-migrate
COPY --from=builder /workspace/out/ryframe-worker /usr/local/bin/ryframe-worker
COPY --from=builder /usr/lib/x86_64-linux-gnu/libgcc_s.so.1 /usr/lib/x86_64-linux-gnu/libgcc_s.so.1
COPY --from=builder --chown=10001:10001 /workspace/runtime-data/ /var/lib/ryframe/
COPY --chown=10001:10001 config ./config
COPY --chown=10001:10001 locales ./locales

ENV APP_ENV=prod
EXPOSE 8080
USER 10001:10001

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD ["/usr/local/bin/ryframe", "--healthcheck"]

ENTRYPOINT ["/usr/local/bin/ryframe"]
