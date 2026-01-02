use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use clap::{Parser, Subcommand};
use nanoid::nanoid;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_rusqlite::Connection;
use tower_http::services::ServeDir;
use url::Url;

const DB_PATH: &str = "data/ekurl.db";

#[derive(Parser)]
#[command(name = "ekurl")]
#[command(about = "A high-performance URL shortener", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the web server (default)
    Serve,
    /// Add a new URL
    Add {
        /// The long URL to shorten
        url: String,
        /// Optional custom code
        #[arg(long)]
        code: Option<String>,
    },
    /// Remove a short link by its code
    Remove {
        /// The short code to remove
        code: String,
    },
    /// List all short codes and their URLs
    List,
    /// Count the total number of shortened URLs
    Count,
}

struct AppState {
    db: Connection,
}

#[derive(Deserialize, Serialize)]
struct ShortenRequest {
    url: String,
    custom_code: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ShortenResponse {
    code: String,
}

#[derive(Serialize, Deserialize)]
struct ErrorResponse {
    error: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve) | None => start_server().await?,
        Some(Commands::Add { url, code }) => handle_add(url, code).await?,
        Some(Commands::Remove { code }) => handle_remove(code).await?,
        Some(Commands::List) => handle_list().await?,
        Some(Commands::Count) => handle_count().await?,
    }

    Ok(())
}

async fn open_db() -> anyhow::Result<Connection> {
    std::fs::create_dir_all("data")?;
    let conn = Connection::open(DB_PATH).await?;
    
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

    Ok(conn)
}

async fn start_server() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let db = open_db().await?;
    let shared_state = Arc::new(AppState { db });

    let app = Router::new()
        .route("/api/v1/shorten", post(shorten_api))
        .route("/:code", get(redirect_url))
        .fallback_service(ServeDir::new("static"))
        .with_state(shared_state);

    let addr = "0.0.0.0:8080";
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

// --- CLI Handlers (Client -> Server) ---

async fn handle_add(url: String, custom_code: Option<String>) -> anyhow::Result<()> {
    if Url::parse(&url).is_err() {
        eprintln!("Error: Invalid URL format");
        std::process::exit(1);
    }
    let code = custom_code.unwrap_or_else(|| nanoid!(7));
    let db = open_db().await?;
    
    let url_clone = url.clone();
    let code_clone = code.clone();

    let result = db.call(move |conn| {
        match conn.execute(
            "INSERT INTO urls (code, url) VALUES (?1, ?2)",
            rusqlite::params![code_clone, url_clone],
        ) {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => Ok(false),
            Err(e) => Err(tokio_rusqlite::Error::Rusqlite(e)),
        }
    }).await?;

    if result {
        println!("Success: {} -> {}", code, url);
    } else {
        eprintln!("Error: Code '{}' updated (was already present)", code);
    }
    Ok(())
}

async fn handle_remove(code: String) -> anyhow::Result<()> {
    let db = open_db().await?;
    let code_clone = code.clone();
    let count = db.call(move |conn| {
        Ok(conn.execute("DELETE FROM urls WHERE code = ?1", rusqlite::params![code_clone])?)
    }).await?;

    if count > 0 {
        println!("Removed: {}", code);
    } else {
        eprintln!("Error: Code '{}' not found", code);
        std::process::exit(1);
    }
    Ok(())
}

async fn handle_list() -> anyhow::Result<()> {
    let db = open_db().await?;
    let items = db.call(|conn| {
        let mut stmt = conn.prepare("SELECT code, url FROM urls")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }).await?;

    println!("{:<20} | {}", "Code", "URL");
    println!("{:-<20}-|-{}", "", "");
    for (code, url) in items {
        println!("{:<20} | {}", code, url);
    }
    Ok(())
}

async fn handle_count() -> anyhow::Result<()> {
    let db = open_db().await?;
    let count: u64 = db.call(|conn| {
        Ok(conn.query_row("SELECT COUNT(*) FROM urls", [], |row| row.get(0))?)
    }).await?;
    
    println!("Total URLs: {}", count);
    Ok(())
}

// --- API Handlers ---

async fn shorten_api(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<ShortenRequest>,
) -> Response {
    if Url::parse(&payload.url).is_err() {
        return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Invalid URL format".to_string() })).into_response();
    }

    let is_custom = payload.custom_code.is_some();
    let code = if let Some(custom) = &payload.custom_code {
        if custom.len() < 3 || custom.len() > 32 {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Custom code must be between 3 and 32 characters".to_string() })).into_response();
        }
        if !custom.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            return (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: "Invalid characters in code".to_string() })).into_response();
        }
        custom.clone()
    } else {
        nanoid!(7)
    };

    let code_clone = code.clone();
    let url_clone = payload.url.clone();

    let result = state.db.call(move |conn| {
        if is_custom {
            // Check existence first if custom code
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM urls WHERE code = ?1)",
                rusqlite::params![code_clone],
                |row| row.get(0)
            )?;
            if exists {
                return Ok(Err("Code already in use"));
            }
        }
        
        match conn.execute(
            "INSERT INTO urls (code, url) VALUES (?1, ?2)",
            rusqlite::params![code_clone, url_clone],
        ) {
            Ok(_) => Ok(Ok(())),
            Err(e) => Err(tokio_rusqlite::Error::Rusqlite(e)),
        }
    }).await;

    match result {
        Ok(Ok(_)) => (StatusCode::CREATED, Json(ShortenResponse { code })).into_response(),
        Ok(Err(msg)) => (StatusCode::CONFLICT, Json(ErrorResponse { error: msg.to_string() })).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn redirect_url(
    Path(code): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Response {
    let result = state.db.call(move |conn| {
        let url: Option<String> = conn.query_row(
            "SELECT url FROM urls WHERE code = ?1",
            rusqlite::params![code],
            |row| row.get(0)
        ).optional()?;
        Ok(url)
    }).await;

    match result {
        Ok(Some(url)) => Redirect::temporary(&url).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Not Found").into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// Add import for Optional trait
use rusqlite::OptionalExtension;