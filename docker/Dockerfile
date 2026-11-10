# Build all runtime assets with the project's supported toolchain.
FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS builder
RUN apt-get update && apt-get install -y --no-install-recommends python3 cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN rustup target add wasm32-unknown-unknown
RUN cargo build --locked --release -p contrix-server && cargo build --locked --release -p contrix-frontend --lib --target wasm32-unknown-unknown
RUN version="$(python3 -c 'import tomllib; print(next(p["version"] for p in tomllib.load(open("Cargo.lock","rb"))["package"] if p["name"]=="wasm-bindgen"))')" && cargo install wasm-bindgen-cli --locked --version "$version"
RUN target="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')" && mkdir -p /app/distribution/static/assets && cp "$target/release/contrix-server" /app/distribution/ && wasm-bindgen "$target/wasm32-unknown-unknown/release/contrix_frontend.wasm" --target web --out-dir /app/distribution/static/assets --out-name contrix_frontend && cp -r static/. /app/distribution/static/ && cp -r crates/frontend/assets/. /app/distribution/static/assets/ && cp .dependencies/sapid/styles/sapid.css /app/distribution/static/assets/

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home office && mkdir /data && chown office:office /data
WORKDIR /app
COPY --from=builder /app/distribution/ /app/
COPY config/ /app/config/
COPY migrations/ /app/migrations/
ENV RUST_LOG=info CONTRIX_ENV=production
USER office
EXPOSE 8080
CMD ["/app/contrix-server"]
