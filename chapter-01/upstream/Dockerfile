# syntax=docker/dockerfile:1.7
FROM rust:1.86-slim AS build
WORKDIR /src
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml ./
COPY controlplane ./controlplane
COPY upstream ./upstream

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --bin upstream && \
    cp /src/target/release/upstream /upstream

FROM debian:bookworm-slim
COPY --from=build /upstream /usr/local/bin/upstream
ENV RUST_LOG=info
EXPOSE 9000
ENTRYPOINT ["/usr/local/bin/upstream"]
