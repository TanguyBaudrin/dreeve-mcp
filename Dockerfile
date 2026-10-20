# syntax=docker/dockerfile:1

###############################################################################
# Build stage — static musl binary (rust:alpine targets *-unknown-linux-musl)
###############################################################################
FROM rust:1-alpine AS builder

# musl toolchain + native build tools (C dependency builds such as aws-lc-sys)
RUN apk add --no-cache build-base cmake perl ca-certificates

WORKDIR /app

# Warm the dependency cache: this layer only reruns when the manifests change.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src

# Real sources
COPY src ./src
RUN touch src/main.rs && cargo build --release --locked

###############################################################################
# Runtime stage — scratch: the binary is fully static, nothing else is needed
###############################################################################
FROM scratch

# CA roots so rustls can verify TLS to the Dreeve instance.
# Mount your own bundle (or set SSL_CERT_FILE) to trust a private CA.
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

COPY --from=builder /app/target/release/dreeve-mcp /dreeve-mcp

ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt

# scratch has no passwd file; run as a plain numeric non-root user.
USER 65532:65532

EXPOSE 8080
ENTRYPOINT ["/dreeve-mcp"]
