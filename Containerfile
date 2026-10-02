# Pinned multi-architecture Rust/Alpine builder (amd64 and arm64).
ARG RUST_IMAGE=docker.io/library/rust:alpine@sha256:a96ea6d18d4062e38f16cfbadd8b4541d622f2527dd0a5eca1fb36d301da4e88
FROM ${RUST_IMAGE} AS build
RUN apk add --no-cache build-base
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
# Alpine's native Rust target is musl on both supported architectures.
RUN cargo build --locked --release

FROM scratch
ARG VERSION=0.1.2
ARG REVISION=unknown
LABEL org.opencontainers.image.title="sms-discord-relay"       org.opencontainers.image.description="Authenticated Android SMS Gateway to Discord webhook relay"       org.opencontainers.image.source="https://github.com/lucination/sms-discord-relay"       org.opencontainers.image.version="${VERSION}"       org.opencontainers.image.revision="${REVISION}"
COPY --from=build /build/target/release/sms-discord-relay /sms-discord-relay
USER 65532:65532
ENV BIND_ADDR=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 CMD ["/sms-discord-relay", "--healthcheck"]
ENTRYPOINT ["/sms-discord-relay"]
