# Ekurl URL Shortener

A high-performance, low-resource URL shortener written in Rust (2024 edition). It uses `axum` for the web server and `SQLite` (via `rusqlite`) for an embedded database.

## Features
- Fast and lightweight.
- API for shortening URLs.
- Web UI for easy use.
- CLI for managing URLs (add, remove, list, count).
- Optional custom short codes.
- URL expiry with automatic cleanup (on startup and hourly while running).
- Abuse protection: `http`/`https`-only links, URL length cap, per-IP rate limiting, and security response headers.
- Static Musl build in a `scratch` Docker image (minimal footprint).

## Configuration

The following environment variables can be used to configure the application:

| Variable | Description | Default |
|----------|-------------|---------|
| `PORT` | The port the server binds to | `8080` |

The SQLite database path is currently fixed at `data/ekurl.db` (created automatically). Mount or persist the `data/` directory to keep links across restarts.

## Security

- **Scheme allowlist:** only `http` and `https` URLs can be shortened. Schemes like `javascript:`, `data:`, and `file:` are rejected with `400` so a short link can't execute script or open local files when followed.
- **URL length limit:** URLs longer than 2048 characters are rejected.
- **Rate limiting:** the `POST /api/v1/shorten` endpoint is rate-limited per client IP (burst of 10, replenishing one slot every 2 seconds). Redirects and static assets are not throttled.
  - The limiter uses the connection peer IP. If you run behind a reverse proxy, every request appears to come from the proxy, so the limit becomes effectively global. Terminate the limiter at your proxy, or adjust the key extractor in `create_router_with_rate_limit`, for true per-client limits.
- **Security headers:** all responses include `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, and a restrictive `Content-Security-Policy`.

## Expiry & cleanup

Each link may have an expiry (see `expires_in` below). Expired links are hidden from reads immediately and are physically purged from the database both on startup and once per hour while the server runs.

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
       environment:
         - PORT=8080
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
2. The server will start on `http://0.0.0.0:8080` (or `PORT`).

### Testing & checks
Run the test suite and lints before opening a pull request (CI runs the same checks):
```bash
cargo test --all
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
```
See [CONTRIBUTING.md](CONTRIBUTING.md) for the full workflow.

## CLI Management
You can manage URLs directly from the command line using the `ekurl` binary.

### Commands
- `ekurl add <url> [--code <custom>]`: Add a new short link.
- `ekurl remove <code>`: Remove a short link.
- `ekurl list`: List all short links.
- `ekurl count`: Show total number of links.
- `ekurl help`: Show help message.

### Managing via Docker
The `ekurl` binary is available in the container's `PATH`.

**List all URLs:**
```bash
docker exec -it <container_name> ekurl list
```

**Add a URL:**
```bash
docker exec -it <container_name> ekurl add https://google.com --code google
```

**Remove a URL:**
```bash
docker exec -it <container_name> ekurl remove google
```

**Get Count:**
```bash
docker exec -it <container_name> ekurl count
```

## API Documentation

### Shorten URL
`POST /api/v1/shorten`

**Request:**
```json
{
  "url": "https://example.com/very-long-url",
  "custom_code": "my-link",
  "expires_in": "1d"
}
```

`custom_code` and `expires_in` are optional. Valid `expires_in` values: `1d`, `7d` (default), `1mo`, `3mo`, `6mo`, `1y`, `never`.

The `url` must be a well-formed `http`/`https` URL no longer than 2048 characters; other schemes or malformed/over-length URLs return `400`. Requests beyond the rate limit return `429`.

If `custom_code` is omitted, a code is auto-generated from lowercase letters and digits (`a-z0-9`), starting at 3 characters. The length is bumped up after repeated collisions.

**Response (201 Created):**
```json
{
  "code": "my-link",
  "expires_at": 1735689600
}
```

`expires_at` is a Unix timestamp, or `null` if `expires_in` was `never`.

## GitHub Actions
- **CI** ([`ci.yml`](.github/workflows/ci.yml)): on every push and pull request to `main`, runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, and a `cargo audit` security scan.
- **Publish** ([`docker-publish.yml`](.github/workflows/docker-publish.yml)): builds and pushes the Docker image to GHCR when a tag starting with `v` (e.g., `v1.0.0`) is pushed.
