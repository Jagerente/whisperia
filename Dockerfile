# syntax=docker/dockerfile:1

FROM rust:1-slim-bookworm AS build
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p whisperia-server \
    && install -D target/release/whisperia-server /out/whisperia-server \
    && mkdir /out/data

FROM gcr.io/distroless/cc-debian12:nonroot

COPY --from=build /out/whisperia-server /usr/local/bin/whisperia-server
COPY LICENSE /usr/local/share/doc/whisperia-server/LICENSE
COPY --from=build --chown=nonroot:nonroot /out/data /data
VOLUME /data

ENV WHISPERIA_DATA_DIR=/data

EXPOSE 47820/udp
USER nonroot
ENTRYPOINT ["/usr/local/bin/whisperia-server"]
