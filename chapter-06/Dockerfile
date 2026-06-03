# syntax=docker/dockerfile:1.7
#
# One build stage compiles both workspace binaries, then we split into
# two minimal runtime images via build targets.

FROM rust:1.96-slim AS build
WORKDIR /src
# protobuf-compiler supplies protoc, which tonic-build uses to generate the
# OpenRcaService from the vendored ORCA protos.
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY controlplane ./controlplane
COPY upstream ./upstream

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --workspace && \
    cp /src/target/release/controlplane /controlplane && \
    cp /src/target/release/upstream /upstream

FROM debian:bookworm-slim AS controlplane
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /controlplane /usr/local/bin/controlplane
ENV RUST_LOG=info
EXPOSE 18000
ENTRYPOINT ["/usr/local/bin/controlplane"]

FROM debian:bookworm-slim AS upstream
COPY --from=build /upstream /usr/local/bin/upstream
ENV RUST_LOG=info
EXPOSE 9000
ENTRYPOINT ["/usr/local/bin/upstream"]
