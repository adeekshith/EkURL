# Ekurl URL Shortener

A high-performance, low-resource URL shortener written in Rust, using the 2024 edition. It uses `axum` for the web server and `redb` for a pure-Rust embedded database.

## Features
- Fast and lightweight.
- API for shortening URLs.
- Web UI for easy use.
- Optional custom short codes.
- Static Musl build in a `scratch` Docker image (minimal footprint).

## Getting Started

### Prerequisites
- Docker and Docker Compose (recommended)
- Rust (if running locally)

### Running with Docker Compose
1. Clone the repository.
2. Run:
   ```bash
   docker-compose up -d
   ```
3. Access the UI at `http://localhost:8080`.

### Running with Docker
1. Build the image:
   ```bash
   docker build -t ekurl .
   ```
2. Run the container:
   ```bash
   docker run -p 8080:8080 -v $(pwd)/data:/app/data ekurl
   ```

### Local Development
1. Build and run:
   ```bash
   cargo run
   ```
2. The server will start on `http://0.0.0.0:8080`.

## API Documentation

### Shorten URL
`POST /api/v1/shorten`

**Request:**
```json
{
  "url": "https://example.com/very-long-url",
  "custom_code": "my-link" (optional)
}
```

**Response (201 Created):**
```json
{
  "code": "my-link"
}
```

## GitHub Actions
The project includes a workflow to automatically build and push the Docker image to GHCR when a new tag starting with `v` (e.g., `v1.0.0`) is pushed.
