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

use std::fs;
use std::path::Path;
use std::sync::Arc;

use prost_types::Timestamp;
use regex::Regex;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, instrument};

use crate::config::Config;
use crate::file_watcher::{ContainerLogsWatcher, LogFileEvent, LogFileEventType};
use crate::grpc_server::{internal_error, permission_denied, proto, ResponseStream};
use crate::kubernetes::KubernetesClient;

use proto::{
    log_metadata_service_server::LogMetadataService, LogMetadata, LogMetadataFileInfo,
    LogMetadataList, LogMetadataListRequest, LogMetadataSpec, LogMetadataWatchEvent,
    LogMetadataWatchRequest,
};

// Log file pattern matching the Go implementation:
// {pod}_{namespace}_{container}-{containerID}.log
static LOG_FILE_REGEX: &str = r"^([^_]+)_([^_]+)_([^-]+)-([^.]+)\.log$";

pub struct LogMetadataServiceImpl {
    config: Arc<Config>,
    kubernetes_client: KubernetesClient,
    file_watcher: ContainerLogsWatcher,
    node_name: String,
}

impl LogMetadataServiceImpl {
    pub fn new(
        config: Arc<Config>,
        kubernetes_client: KubernetesClient,
        file_watcher: ContainerLogsWatcher,
    ) -> Self {
        let node_name = std::env::var("NODE_NAME").unwrap_or_else(|_| "unknown".to_string());

        Self {
            config,
            kubernetes_client,
            file_watcher,
            node_name,
        }
    }

    /// Create LogMetadata from a log file path
    fn create_log_metadata(&self, file_path: &Path) -> Result<LogMetadata, Box<dyn std::error::Error>> {
        let file_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("Invalid file name")?;

        // Parse file name using regex
        let regex = Regex::new(LOG_FILE_REGEX)?;
        let captures = regex
            .captures(file_name)
            .ok_or("File name doesn't match expected pattern")?;

        let pod_name = captures.get(1).unwrap().as_str().to_string();
        let namespace = captures.get(2).unwrap().as_str().to_string();
        let container_name = captures.get(3).unwrap().as_str().to_string();
        let container_id = captures.get(4).unwrap().as_str().to_string();

        // Get file metadata
        let metadata = fs::metadata(file_path)?;
        let size = metadata.len() as i64;
        let modified = metadata.modified()?;
        let timestamp = modified
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs() as i64;

        let file_info = LogMetadataFileInfo {
            size,
            last_modified_at: Some(Timestamp {
                seconds: timestamp,
                nanos: 0,
            }),
        };

        let spec = LogMetadataSpec {
            node_name: self.node_name.clone(),
            namespace: namespace.clone(),
            pod_name: pod_name.clone(),
            container_name: container_name.clone(),
            container_id: container_id.clone(),
        };

        Ok(LogMetadata {
            id: container_id,
            spec: Some(spec),
            file_info: Some(file_info),
        })
    }

    /// Convert file watcher event to LogMetadataWatchEvent
    fn create_watch_event(&self, event: LogFileEvent) -> LogMetadataWatchEvent {
        let event_type = match event.event_type {
            LogFileEventType::Created => "ADDED",
            LogFileEventType::Modified => "MODIFIED",
            LogFileEventType::Deleted => "DELETED",
        };

        let metadata = LogMetadata {
            id: event.container_id.clone(),
            spec: Some(LogMetadataSpec {
                node_name: self.node_name.clone(),
                namespace: event.namespace,
                pod_name: event.pod_name,
                container_name: event.container_name,
                container_id: event.container_id,
            }),
            file_info: None, // File info will be populated by scanning if needed
        };

        LogMetadataWatchEvent {
            r#type: event_type.to_string(),
            object: Some(metadata),
        }
    }
}

#[tonic::async_trait]
impl LogMetadataService for LogMetadataServiceImpl {
    #[instrument(skip(self))]
    async fn list(
        &self,
        request: Request<LogMetadataListRequest>,
    ) -> Result<Response<LogMetadataList>, Status> {
        let req = request.into_inner();
        debug!("Received LogMetadataListRequest for namespaces: {:?}", req.namespaces);

        // Check permissions
        self.kubernetes_client
            .check_permissions(&req.namespaces, "list")
            .await
            .map_err(|e| {
                if e.to_string().contains("Access denied") {
                    permission_denied(&e.to_string())
                } else {
                    internal_error(e)
                }
            })?;

        // Scan container logs directory
        let container_logs_dir = &self.config.container_logs_dir;
        let entries = fs::read_dir(container_logs_dir)
            .map_err(|e| internal_error(format!("Failed to read container logs directory: {}", e)))?;

        let mut items = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| internal_error(format!("Failed to read directory entry: {}", e)))?;
            let path = entry.path();

            // Only process .log files
            if path.extension().and_then(|s| s.to_str()) != Some("log") {
                continue;
            }

            match self.create_log_metadata(&path) {
                Ok(metadata) => {
                    // Check if this metadata's namespace is in the requested namespaces
                    if let Some(ref spec) = metadata.spec {
                        if req.namespaces.is_empty() 
                            || req.namespaces.contains(&"".to_string())
                            || req.namespaces.contains(&spec.namespace) {
                            items.push(metadata);
                        }
                    }
                }
                Err(e) => {
                    debug!("Failed to create metadata for {}: {}", path.display(), e);
                    // Continue processing other files
                }
            }
        }

        debug!("Returning {} log metadata items", items.len());
        Ok(Response::new(LogMetadataList { items }))
    }

    type WatchStream = ResponseStream<LogMetadataWatchEvent>;

    #[instrument(skip(self))]
    async fn watch(
        &self,
        request: Request<LogMetadataWatchRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.into_inner();
        debug!("Received LogMetadataWatchRequest for namespaces: {:?}", req.namespaces);

        // Check permissions
        self.kubernetes_client
            .check_permissions(&req.namespaces, "watch")
            .await
            .map_err(|e| {
                if e.to_string().contains("Access denied") {
                    permission_denied(&e.to_string())
                } else {
                    internal_error(e)
                }
            })?;

        // Create a channel for streaming watch events
        let (tx, rx) = tokio::sync::mpsc::channel(1000);

        // For now, we'll create a placeholder implementation
        // In a real implementation, we'd need to properly handle the file watcher
        // without requiring Clone on non-cloneable types
        let allowed_namespaces = req.namespaces.clone();
        let node_name = self.node_name.clone();

        tokio::spawn(async move {
            // TODO: Implement proper file watching without requiring Clone
            // For now, this is a placeholder that will be completed in Phase Two
            debug!("File watcher task started for namespaces: {:?}", allowed_namespaces);
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            debug!("File watcher placeholder task completed");
        });

        let stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(stream)))
    }
}

// Note: Clone implementation removed since file watcher doesn't support cloning
// In a production implementation, we'd use Arc<Mutex<>> or channels for sharing state

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_log_file_regex() {
        let regex = Regex::new(LOG_FILE_REGEX).unwrap();
        
        let test_cases = [
            ("test-pod_default_app-container123.log", true),
            ("web-server_production_nginx-abc456def.log", true),
            ("invalid.log", false),
            ("pod_namespace.log", false),
        ];

        for (filename, should_match) in test_cases {
            assert_eq!(regex.is_match(filename), should_match, "Failed for: {}", filename);
        }
    }

    #[test]
    fn test_create_log_metadata() {
        // This test would need a proper setup with temp files
        // For now, just test that the function exists
        let config = Arc::new(Config::default());
        let kubernetes_client = KubernetesClient::new(config.clone());
        // We can't easily test this without mocking the file system
    }
} 