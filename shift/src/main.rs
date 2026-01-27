use std::path::PathBuf;

use crate::server_layer::ShiftServer;

mod client_layer;
mod server_layer;
mod comms;
mod ids;
mod sessions;
mod auth;
mod monitor;
#[tokio::main]
async fn main() {
    // ---- logging/tracing ----
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "trace".to_string()),
        )
        .with_target(false)
        .init();

    // ---- socket path ----
    let socket_path = std::env::var_os("SHIFT_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| "/tmp/shift.sock".into());

    // ---- create server ----
    let mut server = match ShiftServer::bind(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("failed to bind ShiftServer at {:?}: {e}", socket_path);
            return;
        }
    };
    server.add_initial_session();
    tracing::info!("starting ShiftServer on {:?}", socket_path);
    server.start().await;

}