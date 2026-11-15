# syntax=docker/dockerfile:1
# Multi-stage build for Ferrum. Produces the UCI binary and the shared
# library in one image.
#
#   docker build -t ferrum .
#   docker run -i --rm ferrum                      # UCI on stdin/stdout
#   docker build --build-arg CARGO_FLAGS=--no-default-features -t ferrum-alpha .
#
# The runtime image is Debian slim because the default Rust target is
# dynamically linked against glibc. For a scratch/musl image, build with
# `--target x86_64-unknown-linux-musl` and copy the static binary instead.

FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
ARG CARGO_FLAGS=""
RUN cargo build --release ${CARGO_FLAGS}

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --create-home ferrum
COPY --from=build /src/target/release/ferrum /usr/local/bin/ferrum
COPY --from=build /src/target/release/libferrum.so /usr/local/lib/libferrum.so
# Cap per-instance memory in containers; override at run time if needed.
ENV FERRUM_HASH_MB=64
USER ferrum
ENTRYPOINT ["ferrum"]
