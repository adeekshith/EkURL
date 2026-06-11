use clap::{Parser, Subcommand};
use ekurl::{
    AppState, Db, create_router_with_rate_limit, generate_and_insert, parse_expires_in,
    validate_target_url,
};
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

const DB_PATH: &str = "data/ekurl.db";

/// How often the background task purges expired URLs from the database.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(3600);

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
        /// Expiry duration: 1d, 7d, 1mo, 3mo, 6mo, 1y, or never (default: 7d)
        #[arg(long, default_value = "7d")]
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
        Some(Commands::Add {
            url,
            code,
            expires_in,
        }) => handle_add(url, code, expires_in).await?,
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

    // Keep purging expired URLs while the server runs, so a long-lived process
    // doesn't accumulate dead rows between restarts.
    let cleanup_db = db.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(CLEANUP_INTERVAL);
        ticker.tick().await; // consume the immediate first tick (just ran above)
        loop {
            ticker.tick().await;
            match cleanup_db.cleanup_expired().await {
                Ok(n) if n > 0 => tracing::info!("cleaned up {} expired URLs", n),
                Ok(_) => {}
                Err(e) => tracing::error!("periodic cleanup failed: {:?}", e),
            }
        }
    });

    let shared_state = Arc::new(AppState { db });

    let app = create_router_with_rate_limit(shared_state);

    let port = env::var("PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8080);
    let addr = format!("0.0.0.0:{}", port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("listening on {}", addr);
    // `into_make_service_with_connect_info` exposes the peer IP to the per-IP
    // rate limiter on the shorten endpoint.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

// --- CLI Handlers ---

async fn handle_add(
    url: String,
    custom_code: Option<String>,
    expires_in: String,
) -> anyhow::Result<()> {
    if let Err(err) = validate_target_url(&url) {
        eprintln!("Error: {}", err);
        std::process::exit(1);
    }

    let expires_at = match parse_expires_in(Some(&expires_in)) {
        Ok(ts) => ts,
        Err(err) => {
            eprintln!("Error: {}", err);
            std::process::exit(1);
        }
    };

    std::fs::create_dir_all("data")?;
    let db = Db::new(DB_PATH).await?;

    let result = if let Some(code) = custom_code {
        db.insert(code.clone(), url.clone(), expires_at)
            .await
            .map(|ok| ok.then_some(code))
    } else {
        generate_and_insert(&db, url.clone(), expires_at).await
    };

    match result {
        Ok(Some(code)) => {
            let expiry_msg = match expires_at {
                Some(ts) => format!(" (expires: {})", ts),
                None => " (never expires)".to_string(),
            };
            println!("Success: {} -> {}{}", code, url, expiry_msg);
        }
        Ok(None) => {
            eprintln!("Error: Code already in use or could not generate a unique code");
            std::process::exit(1);
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

    println!("{:<20} | {:<40} | Expires", "Code", "URL");
    println!("{:-<20}-|-{:-<40}-|-", "", "");
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
