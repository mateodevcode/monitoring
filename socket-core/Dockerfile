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
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin" \
    -o /app/models/ggml-small.bin

# Copiar binario compilado
COPY --from=builder /app/target/release/socket-core /app/socket-core

EXPOSE 3005

CMD ["./socket-core"]