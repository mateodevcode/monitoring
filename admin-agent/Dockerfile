# rust-admin-agent/Dockerfile
FROM rust:1-alpine AS builder
WORKDIR /app

RUN apk add --no-cache musl-dev

COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM alpine:latest
RUN apk --no-cache add ca-certificates docker-cli curl iproute2

WORKDIR /root/
COPY --from=builder /app/target/release/rust-admin-agent .

# 👇 AGREGA ESTA LÍNEA: Copia el JSON al contenedor para que el agente pueda leerlo
COPY monitored_sites.json .

CMD ["./rust-admin-agent"]