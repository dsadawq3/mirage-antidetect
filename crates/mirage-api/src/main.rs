//! Binary entry point: resolve the configuration from the environment, build
//! the shared state and serve the API on `127.0.0.1:<MIRAGE_API_PORT>`.

use std::net::SocketAddr;

use mirage_api::{router, AppState, Config};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    let config = Config::from_env();
    let port = config.port;
    let data_dir = config.data_dir.clone();

    let state = AppState::new(config);
    let unlocked = state.is_unlocked();
    let app = router(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("mirage-api: failed to bind {addr}: {error}");
            std::process::exit(1);
        }
    };

    println!("mirage-api: data dir {}", data_dir.display());
    println!(
        "mirage-api: vault {}",
        if unlocked { "unlocked" } else { "locked" }
    );
    println!("mirage-api: listening on http://127.0.0.1:{port}");

    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("mirage-api: server error: {error}");
        std::process::exit(1);
    }
}
