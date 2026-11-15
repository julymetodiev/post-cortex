# Build stage
FROM rust:1.83-bookworm as builder

WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Create dummy files to build dependencies
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    echo "fn main() {}" > src/lib.rs && \
    mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/pcx.rs

# Build dependencies
RUN cargo build --release

# Copy source code
COPY . .

# Touch main files to force rebuild
RUN touch src/main.rs src/lib.rs src/bin/*.rs

# Build the actual application
RUN cargo build --release --bin pcx

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy binary from builder
COPY --from=builder /app/target/release/pcx /usr/local/bin/

# Create data directory
RUN mkdir -p /root/.post-cortex/data

# Set environment variables
ENV PC_DATA_DIR=/root/.post-cortex/data
ENV RUST_LOG=info

# Default entrypoint is the stdio server (auto-starts daemon internally)
ENTRYPOINT ["pcx"]
