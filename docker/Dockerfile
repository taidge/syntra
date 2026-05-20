# Stage 1: Build
FROM rust:1.83-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p contrix-server

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/contrix-server /app/contrix-server
COPY config/ /app/config/
COPY migrations/ /app/migrations/
COPY static/ /app/static/
EXPOSE 8080
ENV RUST_LOG=info
CMD ["/app/contrix-server"]
