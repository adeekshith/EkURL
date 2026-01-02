use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use clap::{Parser, Subcommand};
use nanoid::nanoid;
use redb::{Database, ReadableTable, TableDefinition, ReadableTableMetadata};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::services::ServeDir;
use url::Url;

const TABLE: TableDefinition<&str, &str> = TableDefinition::new("urls");
const DB_PATH: &str = "data/ekurl.redb";

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
    db: Database,
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

fn open_db() -> anyhow::Result<Database> {
    std::fs::create_dir_all("data")?;
    let db = Database::builder().create(DB_PATH)?;
    // Initialize table
    let write_txn = db.begin_write()?;
    {
        let _ = write_txn.open_table(TABLE)?;
    }
    write_txn.commit()?;
    Ok(db)
}

async fn start_server() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let db = open_db()?;
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
    let db = open_db()?;
    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(TABLE)?;
        if table.insert(code.as_str(), url.as_str())?.is_some() {
             eprintln!("Error: Code '{}' updated (was already present)", code);
        }
    }
    write_txn.commit()?;
    println!("Success: {} -> {}", code, url);
    Ok(())
}

async fn handle_remove(code: String) -> anyhow::Result<()> {
    let db = open_db()?;
    let write_txn = db.begin_write()?;
    let existed = {
        let mut table = write_txn.open_table(TABLE)?;
        let res = table.remove(code.as_str())?.is_some();
        res
    };
    write_txn.commit()?;
    if existed {
        println!("Removed: {}", code);
    } else {
        eprintln!("Error: Code '{}' not found", code);
        std::process::exit(1);
    }
    Ok(())
}

async fn handle_list() -> anyhow::Result<()> {
    let db = open_db()?;
    let read_txn = db.begin_read()?;
    let table = read_txn.open_table(TABLE)?;
    println!("{:<20} | {}", "Code", "URL");
    println!("{:-<20}-|-{}", "", "");
    for item in table.iter()? {
        let (code, url) = item?;
        println!("{:<20} | {}", code.value(), url.value());
    }
    Ok(())
}

async fn handle_count() -> anyhow::Result<()> {
    let db = open_db()?;
    let read_txn = db.begin_read()?;
    let table = read_txn.open_table(TABLE)?;
    println!("Total URLs: {}", table.len()?);
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

    let write_txn = match state.db.begin_write() {
        Ok(t) => t,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    {
        let mut table = match write_txn.open_table(TABLE) {
            Ok(t) => t,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };

        if table.get(code.as_str()).unwrap().is_some() {
            if is_custom {
                return (StatusCode::CONFLICT, Json(ErrorResponse { error: "Code already in use".to_string() })).into_response();
            }
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