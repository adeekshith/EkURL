use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use nanoid::nanoid;
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::services::ServeDir;
use url::Url;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("urls");

struct AppState {
    db: Database,
}

#[derive(Deserialize)]
struct ShortenRequest {
    url: String,
    custom_code: Option<String>,
}

#[derive(Serialize)]
struct ShortenResponse {
    code: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let db_path = "data/ekurl.redb";
    std::fs::create_dir_all("data")?;
    
    let db = Database::builder()
        .create(db_path)?;

    // Initialize table
    let write_txn = db.begin_write()?;
    {
        let _ = write_txn.open_table(TABLE)?;
    }
    write_txn.commit()?;

    let shared_state = Arc::new(AppState { db });

    let app = Router::new()
        .route("/api/v1/shorten", post(shorten_url))
        .route("/:code", get(redirect_url))
        .fallback_service(ServeDir::new("static"))
        .with_state(shared_state);

    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

async fn shorten_url(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ShortenRequest>,
) -> Response {
    // Validate URL
    if let Err(_) = Url::parse(&payload.url) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid URL format".to_string(),
            }),
        )
            .into_response();
    }

    let is_custom = payload.custom_code.is_some();
    let code = if let Some(custom) = &payload.custom_code {
        if custom.len() < 3 || custom.len() > 32 {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Custom code must be between 3 and 32 characters".to_string(),
                }),
            )
                .into_response();
        }
        if !custom.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Custom code can only contain alphanumeric characters, hyphens, and underscores"
                        .to_string(),
                }),
            )
                .into_response();
        }
        custom.clone()
    } else {
        nanoid!(7)
    };

    let write_txn = match state.db.begin_write() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    {
        let mut table = match write_txn.open_table(TABLE) {
            Ok(t) => t,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };

        // Check if code already exists
        if table.get(code.as_str()).unwrap().is_some() {
            if is_custom {
                return (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse {
                        error: "Custom code already in use".to_string(),
                    }),
                )
                    .into_response();
            }
            // If random code collided (unlikely), we could retry, but for simplicity:
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }

        match table.insert(code.as_str(), payload.url.as_str()) {
            Ok(_) => {},
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };
    }

    if let Err(_) = write_txn.commit() {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    (StatusCode::CREATED, Json(ShortenResponse { code })).into_response()
}

async fn redirect_url(
    Path(code): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let read_txn = match state.db.begin_read() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let table = match read_txn.open_table(TABLE) {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    match table.get(code.as_str()).unwrap() {
        Some(url) => Redirect::temporary(url.value()).into_response(),
        None => (StatusCode::NOT_FOUND, "Not Found").into_response(),
    }
}
