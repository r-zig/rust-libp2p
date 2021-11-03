# 1: Build the exe
FROM rust:1 as builder
LABEL Name=libp2p-relay Version=0.0.1

# 1b: Download and compile Rust dependencies (and store as a separate Docker layer)
WORKDIR /usr/src/libp2p

COPY . .
RUN cargo build --example=relay_v? --package=libp2p-relay
# 2: Copy the exe and extra files ("static") to an empty Docker image
FROM debian:buster-slim

COPY --from=builder /usr/src/libp2p/target/debug/examples/relay_v? .
USER 1000