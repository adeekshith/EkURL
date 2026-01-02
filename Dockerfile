# Builder stage
FROM rust:alpine AS builder

RUN apk add --no-cache musl-dev

WORKDIR /app

# Copy the source code
COPY Cargo.toml ./
COPY src ./src
COPY static ./static

# Build the application
RUN cargo build --release

# Final stage
FROM scratch

WORKDIR /app

# Copy the binary and static files
COPY --from=builder /app/target/release/ekurl /app/ekurl
COPY --from=builder /app/static /app/static

EXPOSE 8080

ENTRYPOINT ["/app/ekurl"]