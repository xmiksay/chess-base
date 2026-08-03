//! chess-base CLI entry point. Parses flags into an [`AppConfig`] and serves.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use chess_base::collectors::bulk::{find_or_create_master, BulkImporter};
use chess_base::db::{self, DbConfig};
use chess_base::engine::EngineConfig;
use chess_base::server::identity::CurrentUser;
use chess_base::server::{self, AppConfig, Mode};
use chess_base::service_tokens::ServiceTokenService;

#[derive(Parser, Debug)]
#[command(name = "chess-base", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

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

    /// Directory the engine auto-download manager installs into.
    #[arg(long, default_value = "engines", env = "CHESS_BASE_ENGINES_DIR")]
    engines_dir: std::path::PathBuf,

    /// Disable first-run auto-download of Stockfish + Maia.
    #[arg(long)]
    no_engine_download: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Bulk-import a (optionally `.zst`-compressed) PGN file into a global master
    /// database, then exit without serving. Streams in bounded memory, dedups by
    /// content hash and is restartable (issue #4).
    ImportPgn {
        /// Path to the `.pgn` or `.pgn.zst` file to import.
        path: PathBuf,

        /// SQLite file to import into (local mode).
        #[arg(long, default_value = "chess-base.db")]
        db_path: String,

        /// Postgres connection URL; overrides --db-path when set.
        #[arg(long, env = "DATABASE_URL")]
        database_url: Option<String>,

        /// Name of the global master database to create or append to.
        #[arg(long, default_value = "Master Database")]
        name: String,

        /// Games committed per transaction.
        #[arg(long, default_value_t = 1000)]
        batch_size: usize,
    },

    /// Mint/list/revoke server-mode service tokens (ADR-0044, issue #193) —
    /// the only way to create a scoped token headlessly, since the HTTP route
    /// requires an already-admin session.
    ServiceToken {
        #[command(subcommand)]
        action: ServiceTokenAction,

        /// SQLite file to operate on (local mode).
        #[arg(long, default_value = "chess-base.db")]
        db_path: String,

        /// Postgres connection URL; overrides --db-path when set.
        #[arg(long, env = "DATABASE_URL")]
        database_url: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum ServiceTokenAction {
    /// Mint a fresh scoped token.
    Create {
        /// User id (or `local-admin`) this token acts as.
        #[arg(long)]
        owner_id: String,
        /// Human label shown when listing tokens.
        #[arg(long)]
        label: String,
        /// `full` | `read_only` | `global_read`.
        #[arg(long, default_value = "full")]
        scope: String,
        /// Grant admin (global-resource) rights.
        #[arg(long)]
        admin: bool,
        /// Optional hard expiry, in days from now.
        #[arg(long)]
        expires_in_days: Option<i64>,
    },
    /// List every service token (never shows the raw secret).
    List,
    /// Revoke a token by its non-secret `id`.
    Revoke { id: String },
}

/// Run the bulk PGN import subcommand and report its tally.
async fn run_import(
    path: PathBuf,
    db_path: String,
    database_url: Option<String>,
    name: String,
    batch_size: usize,
) -> Result<()> {
    let cfg = match database_url {
        Some(url) => DbConfig::postgres(url),
        None => DbConfig::sqlite(db_path),
    };
    let conn = db::connect(&cfg).await.context("connecting to database")?;
    let database_id = find_or_create_master(&conn, &name).await?;

    let report = BulkImporter::new()
        .with_batch_size(batch_size)
        .import_path(&conn, database_id, &path)
        .await?;

    println!(
        "imported {} games ({} duplicates skipped, {} errors) into '{name}'",
        report.imported, report.duplicates, report.errors
    );
    Ok(())
}

/// Run the `service-token` subcommand: mint/list/revoke over `service_tokens`.
/// The CLI operator already has direct DB/host access, so `CurrentUser::local_admin()`
/// is used purely to satisfy `ServiceTokenService`'s `assert_admin` gate — the
/// same reasoning `ImportPgn` uses to bypass HTTP auth entirely.
async fn run_service_token(
    action: ServiceTokenAction,
    db_path: String,
    database_url: Option<String>,
) -> Result<()> {
    let cfg = match database_url {
        Some(url) => DbConfig::postgres(url),
        None => DbConfig::sqlite(db_path),
    };
    let conn = db::connect(&cfg).await.context("connecting to database")?;
    let svc = ServiceTokenService::new(conn);
    let admin = CurrentUser::local_admin();

    match action {
        ServiceTokenAction::Create {
            owner_id,
            label,
            scope,
            admin: is_admin,
            expires_in_days,
        } => {
            let minted = svc
                .create(&admin, &owner_id, &label, &scope, is_admin, expires_in_days)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("Minted service token (shown once — store it now):");
            println!("  token: {}", minted.token);
            println!("  id:    {}", minted.view.id);
            println!("  owner: {}", minted.view.owner_id);
            println!("  scope: {}", minted.view.scope);
        }
        ServiceTokenAction::List => {
            let tokens = svc.list(&admin).await.map_err(|e| anyhow::anyhow!("{e}"))?;
            if tokens.is_empty() {
                println!("no service tokens");
            }
            for t in tokens {
                println!(
                    "{}  owner={}  scope={}  admin={}  label={:?}",
                    t.id, t.owner_id, t.scope, t.is_admin, t.label
                );
            }
        }
        ServiceTokenAction::Revoke { id } => {
            svc.revoke(&admin, &id)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("revoked service token {id}");
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    if let Some(Command::ImportPgn {
        path,
        db_path,
        database_url,
        name,
        batch_size,
    }) = cli.command
    {
        return run_import(path, db_path, database_url, name, batch_size).await;
    }

    if let Some(Command::ServiceToken {
        action,
        db_path,
        database_url,
    }) = cli.command
    {
        return run_service_token(action, db_path, database_url).await;
    }

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
        engines_dir: cli.engines_dir,
        download_engines: !cli.no_engine_download,
    };

    server::serve(cfg).await
}
