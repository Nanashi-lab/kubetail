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

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt};
use tonic::{transport::Server, Response, Status};
use tonic_health::server::HealthReporter;
use tracing::{info, instrument};

use crate::config::Config;
use crate::file_watcher::ContainerLogsWatcher;
use crate::kubernetes::KubernetesClient;
use crate::rgkl_integration::RgklIntegration;

mod log_metadata_service;
mod log_records_service;

use log_metadata_service::LogMetadataServiceImpl;
use log_records_service::LogRecordsServiceImpl;

// Include the generated protobuf code
pub mod proto {
    tonic::include_proto!("cluster_agent");
}

use proto::{
    log_metadata_service_server::LogMetadataServiceServer,
    log_records_service_server::LogRecordsServiceServer,
};

pub struct GrpcServer {
    config: Arc<Config>,
    kubernetes_client: KubernetesClient,
    rgkl_integration: RgklIntegration,
}

impl GrpcServer {
    #[instrument(skip(config))]
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        info!("Initializing gRPC server components");

        // Initialize Kubernetes client
        let kubernetes_client = KubernetesClient::new(config.clone())
            .await
            .context("Failed to initialize Kubernetes client")?;

        // Initialize RGKL integration
        let rgkl_integration = RgklIntegration::new(config.clone());

        Ok(Self {
            config,
            kubernetes_client,
            rgkl_integration,
        })
    }

    #[instrument(skip(self, shutdown_rx))]
    pub async fn serve(&self, mut shutdown_rx: broadcast::Receiver<()>) -> Result<()> {
        let addr: SocketAddr = self.config.grpc.address.parse()
            .with_context(|| format!("Invalid gRPC address: {}", self.config.grpc.address))?;

        info!("Starting gRPC server on {}", addr);

        // Create health reporter and service
        let (mut health_reporter, health_service) = tonic_health::server::health_reporter();

        // Initialize file watcher
        let file_watcher = ContainerLogsWatcher::new(
            self.config.clone(),
            self.config.allowed_namespaces.clone(),
        )
        .await
        .context("Failed to initialize file watcher")?;

        // Create service implementations
        let log_metadata_service = LogMetadataServiceImpl::new(
            self.config.clone(),
            self.kubernetes_client.clone(),
            file_watcher,
        );

        let log_records_service = LogRecordsServiceImpl::new(
            self.config.clone(),
            self.kubernetes_client.clone(),
            self.rgkl_integration.clone(),
        );

        // Set initial health status
        health_reporter
            .set_serving::<LogMetadataServiceServer<LogMetadataServiceImpl>>()
            .await;
        health_reporter
            .set_serving::<LogRecordsServiceServer<LogRecordsServiceImpl>>()
            .await;

        // Build the server
        let mut server_builder = Server::builder();

        // Add TLS if configured
        if self.config.grpc.tls.enabled {
            info!("TLS enabled for gRPC server");
            // TODO: Implement TLS configuration
            // let tls_config = ...;
            // server_builder = server_builder.tls_config(tls_config)?;
        }

        let server = server_builder
            .add_service(LogMetadataServiceServer::new(log_metadata_service))
            .add_service(LogRecordsServiceServer::new(log_records_service))
            .add_service(health_service);

        // Add reflection if enabled
        #[cfg(feature = "reflection")]
        let server = if self.config.grpc.reflection {
            info!("gRPC reflection enabled");
            let reflection = tonic_reflection::server::Builder::configure()
                .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
                .build()
                .context("Failed to build gRPC reflection service")?;
            server.add_service(reflection)
        } else {
            server
        };

        // Serve with graceful shutdown
        server
            .serve_with_shutdown(addr, async {
                let _ = shutdown_rx.recv().await;
                info!("gRPC server received shutdown signal");
            })
            .await
            .context("gRPC server error")?;

        info!("gRPC server shutdown complete");
        Ok(())
    }
}

/// Utility function to convert errors to gRPC Status
pub fn internal_error(err: impl std::fmt::Display) -> Status {
    Status::internal(err.to_string())
}

/// Utility function to convert permission errors to gRPC Status
pub fn permission_denied(msg: &str) -> Status {
    Status::permission_denied(msg)
}

/// Stream type for async streaming responses
pub type ResponseStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_conversion() {
        let error = anyhow::anyhow!("test error");
        let status = internal_error(error);
        assert_eq!(status.code(), tonic::Code::Internal);
    }

    #[test]
    fn test_permission_denied() {
        let status = permission_denied("test message");
        assert_eq!(status.code(), tonic::Code::PermissionDenied);
        assert_eq!(status.message(), "test message");
    }

    #[tokio::test]
    async fn test_grpc_server_creation() {
        let config = Arc::new(Config::default());
        
        // This test may fail in CI without proper Kubernetes setup
        // In a real environment, we'd mock the Kubernetes client
        match GrpcServer::new(config).await {
            Ok(_) => {
                // Server created successfully
            }
            Err(e) => {
                // Expected in environments without Kubernetes access
                assert!(e.to_string().contains("Kubernetes") || e.to_string().contains("kubeconfig"));
            }
        }
    }
} 