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
    let db = Db::open_sqlite(&config.db_path)?;
    let address = format!("{}:{}", config.host, config.port);
    let state = AppState::new(config, db)?;
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
