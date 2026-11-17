FROM rust:1.100.0-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS build
WORKDIR /build
ENV CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
COPY config ./config
COPY assets ./assets
RUN mkdir -p /build/artifacts && cargo build --release --locked --package stories-app --bin stories-tech

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
WORKDIR /app
COPY --from=build /build/target/release/stories-tech /app/stories-tech
COPY config /app/config
COPY assets/fonts/LICENSE.txt /app/licenses/Inter-OFL.txt
COPY --from=build --chown=65532:65532 /build/artifacts /app/artifacts
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/app/stories-tech"]
