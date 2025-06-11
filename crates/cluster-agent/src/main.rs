// Copyright 2024-2025 Andres Morey
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use signal_hook_tokio::Signals;
use futures_util::stream::StreamExt;
use tokio::sync::broadcast;
use tokio::time::timeout;
use tracing::{info, error, warn};

mod config;
mod grpc_server;
mod kubernetes;
mod file_watcher;
mod rgkl_integration;

use config::Config;
use grpc_server::GrpcServer;

/// Kubetail Cluster Agent - High-performance Rust implementation
#[derive(Parser, Debug)]
#[command(version, about = "Kubetail Cluster Agent - Rust implementation")]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "FILE")]
    config: Option<String>,

    /// Host address to bind to
    #[arg(short, long, default_value = ":50051")]
    addr: String,

    /// Configuration parameters in key:value format
    #[arg(short, long)]
    param: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter("kubetail_cluster_agent=debug,rgkl=debug")
        .init();

    let args = Args::parse();

    // Load configuration
    let config = match Config::load(args.config.as_deref(), &args.param, &args.addr) {
        Ok(config) => Arc::new(config),
        Err(e) => {
            error!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };

    info!("Starting Kubetail Cluster Agent (Rust) on {}", config.grpc.address);

    // Create shutdown broadcast channel
    let (shutdown_tx, _) = broadcast::channel(1);

    // Set up signal handling for graceful shutdown
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        let mut signals = Signals::new([signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM])
            .expect("Failed to register signal handlers");

        if let Some(signal) = signals.next().await {
            match signal {
                signal_hook::consts::SIGINT => info!("Received SIGINT, initiating graceful shutdown"),
                signal_hook::consts::SIGTERM => info!("Received SIGTERM, initiating graceful shutdown"),
                _ => warn!("Received unexpected signal: {}", signal),
            }
            let _ = shutdown_tx_clone.send(());
        }
    });

    // Initialize gRPC server
    let grpc_server = match GrpcServer::new(config.clone()).await {
        Ok(server) => server,
        Err(e) => {
            error!("Failed to initialize gRPC server: {}", e);
            std::process::exit(1);
        }
    };

    // Create shutdown receiver before moving grpc_server
    let mut shutdown_rx = shutdown_tx.subscribe();
    let server_shutdown_rx = shutdown_tx.subscribe();

    // Start the server
    let server_handle = tokio::spawn(async move {
        if let Err(e) = grpc_server.serve(server_shutdown_rx).await {
            error!("gRPC server error: {}", e);
        }
    });

    // Wait for shutdown signal
    let _ = shutdown_rx.recv().await;

    info!("Shutdown signal received, waiting for server to stop...");

    // Give the server some time to shut down gracefully
    if let Err(_) = timeout(Duration::from_secs(30), server_handle).await {
        warn!("Server did not shut down within 30 seconds, forcing exit");
    }

    info!("Kubetail Cluster Agent shutdown complete");
    Ok(())
}
