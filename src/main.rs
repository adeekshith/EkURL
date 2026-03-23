use clap::{Parser, Subcommand};
use ekurl::{create_router, parse_expires_in, AppState, Db};
use nanoid::nanoid;
use std::env;
use std::sync::Arc;
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
        /// Expiry duration: 30m, 1h, 1d, 7d, or never (default: 1d)
        #[arg(long, default_value = "1d")]
        expires_in: String,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve) | None => start_server().await?,
        Some(Commands::Add { url, code, expires_in }) => handle_add(url, code, expires_in).await?,
        Some(Commands::Remove { code }) => handle_remove(code).await?,
        Some(Commands::List) => handle_list().await?,
        Some(Commands::Count) => handle_count().await?,
    }

    Ok(())
}

async fn start_server() -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    tracing_subscriber::fmt::init();
    let db = Db::new(DB_PATH).await?;

    // Clean up expired URLs on startup
    let cleaned = db.cleanup_expired().await?;
    if cleaned > 0 {
        tracing::info!("cleaned up {} expired URLs", cleaned);
    }

    let shared_state = Arc::new(AppState { db });

    let app = create_router(shared_state);

    let port = env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8080);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}

// --- CLI Handlers ---

async fn handle_add(url: String, custom_code: Option<String>, expires_in: String) -> anyhow::Result<()> {
    if Url::parse(&url).is_err() {
        eprintln!("Error: Invalid URL format");
        std::process::exit(1);
    }

    let expires_at = match parse_expires_in(Some(&expires_in)) {
        Ok(ts) => ts,
        Err(err) => {
            eprintln!("Error: {}", err);
            std::process::exit(1);
        }
    };

    let code = custom_code.unwrap_or_else(|| nanoid!(7));

    std::fs::create_dir_all("data")?;
    let db = Db::new(DB_PATH).await?;

    match db.insert(code.clone(), url.clone(), expires_at).await {
        Ok(true) => {
            let expiry_msg = match expires_at {
                Some(ts) => format!(" (expires: {})", ts),
                None => " (never expires)".to_string(),
            };
            println!("Success: {} -> {}{}", code, url, expiry_msg);
        }
        Ok(false) => {
             eprintln!("Error: Code '{}' updated (was already present)", code);
        }
        Err(e) => {
             eprintln!("Error: {}", e);
             std::process::exit(1);
        }
    }
    Ok(())
}

async fn handle_remove(code: String) -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let db = Db::new(DB_PATH).await?;
    
    match db.delete(code.clone()).await {
        Ok(true) => println!("Removed: {}", code),
        Ok(false) => {
            eprintln!("Error: Code '{}' not found", code);
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn handle_list() -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let db = Db::new(DB_PATH).await?;
    let items = db.list().await?;

    println!("{:<20} | {:<40} | {}", "Code", "URL", "Expires");
    println!("{:-<20}-|-{:-<40}-|-{}", "", "", "");
    for (code, url, expires_at) in items {
        let expiry = match expires_at {
            Some(ts) => format!("{}", ts),
            None => "never".to_string(),
        };
        println!("{:<20} | {:<40} | {}", code, url, expiry);
    }
    Ok(())
}

async fn handle_count() -> anyhow::Result<()> {
    std::fs::create_dir_all("data")?;
    let db = Db::new(DB_PATH).await?;
    let count = db.count().await?;
    
    println!("Total URLs: {}", count);
    Ok(())
}
