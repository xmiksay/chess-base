//! chess-base CLI entry point. Parses flags into an [`AppConfig`] and serves.

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::EnvFilter;

use chess_base::db::DbConfig;
use chess_base::engine::EngineConfig;
use chess_base::server::{self, AppConfig, Mode};

#[derive(Parser, Debug)]
#[command(name = "chess-base", version, about)]
struct Cli {
    /// Run in multi-user server mode against Postgres (requires --database-url).
    #[arg(long)]
    server: bool,

    /// Postgres connection URL (server mode). May also be set via DATABASE_URL.
    #[arg(long, env = "DATABASE_URL")]
    database_url: Option<String>,

    /// SQLite file for local mode.
    #[arg(long, default_value = "chess-base.db")]
    db_path: String,

    /// Address to bind.
    #[arg(long, default_value = "127.0.0.1")]
    host: std::net::IpAddr,

    /// Port to bind (0 = pick a free port).
    #[arg(long, short, default_value_t = 0)]
    port: u16,

    /// Do not auto-open the browser in local mode.
    #[arg(long)]
    no_open: bool,

    /// Path to a UCI engine binary (e.g. Stockfish) to enable the analysis
    /// WebSocket. May also be set via CHESS_BASE_ENGINE.
    #[arg(long, env = "CHESS_BASE_ENGINE")]
    engine: Option<std::path::PathBuf>,

    /// Optional neural-net weights file for the engine (Lc0/Maia `WeightsFile`).
    #[arg(long, requires = "engine", env = "CHESS_BASE_ENGINE_WEIGHTS")]
    engine_weights: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    let (mode, db) = if cli.server {
        let url = cli
            .database_url
            .context("server mode requires --database-url (or DATABASE_URL)")?;
        (Mode::Server, DbConfig::postgres(url))
    } else {
        let db = match cli.database_url {
            Some(url) => DbConfig::postgres(url),
            None => DbConfig::sqlite(cli.db_path),
        };
        (Mode::Local, db)
    };

    let engine = cli.engine.map(|path| {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "engine".to_string());
        let cfg = EngineConfig::new(name, path);
        match cli.engine_weights {
            Some(weights) => cfg.with_weights(weights),
            None => cfg,
        }
    });

    let cfg = AppConfig {
        mode,
        host: cli.host,
        port: cli.port,
        open_browser: mode == Mode::Local && !cli.no_open,
        db,
        engine,
    };

    server::serve(cfg).await
}
