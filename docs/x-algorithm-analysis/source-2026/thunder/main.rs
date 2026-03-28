use anyhow::{Context, Result};
use axum::Router;
use clap::Parser;
use log::info;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tonic::service::Routes;
use xai_http_server::{CancellationToken, GrpcConfig, HttpServer};

use thunder::{
    args, kafka_utils, posts::post_store::PostStore, strato_client::StratoClient,
    thunder_service::ThunderServiceImpl,
};

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();
    let args = args::Args::parse();

    // Initialize PostStore
    let post_store = Arc::new(PostStore::new(
        args.post_retention_seconds,
        args.request_timeout_ms,
    ));
    info!(
        "Initialized PostStore for in-memory post storage (retention: {} seconds / {:.1} days, request_timeout: {}ms)",
        args.post_retention_seconds,
        args.post_retention_seconds as f64 / 86400.0,
        args.request_timeout_ms
    );

    // Initialize StratoClient for fetching following lists
    let strato_client = Arc::new(StratoClient::new());
    info!("Initialized StratoClient");

    // Create ThunderService with the PostStore, StratoClient, and concurrency limit
    let thunder_service = ThunderServiceImpl::new(
        Arc::clone(&post_store),
        Arc::clone(&strato_client),
        args.max_concurrent_requests,
    );
    info!(
        "Initialized with max_concurrent_requests={}",
        args.max_concurrent_requests
    );
    let routes = Routes::new(thunder_service.server());

    // Set up gRPC config
    let grpc_config = GrpcConfig::new(args.grpc_port, routes);

    // Create HTTP server with gRPC support
    let mut http_server = HttpServer::new(
        args.http_port,
        Router::new(),
        Some(grpc_config),
        CancellationToken::new(),
        Duration::from_secs(10),
    )
    .await
    .context("Failed to create HTTP server")?;

    if args.enable_profiling {
        xai_profiling::spawn_server(3000, CancellationToken::new()).await;
    }

    // Create channel for post events
    let (tx, mut rx) = tokio::sync::mpsc::channel::<i64>(args.kafka_num_threads);
    kafka_utils::start_kafka(&args, post_store.clone(), "", tx).await?;

    if args.is_serving {
        // Wait for Kafka catchup signal
        let start = Instant::now();
        for _ in 0..args.kafka_num_threads {
            rx.recv().await;
        }
        info!("Kafka init took {:?}", start.elapsed());

        post_store.finalize_init().await?;

        // Start stats logger
        Arc::clone(&post_store).start_stats_logger();
        info!("Started PostStore stats logger",);

        // Start auto-trim task to remove posts older than retention period
        Arc::clone(&post_store).start_auto_trim(2); // Run every 2 minutes
        info!(
            "Started PostStore auto-trim task (interval: 2 minutes, retention: {:.1} days)",
            args.post_retention_seconds as f64 / 86400.0
        );
    }

    http_server.set_readiness(true);
    info!("HTTP/gRPC server is ready");

    // Wait for termination signal
    http_server.wait_for_termination().await;
    info!("Server terminated");

    Ok(())
}
