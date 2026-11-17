FROM rust:1.100.0-slim-bookworm AS build
WORKDIR /build
ENV CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
COPY config ./config
COPY assets ./assets
RUN mkdir -p /build/artifacts && cargo build --release --locked --package stories-app --bin stories-tech

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=build /build/target/release/stories-tech /app/stories-tech
COPY config /app/config
COPY assets/fonts/LICENSE.txt /app/licenses/Inter-OFL.txt
COPY --from=build --chown=65532:65532 /build/artifacts /app/artifacts
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/app/stories-tech"]
