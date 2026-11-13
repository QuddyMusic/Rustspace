//! Точка входа сервера Backspace.

use std::net::SocketAddr;

use bs_server::config::Config;
use bs_server::db::Db;
use bs_server::state::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let config = Config::from_env()?;
    let db = match &config.database_url {
        Some(url) => {
            tracing::info!("хранилище: PostgreSQL");
            Db::open_postgres(url)?
        }
        None => Db::open_sqlite(&config.db_path)?,
    };
    // `backspace-server import-sqlite <файл>`: перенос данных в PostgreSQL.
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some("import-sqlite") {
        let Some(path) = arguments.next() else {
            return Err("укажите файл базы SQLite: backspace-server import-sqlite <файл>".into());
        };
        let rows = db.import_sqlite(std::path::Path::new(&path))?;
        tracing::info!(rows, "данные перенесены из SQLite в PostgreSQL");
        return Ok(());
    }
    let address = format!("{}:{}", config.host, config.port);
    let state = AppState::new(config, db)?;
    let peers = state
        .run(|tx, _| {
            Ok(tx
                .query_one(
                    "SELECT count(*) AS n FROM federation_peers WHERE status IN ('active', 'unreachable')",
                    &[],
                )?
                .map_or(Ok(0), |row| row.int("n"))?)
        })
        .await
        .unwrap_or(0);
    if peers > 0 {
        tracing::warn!(
            peers,
            "у инстанса есть пиры федерации; пиринг ведётся, но доставка и приём событий между \
             инстансами в этом сервере ещё не перенесены: события пирам не отправляются и не принимаются"
        );
    }
    bs_server::spawn_background(&state);
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!("Backspace server running at http://{address}");
    axum::serve(
        listener,
        bs_server::app(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Shutting down...");
    })
    .await?;
    Ok(())
}
