use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::PathBuf;
use std::time::Duration;

use tracing_subscriber::EnvFilter;

use cyclone_dds_ws_bridge::bridge;
use cyclone_dds_ws_bridge::config::Config;
use cyclone_dds_ws_bridge::ws;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.get(1).map(|s| s.as_str()) == Some("healthcheck") {
        std::process::exit(run_healthcheck(&args));
    }

    // Load configuration
    let config = match load_config_from_args(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(1);
        }
    };

    // Initialize tracing
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&config.logging.level));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // Create bridge
    let bridge_handle = match bridge::create_bridge(config.clone()) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e, "failed to create DDS bridge");
            std::process::exit(1);
        }
    };

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    // Spawn DDS reader polling loop
    let poll_bridge = bridge_handle.clone();
    let poll_handle = tokio::spawn(async move {
        bridge::reader_poll_loop(poll_bridge, Duration::from_millis(10)).await;
    });

    // Spawn WS server
    let ws_bridge = bridge_handle.clone();
    let ws_addr = config.websocket.addr.clone();
    let ws_port = config.websocket.port;
    let max_conn = config.websocket.max_connections;
    let ws_handle = tokio::spawn(async move {
        if let Err(e) = ws::server::start(ws_bridge, &ws_addr, ws_port, max_conn, shutdown_rx).await
        {
            tracing::error!(error = %e, "WebSocket server error");
        }
    });

    // Wait for shutdown signal
    tokio::signal::ctrl_c()
        .await
        .expect("failed to listen for ctrl+c");
    tracing::info!("shutdown signal received");

    let _ = shutdown_tx.send(());

    // Wait for WS server to finish (poll loop will be cancelled when the runtime shuts down)
    let _ = ws_handle.await;
    poll_handle.abort();
}

fn run_healthcheck(args: &[String]) -> i32 {
    let config = match load_config_from_args(args) {
        Ok(c) => c,
        Err(_) => Config::default(),
    };

    let host = if config.websocket.addr == "0.0.0.0" {
        "127.0.0.1"
    } else {
        &config.websocket.addr
    };
    let addr = format!("{host}:{}", config.websocket.port);
    let sock_addr = match addr.parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("healthcheck: invalid address {addr}: {e}");
            return 1;
        }
    };
    match TcpStream::connect_timeout(&sock_addr, Duration::from_secs(5)) {
        Ok(mut stream) => {
            let request = format!("GET /health HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
            if stream.write_all(request.as_bytes()).is_err() {
                eprintln!("healthcheck: FAIL (write error)");
                return 1;
            }
            let mut response = String::new();
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            if stream.read_to_string(&mut response).is_err() {
                eprintln!("healthcheck: FAIL (read error)");
                return 1;
            }
            if response.starts_with("HTTP/1.1 200") {
                println!("healthcheck: OK");
                0
            } else {
                eprintln!("healthcheck: FAIL (unexpected response)");
                1
            }
        }
        Err(e) => {
            eprintln!("healthcheck: FAIL ({e})");
            1
        }
    }
}

fn parse_verbosity(args: &[String]) -> Option<&str> {
    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "-vv" => return Some("trace"),
            "-v" => return Some("debug"),
            _ => {}
        }
    }
    None
}

fn load_config_from_args(args: &[String]) -> Result<Config, cyclone_dds_ws_bridge::config::ConfigError> {
    let config_path = args
        .windows(2)
        .find(|w| w[0] == "--config")
        .map(|w| PathBuf::from(&w[1]));

    let mut config = match config_path {
        Some(path) => Config::load(&path)?,
        None => Config::from_defaults()?,
    };

    if let Some(level) = parse_verbosity(args) {
        config.logging.level = level.to_string();
    }

    Ok(config)
}
