FROM rust:1.100.0-slim-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
COPY config ./config
RUN cargo build --release --locked --package stories-app --bin stories-tech

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=build /build/target/release/stories-tech /app/stories-tech
COPY config /app/config
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/app/stories-tech"]
