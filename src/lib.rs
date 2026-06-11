use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use nanoid::nanoid;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_rusqlite::Connection;
use tower_governor::{GovernorLayer, governor::GovernorConfigBuilder};
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use url::Url;

/// Current Unix time in seconds. Falls back to `0` (and logs) if the system
/// clock is set before the Unix epoch, rather than panicking like a bare
/// `unwrap()` would.
pub fn now_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(_) => {
            tracing::error!("system clock is before UNIX_EPOCH; falling back to 0");
            0
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
}

#[derive(Clone)]
pub struct Db {
    pub conn: Connection,
}

impl Db {
    pub async fn new(path: &str) -> anyhow::Result<Self> {
        let conn = Connection::open(path).await?;

        conn.call(|conn| {
            // Enable WAL mode for concurrency
            conn.execute_batch("PRAGMA journal_mode=WAL;")?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS urls (
                    code TEXT PRIMARY KEY,
                    url TEXT NOT NULL,
                    expires_at INTEGER
                )",
                [],
            )?;

            // Migration: add expires_at column if missing (existing DBs)
            let has_expires_at: bool = conn
                .prepare("SELECT COUNT(*) FROM pragma_table_info('urls') WHERE name='expires_at'")?
                .query_row([], |row| row.get::<_, i64>(0))
                .map(|c| c > 0)?;
            if !has_expires_at {
                conn.execute("ALTER TABLE urls ADD COLUMN expires_at INTEGER", [])?;
            }

            // Index expiry so reads and cleanup don't full-scan as the table grows.
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_urls_expires_at ON urls(expires_at)",
                [],
            )?;

            Ok::<_, rusqlite::Error>(())
        })
        .await?;

        Ok(Self { conn })
    }

    pub async fn insert(
        &self,
        code: String,
        url: String,
        expires_at: Option<i64>,
    ) -> anyhow::Result<bool> {
        self.conn
            .call(move |conn| {
                match conn.execute(
                    "INSERT INTO urls (code, url, expires_at) VALUES (?1, ?2, ?3)",
                    rusqlite::params![code, url, expires_at],
                ) {
                    Ok(_) => Ok(true),
                    Err(rusqlite::Error::SqliteFailure(e, _))
                        if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                    {
                        Ok(false)
                    }
                    Err(e) => Err(tokio_rusqlite::Error::Error(e)),
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn delete(&self, code: String) -> anyhow::Result<bool> {
        let count = self
            .conn
            .call(move |conn| {
                conn.execute("DELETE FROM urls WHERE code = ?1", rusqlite::params![code])
            })
            .await?;
        Ok(count > 0)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<(String, String, Option<i64>)>> {
        self.conn
            .call(|conn| {
                let now = now_secs();
                let mut stmt = conn.prepare(
                "SELECT code, url, expires_at FROM urls WHERE expires_at IS NULL OR expires_at > ?1"
            )?;
                let rows = stmt.query_map(rusqlite::params![now], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;

                let mut result = Vec::new();
                for row in rows {
                    result.push(row?);
                }
                Ok::<_, rusqlite::Error>(result)
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn count(&self) -> anyhow::Result<u64> {
        self.conn
            .call(|conn| {
                let now = now_secs();
                conn.query_row(
                    "SELECT COUNT(*) FROM urls WHERE expires_at IS NULL OR expires_at > ?1",
                    rusqlite::params![now],
                    |row| row.get(0),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn get_url(&self, code: String) -> anyhow::Result<Option<String>> {
        self.conn
            .call(move |conn| {
                let now = now_secs();
                let url: Option<String> = conn.query_row(
                "SELECT url FROM urls WHERE code = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                rusqlite::params![code, now],
                |row| row.get(0)
            ).optional()?;
                Ok::<_, rusqlite::Error>(url)
            })
            .await
            .map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn exists(&self, code: String) -> anyhow::Result<bool> {
        self.conn.call(move |conn| {
            let now = now_secs();
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM urls WHERE code = ?1 AND (expires_at IS NULL OR expires_at > ?2))",
                rusqlite::params![code, now],
                |row| row.get(0)
            )?;
            Ok::<_, rusqlite::Error>(exists)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn cleanup_expired(&self) -> anyhow::Result<u64> {
        let count = self
            .conn
            .call(|conn| {
                let now = now_secs();
                let deleted = conn.execute(
                    "DELETE FROM urls WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                    rusqlite::params![now],
                )?;
                Ok::<_, rusqlite::Error>(deleted as u64)
            })
            .await?;
        Ok(count)
    }
}

/// Per-IP rate limit for the shorten endpoint: allow a burst of this many
/// requests, replenishing one slot every `RATE_LIMIT_REFILL_SECS` seconds.
const RATE_LIMIT_BURST: u32 = 10;
const RATE_LIMIT_REFILL_SECS: u64 = 2;

/// Layer common security response headers onto every route (including static
/// assets). `script-src 'self'` is the meaningful XSS guard for the bundled UI,
/// which loads only same-origin CSS/JS.
fn with_security_headers<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    const CSP: &str = "default-src 'self'; img-src 'self' data:; \
        style-src 'self' 'unsafe-inline'; script-src 'self'; \
        base-uri 'self'; form-action 'self'; frame-ancestors 'none'";
    router
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        ))
}

/// Router without rate limiting. Used by tests, which drive it directly via
/// `oneshot` (no connection info to key a rate limiter on).
pub fn create_router(state: Arc<AppState>) -> Router {
    with_security_headers(
        Router::new()
            .route("/api/v1/shorten", post(shorten_api))
            .route("/{code}", get(redirect_url))
            .fallback_service(ServeDir::new("static")),
    )
    .with_state(state)
}

/// Production router: same as [`create_router`] but with a per-IP rate limiter
/// on the shorten endpoint. Redirects and static assets stay unthrottled.
/// Requires the server to be run with
/// `into_make_service_with_connect_info::<SocketAddr>()` so the peer IP is
/// available to the limiter.
pub fn create_router_with_rate_limit(state: Arc<AppState>) -> Router {
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(RATE_LIMIT_REFILL_SECS)
        .burst_size(RATE_LIMIT_BURST)
        .finish()
        .expect("valid rate-limit configuration");

    with_security_headers(
        Router::new()
            .route(
                "/api/v1/shorten",
                post(shorten_api).layer(GovernorLayer::new(governor_conf)),
            )
            .route("/{code}", get(redirect_url))
            .fallback_service(ServeDir::new("static")),
    )
    .with_state(state)
}

#[derive(Deserialize, Serialize)]
pub struct ShortenRequest {
    pub url: String,
    pub custom_code: Option<String>,
    pub expires_in: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ShortenResponse {
    pub code: String,
    pub expires_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub fn parse_expires_in(expires_in: Option<&str>) -> Result<Option<i64>, String> {
    const DAY: i64 = 24 * 60 * 60;
    let duration_secs = match expires_in.unwrap_or("7d") {
        "1d" => Some(DAY),
        "7d" => Some(7 * DAY),
        "1mo" => Some(30 * DAY),
        "3mo" => Some(90 * DAY),
        "6mo" => Some(180 * DAY),
        "1y" => Some(365 * DAY),
        "never" => None,
        other => {
            return Err(format!(
                "Invalid expires_in value: '{}'. Use 1d, 7d, 1mo, 3mo, 6mo, 1y, or never",
                other
            ));
        }
    };

    Ok(duration_secs.map(|secs| now_secs() + secs))
}

/// Determine the code length to use for a given attempt number when
/// auto-generating a short code. Starts at the minimum length and bumps
/// up after a fixed number of consecutive collisions at the current length.
pub fn code_length_for_attempt(attempt: usize) -> usize {
    const MIN_LEN: usize = 3;
    const RETRIES_PER_LEN: usize = 2;
    MIN_LEN + (attempt / RETRIES_PER_LEN)
}

const MAX_GENERATE_ATTEMPTS: usize = 20;

/// Alphabet for auto-generated short codes: lowercase letters and digits.
const CODE_ALPHABET: [char; 36] = [
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
];

/// Generate a short code and insert it into the DB, retrying on collisions
/// and bumping the code length after a couple of failures at the current
/// length. Returns `Ok(Some(code))` on success, `Ok(None)` if all attempts
/// were exhausted.
pub async fn generate_and_insert(
    db: &Db,
    url: String,
    expires_at: Option<i64>,
) -> anyhow::Result<Option<String>> {
    for attempt in 0..MAX_GENERATE_ATTEMPTS {
        let len = code_length_for_attempt(attempt);
        let code = nanoid!(len, &CODE_ALPHABET);
        match db.insert(code.clone(), url.clone(), expires_at).await? {
            true => return Ok(Some(code)),
            false => continue,
        }
    }
    Ok(None)
}

/// Maximum accepted length of a URL to shorten. Guards against giant payloads
/// (e.g. multi-megabyte `data:` URLs) being persisted.
pub const MAX_URL_LEN: usize = 2048;

/// Validate a URL submitted for shortening: it must be within the length limit
/// and a well-formed `http`/`https` URL. Returns the parsed URL on success, or
/// a user-facing error message. Rejecting non-http(s) schemes blocks
/// `javascript:`/`data:`/`file:` links that would otherwise execute or fetch
/// when a visitor follows the short link.
pub fn validate_target_url(raw: &str) -> Result<Url, String> {
    if raw.len() > MAX_URL_LEN {
        return Err(format!("URL is too long (max {} characters)", MAX_URL_LEN));
    }
    let parsed = Url::parse(raw).map_err(|_| "Invalid URL format".to_string())?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(format!(
            "Only http and https URLs are supported (got '{}')",
            other
        )),
    }
}

pub async fn shorten_api(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ShortenRequest>,
) -> Response {
    let url_parsed = match validate_target_url(&payload.url) {
        Ok(u) => u,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: err })).into_response();
        }
    };

    if let Some(host_header) = headers.get("host")
        && let Ok(host_str) = host_header.to_str()
        && is_same_domain(&url_parsed, host_str)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Cannot shorten URLs from the same domain".to_string(),
            }),
        )
            .into_response();
    }

    let expires_at = match parse_expires_in(payload.expires_in.as_deref()) {
        Ok(ts) => ts,
        Err(err) => {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: err })).into_response();
        }
    };

    if let Some(custom) = payload.custom_code {
        if let Err(err) = validate_custom_code(&custom) {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: err })).into_response();
        }
        return match state
            .db
            .insert(custom.clone(), payload.url, expires_at)
            .await
        {
            Ok(true) => (
                StatusCode::CREATED,
                Json(ShortenResponse {
                    code: custom,
                    expires_at,
                }),
            )
                .into_response(),
            Ok(false) => (
                StatusCode::CONFLICT,
                Json(ErrorResponse {
                    error: "Code already in use".to_string(),
                }),
            )
                .into_response(),
            Err(e) => {
                tracing::error!("failed to insert custom code: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: "Internal server error".to_string(),
                    }),
                )
                    .into_response()
            }
        };
    }

    match generate_and_insert(&state.db, payload.url, expires_at).await {
        Ok(Some(code)) => (
            StatusCode::CREATED,
            Json(ShortenResponse { code, expires_at }),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Could not generate unique code".to_string(),
            }),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("failed to generate and insert code: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Internal server error".to_string(),
                }),
            )
                .into_response()
        }
    }
}

