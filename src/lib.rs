use axum::{
    extract::{Path, State},
    http::StatusCode,
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
            Ok(())
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
                Err(e) => Err(tokio_rusqlite::Error::Rusqlite(e)),
            }
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn delete(&self, code: String) -> anyhow::Result<bool> {
        let count = self.conn.call(move |conn| {
            Ok(conn.execute("DELETE FROM urls WHERE code = ?1", rusqlite::params![code])?)
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
            Ok(result)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn count(&self) -> anyhow::Result<u64> {
        self.conn.call(|conn| {
            Ok(conn.query_row("SELECT COUNT(*) FROM urls", [], |row| row.get(0))?)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn get_url(&self, code: String) -> anyhow::Result<Option<String>> {
        self.conn.call(move |conn| {
            let url: Option<String> = conn.query_row(
                "SELECT url FROM urls WHERE code = ?1",
                rusqlite::params![code],
                |row| row.get(0)
            ).optional()?;
            Ok(url)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }

    pub async fn exists(&self, code: String) -> anyhow::Result<bool> {
        self.conn.call(move |conn| {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM urls WHERE code = ?1)",
                rusqlite::params![code],
                |row| row.get(0)
            )?;
            Ok(exists)
        }).await.map_err(|e| anyhow::anyhow!(e))
    }
}

pub fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/shorten", post(shorten_api))
        .route("/:code", get(redirect_url))
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
    Json(payload): Json<ShortenRequest>,
) -> Response {
    if Url::parse(&payload.url).is_err() {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Invalid URL format".to_string() })).into_response();
    }

    let is_custom = payload.custom_code.is_some();
    let code = if let Some(custom) = payload.custom_code {
        if custom.len() < 3 || custom.len() > 32 {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Custom code must be between 3 and 32 characters".to_string() })).into_response();
        }
        if !custom.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Invalid characters in code".to_string() })).into_response();
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
        Ok(false) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "Failed to insert".to_string() })).into_response(), // Should be caught by exists check ideally, but for auto-gen codes collision is rare
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
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
