# ============================================
# BUILDER - Compila Rust + whisper.cpp
# ============================================
FROM rust:1-bookworm AS builder

WORKDIR /app

# Instalar TODAS las dependencias de compilación
RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    libssl-dev \
    clang \
    libclang-dev \
    cmake \
    && rm -rf /var/lib/apt/lists/*

# Copiar manifests para cache de dependencias
COPY Cargo.toml Cargo.lock* ./

# Crear dummy main para compilar dependencias (cache layer)
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release

# Limpiar dummy y copiar código real
RUN rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release

# ============================================
# RUNTIME - Contenedor final
# ============================================
FROM debian:bookworm-slim

WORKDIR /app

# Instalar runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    curl \
    ffmpeg \
    && rm -rf /var/lib/apt/lists/*

# Crear directorio para modelos
RUN mkdir -p /app/models

# Descargar modelo Whisper small (~466MB, rápido en CPU)
RUN curl -L \
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin" \
    -o /app/models/ggml-base.bin

# Copiar binario compilado
COPY --from=builder /app/target/release/socket-core /app/socket-core

EXPOSE 3005

CMD ["./socket-core"]