fn validate_custom_code(code: &str) -> Result<(), String> {
    if code.len() < 3 || code.len() > 32 {
        return Err("Custom code must be between 3 and 32 characters".to_string());
    }
    if !code
        .chars()
        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Invalid characters in code".to_string());
    }
    Ok(())
}

fn is_same_domain(url: &Url, host_header: &str) -> bool {
    let app_host = host_header.split(':').next().unwrap_or(host_header);
    if let Some(input_host) = url.host_str() {
        return input_host == app_host;
    }
    false
}

pub async fn redirect_url(
    Path(code): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    match state.db.get_url(code).await {
        Ok(Some(url)) => Redirect::temporary(&url).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        Err(e) => {
            tracing::error!("failed to look up code for redirect: {:?}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_custom_code() {
        assert!(validate_custom_code("abc").is_ok());
        assert!(validate_custom_code("my-code_123").is_ok());
        assert!(validate_custom_code("ab").is_err());
        assert!(validate_custom_code(&"a".repeat(33)).is_err());
        assert!(validate_custom_code("code!").is_err());
        assert!(validate_custom_code("code space").is_err());
    }

    #[test]
    fn test_is_same_domain() {
        let url = Url::parse("https://example.com/foo").unwrap();
        assert!(is_same_domain(&url, "example.com"));
        assert!(is_same_domain(&url, "example.com:8080"));
        assert!(!is_same_domain(&url, "google.com"));

        let local_url = Url::parse("http://localhost:8080/bar").unwrap();
        assert!(is_same_domain(&local_url, "localhost:8080"));
        assert!(is_same_domain(&local_url, "localhost"));
    }

    #[test]
    fn test_parse_expires_in() {
        const DAY: i64 = 86400;
        // Default (None) -> 7 days from now
        let result = parse_expires_in(None).unwrap();
        assert!(result.is_some());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let diff = result.unwrap() - now;
        assert!((7 * DAY - 2..=7 * DAY).contains(&diff));

        // Explicit values
        let cases = [
            ("1d", DAY),
            ("7d", 7 * DAY),
            ("1mo", 30 * DAY),
            ("3mo", 90 * DAY),
            ("6mo", 180 * DAY),
            ("1y", 365 * DAY),
        ];
        for (input, expected) in cases {
            let result = parse_expires_in(Some(input)).unwrap();
            let diff = result.unwrap() - now;
            assert!(
                (expected - 2..=expected).contains(&diff),
                "expires_in={} produced diff={}, expected ~{}",
                input,
                diff,
                expected
            );
        }

        // Never -> None
        let result = parse_expires_in(Some("never")).unwrap();
        assert!(result.is_none());

        // Removed values are now invalid
        assert!(parse_expires_in(Some("30m")).is_err());
        assert!(parse_expires_in(Some("1h")).is_err());

        // Other invalid values
        assert!(parse_expires_in(Some("5m")).is_err());
        assert!(parse_expires_in(Some("")).is_err());
    }

    #[test]
    fn test_code_length_for_attempt() {
        // First two attempts use the minimum length of 3, then bump by 1
        // after every two consecutive collisions.
        assert_eq!(code_length_for_attempt(0), 3);
        assert_eq!(code_length_for_attempt(1), 3);
        assert_eq!(code_length_for_attempt(2), 4);
        assert_eq!(code_length_for_attempt(3), 4);
        assert_eq!(code_length_for_attempt(4), 5);
        assert_eq!(code_length_for_attempt(5), 5);
        assert_eq!(code_length_for_attempt(10), 8);
    }

    #[tokio::test]
    async fn test_generate_and_insert_uses_min_length_on_first_attempt() {
        let db = Db::new(":memory:").await.unwrap();
        let code = generate_and_insert(&db, "https://example.com".to_string(), None)
            .await
            .unwrap()
            .expect("should generate a code");
        assert_eq!(
            code.len(),
            3,
            "first successful code should be at min length 3"
        );
        assert_eq!(
            db.get_url(code.clone()).await.unwrap().as_deref(),
            Some("https://example.com")
        );
    }

    #[tokio::test]
    async fn test_generate_and_insert_uses_lowercase_alphanumeric_alphabet() {
        let db = Db::new(":memory:").await.unwrap();
        for i in 0..50 {
            let code = generate_and_insert(&db, format!("https://example.com/{}", i), None)
                .await
                .unwrap()
                .expect("should generate a code");
            assert!(
                code.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "generated code '{}' must only contain a-z0-9",
                code
            );
        }
    }

    #[tokio::test]
    async fn test_generate_and_insert_succeeds_under_contention() {
        let db = Db::new(":memory:").await.unwrap();
        for _ in 0..50 {
            let _ = db
                .insert(nanoid!(3, &CODE_ALPHABET), "https://x".to_string(), None)
                .await;
        }
        let code = generate_and_insert(&db, "https://example.com".to_string(), None)
            .await
            .unwrap()
            .expect("should still generate a code");
        assert!((3..=12).contains(&code.len()));
    }

    #[test]
    fn test_validate_target_url() {
        // Accepts http/https and returns the parsed URL.
        assert!(validate_target_url("https://example.com/path").is_ok());
        assert!(validate_target_url("http://example.com").is_ok());

        // Rejects dangerous / unexpected schemes.
        assert!(validate_target_url("javascript:alert(1)").is_err());
        assert!(validate_target_url("data:text/html,<script>alert(1)</script>").is_err());
        assert!(validate_target_url("file:///etc/passwd").is_err());
        assert!(validate_target_url("ftp://example.com").is_err());

        // Rejects malformed URLs.
        assert!(validate_target_url("not-a-url").is_err());

        // Rejects over-length URLs.
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(validate_target_url(&long).is_err());
    }

    #[test]
    fn test_now_secs_is_plausible() {
        // Well past 2021-01-01 (1_600_000_000) and not the epoch fallback.
        assert!(now_secs() > 1_600_000_000);
    }

    #[tokio::test]
    async fn test_db_new_creates_expires_at_index() {
        let db = Db::new(":memory:").await.unwrap();
        let has_index = db
            .conn
            .call(|conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM pragma_index_list('urls') WHERE name = 'idx_urls_expires_at'",
                    [],
                    |row| row.get(0),
                )?;
                Ok::<_, rusqlite::Error>(count)
            })
            .await
            .unwrap();
        assert_eq!(
            has_index, 1,
            "idx_urls_expires_at should exist after Db::new"
        );
    }
}
