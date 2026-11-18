FROM gcr.io/distroless/cc-debian13:nonroot@sha256:cb87acc574abe719b6e5c547384defa2f7b1f24af9605d3f618cf852495d8be2 AS runtime-libs
FROM rust:1.100.0-slim-bookworm@sha256:7c7d4eb4d05b7781187a327809466cfddf29753f6defdc3d19a0e291951b7a37 AS build
WORKDIR /build
ENV CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY migrations ./migrations
COPY config ./config
COPY assets ./assets
RUN cargo install cargo-auditable --version 0.7.7 --locked
RUN mkdir -p /build/artifacts && cargo auditable build --release --locked --package stories-app --bin stories-tech

FROM gcr.io/distroless/base-nossl-debian13:nonroot@sha256:89815d620ac01d8de13cdaf04d19ed874447ca2296d4985b14b818f6e5e47854
WORKDIR /app
# O executável usa glibc e libgcc, com TLS em Rustls; OpenSSL não participa da imagem.
COPY --from=runtime-libs /usr/lib/*/libgcc_s.so.1 /app/lib/libgcc_s.so.1
COPY --from=runtime-libs /usr/share/doc/gcc-14-base/copyright /app/licenses/libgcc-copyright
COPY --from=runtime-libs /var/lib/dpkg/status.d/libgcc-s1 /var/lib/dpkg/status.d/libgcc-s1
COPY --from=runtime-libs /var/lib/dpkg/status.d/libgcc-s1.md5sums /var/lib/dpkg/status.d/libgcc-s1.md5sums
ENV LD_LIBRARY_PATH=/app/lib
COPY --from=build /build/target/release/stories-tech /app/stories-tech
COPY config /app/config
COPY assets/fonts/LICENSE.txt /app/licenses/Inter-OFL.txt
COPY assets/licenses/libgcc /app/licenses/libgcc
COPY --from=build --chown=65532:65532 /build/artifacts /app/artifacts
USER 65532:65532
EXPOSE 8080
ENTRYPOINT ["/app/stories-tech"]
