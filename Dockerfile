# Builder stage
FROM rust:1.83-alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

# Copy the source code
COPY Cargo.toml ./
COPY src ./src
COPY static ./static

# Build the application
RUN cargo build --release --target x86_64-unknown-linux-musl

# Final stage
FROM scratch

WORKDIR /app

# Copy the binary and static files
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/ekurl /app/ekurl
COPY --from=builder /app/static /app/static

EXPOSE 8080

ENTRYPOINT ["/app/ekurl"]