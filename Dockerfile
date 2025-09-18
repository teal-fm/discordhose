# Use the official Rust image
FROM rust:1.75-slim as builder

# Set working directory
WORKDIR /app

# Copy manifest files
COPY Cargo.toml Cargo.lock ./

# Copy source code
COPY src ./src

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Create app user
RUN useradd -r -s /bin/false appuser

# Set working directory
WORKDIR /app

# Copy the binary from builder stage
COPY --from=builder /app/target/release/discordhose /app/discordhose

# Change ownership to app user
RUN chown -R appuser:appuser /app

# Switch to app user
USER appuser

# Run the application
CMD ["./discordhose"]
