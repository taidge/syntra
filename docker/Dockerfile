# Build all runtime assets with the project's supported toolchain.
FROM node:22-bookworm-slim@sha256:e9f565d32859f0cbfb52125eafe8be39390f24dac302dc3cdf566e8ba3ab21ee AS pdf-assets
WORKDIR /web-build
COPY crates/frontend/package.json crates/frontend/package-lock.json ./crates/frontend/
RUN npm ci --prefix crates/frontend --ignore-scripts --no-audit --no-fund
COPY crates/frontend/build-pdf-assets.mjs ./crates/frontend/
RUN node crates/frontend/build-pdf-assets.mjs /web-assets/assets

FROM rust:1.99.1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS builder
RUN apt-get update && apt-get install -y --no-install-recommends python3 cmake && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY . .
RUN rustup target add wasm32-unknown-unknown
RUN cargo build --locked --release -p syntra-server && cargo build --locked --release -p syntra-frontend --lib --target wasm32-unknown-unknown
RUN version="$(python3 -c 'import tomllib; print(next(p["version"] for p in tomllib.load(open("Cargo.lock","rb"))["package"] if p["name"]=="wasm-bindgen"))')" && cargo install wasm-bindgen-cli --locked --version "$version"
RUN target="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')" && mkdir -p /app/distribution/static/assets && cp "$target/release/syntra-server" /app/distribution/ && wasm-bindgen "$target/wasm32-unknown-unknown/release/syntra_frontend.wasm" --target web --out-dir /app/distribution/static/assets --out-name syntra_frontend && cp -r static/. /app/distribution/static/ && cp -r crates/frontend/assets/. /app/distribution/static/assets/ && cp .dependencies/sapid/styles/sapid.css /app/distribution/static/assets/

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libssl3 chromium chromium-sandbox fonts-liberation && rm -rf /var/lib/apt/lists/* && useradd --uid 10001 --create-home syntra && mkdir /data && chown syntra:syntra /data
WORKDIR /app
COPY --from=builder /app/distribution/ /app/
COPY --from=pdf-assets /web-assets/assets/pdf-vendor/ /app/static/assets/pdf-vendor/
COPY config/ /app/config/
COPY migrations/ /app/migrations/
ENV RUST_LOG=info SYNTRA_ENV=production
USER syntra
EXPOSE 8080
CMD ["/app/syntra-server"]
