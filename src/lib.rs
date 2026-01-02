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
                    url TEXT NOT NULL
                )",
                [],
            )?;
            Ok::<_, rusqlite::Error>(())
        }).await?;

        Ok(Self { conn })
    }

    pub async fn insert(&self, code: String, url: String) -> anyhow::Result<bool> {
        self.conn.call(move |conn| {
            match conn.execute(
                "INSERT INTO urls (code, url) VALUES (?1, ?2)",
                rusqlite::params![code, url],
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

    pub async fn list(&self) -> anyhow::Result<Vec<(String, String)>> {
        self.conn.call(|conn| {
            let mut stmt = conn.prepare("SELECT code, url FROM urls")?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?))
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
            Ok::<_, rusqlite::Error>(conn.query_row("SELECT COUNT(*) FROM urls", [], |row| row.get(0))?)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn get_url(&self, code: String) -> anyhow::Result<Option<String>> {
        self.conn.call(move |conn| {
            let url: Option<String> = conn.query_row(
                "SELECT url FROM urls WHERE code = ?1",
                rusqlite::params![code],
                |row| row.get(0)
            ).optional()?;
            Ok::<_, rusqlite::Error>(url)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn exists(&self, code: String) -> anyhow::Result<bool> {
        self.conn.call(move |conn| {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM urls WHERE code = ?1)",
                rusqlite::params![code],
                |row| row.get(0)
            )?;
            Ok::<_, rusqlite::Error>(exists)
        }).await.map_err(|e| anyhow::anyhow!(e))
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
}

#[derive(Serialize, Deserialize)]
pub struct ShortenResponse {
    pub code: String,
}

#[derive(Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
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

    let is_custom = payload.custom_code.is_some();
    let code = if let Some(custom) = payload.custom_code {
        if let Err(err) = validate_custom_code(&custom) {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: err })).into_response();
        }
        custom
    } else {
        nanoid!(7)
    };

    if is_custom {
        match state.db.exists(code.clone()).await {
            Ok(true) => return (StatusCode::CONFLICT, Json(ErrorResponse { error: "Code already in use".to_string() })).into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
            _ => {}
        }
    }

    match state.db.insert(code.clone(), payload.url).await {
        Ok(true) => (StatusCode::CREATED, Json(ShortenResponse { code })).into_response(),
        Ok(false) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Failed to insert".to_string() })).into_response(),
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
}