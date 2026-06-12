# Builder stage
FROM rust:alpine AS builder

RUN apk add --no-cache musl-dev build-base

WORKDIR /app

# Copy the source code
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY static ./static

# Build the application against the locked dependency versions
RUN cargo build --release --locked

# Final stage
FROM scratch

WORKDIR /app

# Copy the binary and static files
COPY --from=builder /app/target/release/ekurl /usr/local/bin/ekurl
COPY --from=builder /app/static /app/static

ENV PATH="/usr/local/bin:${PATH}"

EXPOSE 8080

ENTRYPOINT ["ekurl"]