# syntax=docker/dockerfile:1.7
FROM rust:1.86-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy the whole workspace so `xds-api` (a regular crates.io dep) resolves
# transitively. Cargo Lockfile is cached at the workspace root.
COPY Cargo.toml ./
COPY controlplane ./controlplane
COPY upstream ./upstream

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --bin controlplane && \
    cp /src/target/release/controlplane /controlplane

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /controlplane /usr/local/bin/controlplane
ENV RUST_LOG=info
EXPOSE 18000
ENTRYPOINT ["/usr/local/bin/controlplane"]
