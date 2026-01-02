# Ekurl URL Shortener

A high-performance, low-resource URL shortener written in Rust, using the 2021 edition. It uses `axum` for the web server and `redb` for a pure-Rust embedded database.

## Features
- Fast and lightweight.
- API for shortening URLs.
- Web UI for easy use.
- CLI for managing URLs (add, remove, list, count).
- Optional custom short codes.
- Static Musl build in a `scratch` Docker image (minimal footprint).

## Getting Started

### Prerequisites
- Docker and Docker Compose (recommended)
- Rust (if running locally)

### Running with Docker Compose (Local Build)
1. Clone the repository.
2. Run:
   ```bash
   docker-compose up -d
   ```
3. Access the UI at `http://localhost:8080`.

### Running with Docker Compose (GHCR Image)
If you prefer to use the pre-built image:

1. Create a `docker-compose.yml` file:
   ```yaml
   services:
     ekurl:
       image: ghcr.io/adeekshith/ekurl:latest
       ports:
         - "8080:8080"
       volumes:
         - ./data:/app/data
       restart: unless-stopped
   ```
2. Run:
   ```bash
   docker-compose up -d
   ```

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

## CLI Management
You can manage URLs directly from the command line using the `ekurl` binary.

### Commands
- `ekurl add <url> [--code <custom>]`: Add a new short link.
- `ekurl remove <code>`: Remove a short link.
- `ekurl list`: List all short links.
- `ekurl count`: Show total number of links.
- `ekurl help`: Show help message.

### Managing via Docker
Since the Docker image is built from `scratch`, you must invoke the binary directly to run commands inside the container.

**List all URLs:**
```bash
docker exec -it <container_name> /app/ekurl list
```

**Add a URL:**
```bash
docker exec -it <container_name> /app/ekurl add https://google.com --code google
```

**Remove a URL:**
```bash
docker exec -it <container_name> /app/ekurl remove google
```

**Get Count:**
```bash
docker exec -it <container_name> /app/ekurl count
```

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
