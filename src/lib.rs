use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use nanoid::nanoid;
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_rusqlite::Connection;
use tower_http::services::ServeDir;
use url::Url;

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

            Ok::<_, rusqlite::Error>(())
        }).await?;

        Ok(Self { conn })
    }

    pub async fn insert(&self, code: String, url: String, expires_at: Option<i64>) -> anyhow::Result<bool> {
        self.conn.call(move |conn| {
            match conn.execute(
                "INSERT INTO urls (code, url, expires_at) VALUES (?1, ?2, ?3)",
                rusqlite::params![code, url, expires_at],
            ) {
                Ok(_) => Ok(true),
                Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => Ok(false),
                Err(e) => Err(tokio_rusqlite::Error::Error(e)),
            }
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn delete(&self, code: String) -> anyhow::Result<bool> {
        let count = self.conn.call(move |conn| {
            Ok::<_, rusqlite::Error>(conn.execute("DELETE FROM urls WHERE code = ?1", rusqlite::params![code])?)
        }).await?;
        Ok(count > 0)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<(String, String, Option<i64>)>> {
        self.conn.call(|conn| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
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
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn count(&self) -> anyhow::Result<u64> {
        self.conn.call(|conn| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
            Ok::<_, rusqlite::Error>(conn.query_row(
                "SELECT COUNT(*) FROM urls WHERE expires_at IS NULL OR expires_at > ?1",
                rusqlite::params![now],
                |row| row.get(0),
            )?)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn get_url(&self, code: String) -> anyhow::Result<Option<String>> {
        self.conn.call(move |conn| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
            let url: Option<String> = conn.query_row(
                "SELECT url FROM urls WHERE code = ?1 AND (expires_at IS NULL OR expires_at > ?2)",
                rusqlite::params![code, now],
                |row| row.get(0)
            ).optional()?;
            Ok::<_, rusqlite::Error>(url)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn exists(&self, code: String) -> anyhow::Result<bool> {
        self.conn.call(move |conn| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM urls WHERE code = ?1 AND (expires_at IS NULL OR expires_at > ?2))",
                rusqlite::params![code, now],
                |row| row.get(0)
            )?;
            Ok::<_, rusqlite::Error>(exists)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn cleanup_expired(&self) -> anyhow::Result<u64> {
        let count = self.conn.call(|conn| {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
            let deleted = conn.execute(
                "DELETE FROM urls WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                rusqlite::params![now],
            )?;
            Ok::<_, rusqlite::Error>(deleted as u64)
        }).await?;
        Ok(count)
    }
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/shorten", post(shorten_api))
        .route("/{code}", get(redirect_url))
        .fallback_service(ServeDir::new("static"))
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
        other => return Err(format!("Invalid expires_in value: '{}'. Use 1d, 7d, 1mo, 3mo, 6mo, 1y, or never", other)),
    };

    Ok(duration_secs.map(|secs| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + secs
    }))
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
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm',
    'n', 'o', 'p', 'q', 'r', 's', 't', 'u', 'v', 'w', 'x', 'y', 'z',
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9',
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

pub async fn shorten_api(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(payload): Json<ShortenRequest>,
) -> Response {
    let url_parsed = match Url::parse(&payload.url) {
        Ok(u) => u,
        Err(_) => return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Invalid URL format".to_string() })).into_response(),
    };

    if let Some(host_header) = headers.get("host") {
        if let Ok(host_str) = host_header.to_str() {
            if is_same_domain(&url_parsed, host_str) {
                return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Cannot shorten URLs from the same domain".to_string() })).into_response();
            }
        }
    }

    let expires_at = match parse_expires_in(payload.expires_in.as_deref()) {
        Ok(ts) => ts,
        Err(err) => return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: err })).into_response(),
    };

    if let Some(custom) = payload.custom_code {
        if let Err(err) = validate_custom_code(&custom) {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: err })).into_response();
        }
        return match state.db.insert(custom.clone(), payload.url, expires_at).await {
            Ok(true) => (StatusCode::CREATED, Json(ShortenResponse { code: custom, expires_at })).into_response(),
            Ok(false) => (StatusCode::CONFLICT, Json(ErrorResponse { error: "Code already in use".to_string() })).into_response(),
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }

    match generate_and_insert(&state.db, payload.url, expires_at).await {
        Ok(Some(code)) => (StatusCode::CREATED, Json(ShortenResponse { code, expires_at })).into_response(),
        Ok(None) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Could not generate unique code".to_string() })).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn validate_custom_code(code: &str) -> Result<(), String> {
    if code.len() < 3 || code.len() > 32 {
        return Err("Custom code must be between 3 and 32 characters".to_string());
    }
    if !code.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
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
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64;
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
                "expires_in={} produced diff={}, expected ~{}", input, diff, expected
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
        assert_eq!(code.len(), 3, "first successful code should be at min length 3");
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
                code.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "generated code '{}' must only contain a-z0-9", code
            );
        }
    }

    #[tokio::test]
    async fn test_generate_and_insert_succeeds_under_contention() {
        let db = Db::new(":memory:").await.unwrap();
        for _ in 0..50 {
            let _ = db.insert(nanoid!(3, &CODE_ALPHABET), "https://x".to_string(), None).await;
        }
        let code = generate_and_insert(&db, "https://example.com".to_string(), None)
            .await
            .unwrap()
            .expect("should still generate a code");
        assert!((3..=12).contains(&code.len()));
    }
}