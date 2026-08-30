# admin-agent/Dockerfile

FROM rust:1.82-alpine3.24 AS builder
WORKDIR /app

RUN apk add --no-cache musl-dev

COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release
RUN rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM alpine:3.24

RUN apk --no-cache add \
    ca-certificates \
    docker-cli \
    curl \
    iproute2 \
    util-linux \
    && rm -rf /var/cache/apk/*

# ⬇️ AGREGAR ESTA LÍNEA: Actualizar openssl a la última versión parcheada
RUN apk upgrade --no-cache openssl

WORKDIR /root/
COPY --from=builder /app/target/release/admin-agent .
COPY monitored_sites.json .

CMD ["./admin-agent"]