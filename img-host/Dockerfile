# ---- Build stage ----
FROM rust:1.97-alpine AS builder
RUN apk add --no-cache musl-dev
WORKDIR /build
COPY Cargo.toml ./
# Cache dependencies before copying source
RUN mkdir src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --target x86_64-unknown-linux-musl \
    && rm -rf src target/release/deps/img_host*
COPY src ./src
RUN touch src/main.rs \
    && cargo build --release --target x86_64-unknown-linux-musl

# ---- Runtime stage ----
FROM scratch
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/img-host /img-host
ENV LISTEN_ADDR=0.0.0.0:8080 \
    STORAGE_DIR=/data \
    MAX_AGE=1h \
    MAX_FILE_SIZE=500MB \
    MAX_TOTAL_SIZE=0 \
    BASE_URL=
EXPOSE 8080
VOLUME ["/data"]
CMD ["/img-host"]
