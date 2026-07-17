mod account;
mod auth;
mod chat;
mod config;
mod db;
mod handlers;
mod limits;
mod mail;
mod protocol;
mod relations;
mod room;
mod socket_util;
mod state;
mod util;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::get;
use axum::Router;
use socketioxide::extract::SocketRef;
use socketioxide::SocketIo;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::Config;
use crate::db::Db;
use crate::mail::Mailer;
use crate::state::{new_shared_world, SharedWorld};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub world: SharedWorld,
    pub mailer: Mailer,
    pub login_mutex: Arc<Mutex<()>>,
    /// Set after SocketIo is built (once).
    pub io: Arc<std::sync::OnceLock<SocketIo>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config = Arc::new(Config::from_env());
    info!(
        port = config.port,
        backend = ?config.db_backend,
        "Starting Bondage Club Server (Rust)"
    );

    let db = Db::connect(&config).await?;
    let next_member = db.next_member_number().await?;
    info!(next_member, "Next Member Number");

    let world = new_shared_world(next_member);
    let mailer = Mailer::new(&config);

    let io_slot = Arc::new(std::sync::OnceLock::new());

    let state = AppState {
        config: config.clone(),
        db,
        world,
        mailer,
        login_mutex: Arc::new(Mutex::new(())),
        io: io_slot.clone(),
    };

    let (layer, io) = SocketIo::builder()
        .max_payload(180_000)
        .ping_interval(Duration::from_millis(50_000))
        .ping_timeout(Duration::from_millis(30_000))
        .with_state(state.clone())
        .build_layer();

    let _ = io_slot.set(io.clone());

    {
        let st = state.clone();
        io.ns("/", move |socket: SocketRef| {
            handlers::on_connection(socket, st.clone());
        });
    }

    // Periodic ServerInfo (60s) + delayed DB flush (300s)
    {
        let io_info = io.clone();
        let st = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                handlers::broadcast_server_info(&io_info, &st);
            }
        });
    }
    {
        let st = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                account::flush_delayed_updates(&st).await;
            }
        });
    }

    let cors = if config.cors_origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<_> = config
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .layer(layer)
        .layer(cors);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    info!("Bondage Club server is listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Graceful shutdown on Ctrl+C / SIGTERM
    let io_shutdown = io.clone();
    let st_shutdown = state.clone();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            info!("Shutdown signal received");
            account::flush_delayed_updates(&st_shutdown).await;
            handlers::graceful_shutdown_message(&io_shutdown).await;
        })
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
