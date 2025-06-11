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

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use prost_types::Timestamp;
use regex::Regex;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, error, instrument};

use crate::config::Config;
use crate::grpc_server::{internal_error, permission_denied, proto, ResponseStream};
use crate::kubernetes::KubernetesClient;
use crate::rgkl_integration::{RgklIntegration, FollowFromType};

use proto::{
    log_records_service_server::LogRecordsService, FollowFrom, LogRecord,
    LogRecordsStreamRequest,
};

// Log file pattern matching the Go implementation:
// {pod}_{namespace}_{container}-{containerID}.log
static LOG_FILE_REGEX: &str = r"^([^_]+)_([^_]+)_([^-]+)-([^.]+)\.log$";

pub struct LogRecordsServiceImpl {
    config: Arc<Config>,
    kubernetes_client: KubernetesClient,
    rgkl_integration: RgklIntegration,
}

impl LogRecordsServiceImpl {
    pub fn new(
        config: Arc<Config>,
        kubernetes_client: KubernetesClient,
        rgkl_integration: RgklIntegration,
    ) -> Self {
        Self {
            config,
            kubernetes_client,
            rgkl_integration,
        }
    }

    /// Find log file for given container
    /// This replaces the Go findLogFile function
    fn find_log_file(
        &self,
        namespace: &str,
        pod_name: &str,
        container_name: &str,
        container_id: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let container_logs_dir = &self.config.container_logs_dir;
        
        // Strip runtime prefix from container ID (docker://, containerd://, etc.)
        let stripped_id = if let Some(pos) = container_id.find("://") {
            &container_id[pos + 3..]
        } else {
            container_id
        };

        // Build expected file name pattern
        let pattern = format!("{}_{}_{}-*.log", pod_name, namespace, container_name);
        let pattern_regex = Regex::new(&pattern.replace("*", r"[^.]+"))?;

        // Search for matching files
        let entries = std::fs::read_dir(container_logs_dir)?;
        
        for entry in entries {
            let entry = entry?;
            let file_name = entry.file_name();
            let file_name_str = file_name.to_str().unwrap_or("");

            if pattern_regex.is_match(file_name_str) {
                // Additional check to match container ID
                if file_name_str.contains(stripped_id) {
                    return Ok(entry.path().to_string_lossy().to_string());
                }
            }
        }

        Err(format!(
            "Log file not found for pod {}/{}, container {}, id {}",
            namespace, pod_name, container_name, stripped_id
        ).into())
    }

    /// Convert protobuf FollowFrom to rgkl FollowFromType
    fn convert_follow_from(follow_from: i32) -> FollowFromType {
        match FollowFrom::try_from(follow_from).unwrap_or(FollowFrom::Noop) {
            FollowFrom::Noop => FollowFromType::Noop,
            FollowFrom::Default => FollowFromType::Default,
            FollowFrom::End => FollowFromType::End,
        }
    }

    /// Convert rgkl LogRecord to protobuf LogRecord
    fn convert_log_record(record: crate::rgkl_integration::LogRecord) -> LogRecord {
        let timestamp = record.timestamp.map(|dt| Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        });

        LogRecord {
            timestamp,
            message: record.message,
        }
    }
}

#[tonic::async_trait]
impl LogRecordsService for LogRecordsServiceImpl {
    type StreamForwardStream = ResponseStream<LogRecord>;

    #[instrument(skip(self))]
    async fn stream_forward(
        &self,
        request: Request<LogRecordsStreamRequest>,
    ) -> Result<Response<Self::StreamForwardStream>, Status> {
        let req = request.into_inner();
        debug!(
            "Received StreamForward request for {}/{}/{}",
            req.namespace, req.pod_name, req.container_name
        );

        // Check permissions
        self.kubernetes_client
            .check_permissions(&[req.namespace.clone()], "list")
            .await
            .map_err(|e| {
                if e.to_string().contains("Access denied") {
                    permission_denied(&e.to_string())
                } else {
                    internal_error(e)
                }
            })?;

        // Find the log file
        let file_path = self
            .find_log_file(&req.namespace, &req.pod_name, &req.container_name, &req.container_id)
            .map_err(|e| internal_error(format!("Failed to find log file: {}", e)))?;

        debug!("Found log file: {}", file_path);

        // Parse time parameters
        let start_time = if req.start_time.is_empty() {
            None
        } else {
            req.start_time.parse::<DateTime<Utc>>()
                .map_err(|e| internal_error(format!("Invalid start_time: {}", e)))?
                .into()
        };

        let stop_time = if req.stop_time.is_empty() {
            None
        } else {
            req.stop_time.parse::<DateTime<Utc>>()
                .map_err(|e| internal_error(format!("Invalid stop_time: {}", e)))?
                .into()
        };

        let follow_from = Self::convert_follow_from(req.follow_from);

        // Create channel for streaming records
        let (rgkl_tx, mut rgkl_rx) = tokio::sync::mpsc::channel(1000);
        let (grpc_tx, grpc_rx) = tokio::sync::mpsc::channel(1000);

        // Start rgkl stream forward
        let rgkl_integration = self.rgkl_integration.clone();
        let file_path_clone = file_path.clone();
        let grep = req.grep.clone();

        tokio::spawn(async move {
            if let Err(e) = rgkl_integration
                .stream_forward(
                    &file_path_clone,
                    start_time,
                    stop_time,
                    &grep,
                    follow_from,
                    rgkl_tx,
                )
                .await
            {
                error!("RGKL stream forward error: {}", e);
            }
        });

        // Convert rgkl records to protobuf records
        tokio::spawn(async move {
            while let Some(rgkl_record) = rgkl_rx.recv().await {
                let grpc_record = Self::convert_log_record(rgkl_record);
                
                if grpc_tx.send(Ok(grpc_record)).await.is_err() {
                    debug!("gRPC stream closed, stopping conversion");
                    break;
                }
            }
        });

        let stream = ReceiverStream::new(grpc_rx);
        Ok(Response::new(Box::pin(stream)))
    }

    type StreamBackwardStream = ResponseStream<LogRecord>;

    #[instrument(skip(self))]
    async fn stream_backward(
        &self,
        request: Request<LogRecordsStreamRequest>,
    ) -> Result<Response<Self::StreamBackwardStream>, Status> {
        let req = request.into_inner();
        debug!(
            "Received StreamBackward request for {}/{}/{}",
            req.namespace, req.pod_name, req.container_name
        );

        // Check permissions
        self.kubernetes_client
            .check_permissions(&[req.namespace.clone()], "list")
            .await
            .map_err(|e| {
                if e.to_string().contains("Access denied") {
                    permission_denied(&e.to_string())
                } else {
                    internal_error(e)
                }
            })?;

        // Find the log file
        let file_path = self
            .find_log_file(&req.namespace, &req.pod_name, &req.container_name, &req.container_id)
            .map_err(|e| internal_error(format!("Failed to find log file: {}", e)))?;

        debug!("Found log file: {}", file_path);

        // Parse time parameters
        let start_time = if req.start_time.is_empty() {
            None
        } else {
            req.start_time.parse::<DateTime<Utc>>()
                .map_err(|e| internal_error(format!("Invalid start_time: {}", e)))?
                .into()
        };

        let stop_time = if req.stop_time.is_empty() {
            None
        } else {
            req.stop_time.parse::<DateTime<Utc>>()
                .map_err(|e| internal_error(format!("Invalid stop_time: {}", e)))?
                .into()
        };

        // Create channel for streaming records
        let (rgkl_tx, mut rgkl_rx) = tokio::sync::mpsc::channel(1000);
        let (grpc_tx, grpc_rx) = tokio::sync::mpsc::channel(1000);

        // Start rgkl stream backward
        let rgkl_integration = self.rgkl_integration.clone();
        let file_path_clone = file_path.clone();
        let grep = req.grep.clone();

        tokio::spawn(async move {
            if let Err(e) = rgkl_integration
                .stream_backward(
                    &file_path_clone,
                    start_time,
                    stop_time,
                    &grep,
                    rgkl_tx,
                )
                .await
            {
                error!("RGKL stream backward error: {}", e);
            }
        });

        // Convert rgkl records to protobuf records
        tokio::spawn(async move {
            while let Some(rgkl_record) = rgkl_rx.recv().await {
                let grpc_record = Self::convert_log_record(rgkl_record);
                
                if grpc_tx.send(Ok(grpc_record)).await.is_err() {
                    debug!("gRPC stream closed, stopping conversion");
                    break;
                }
            }
        });

        let stream = ReceiverStream::new(grpc_rx);
        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_follow_from_conversion() {
        assert_eq!(
            matches!(LogRecordsServiceImpl::convert_follow_from(0), FollowFromType::Noop),
            true
        );
        assert_eq!(
            matches!(LogRecordsServiceImpl::convert_follow_from(1), FollowFromType::Default),
            true
        );
        assert_eq!(
            matches!(LogRecordsServiceImpl::convert_follow_from(2), FollowFromType::End),
            true
        );
    }

    #[test]
    fn test_log_record_conversion() {
        let rgkl_record = crate::rgkl_integration::LogRecord {
            timestamp: Some(DateTime::from_timestamp(1234567890, 0).unwrap()),
            message: "test message".to_string(),
        };

        let grpc_record = LogRecordsServiceImpl::convert_log_record(rgkl_record);
        assert_eq!(grpc_record.message, "test message");
        assert!(grpc_record.timestamp.is_some());
    }

    #[test]
    fn test_strip_runtime_prefix() {
        // Test container ID parsing logic
        let test_cases = [
            ("docker://abc123", "abc123"),
            ("containerd://def456", "def456"),
            ("abc123", "abc123"), // No prefix
        ];

        for (input, expected) in test_cases {
            let result = if let Some(pos) = input.find("://") {
                &input[pos + 3..]
            } else {
                input
            };
            assert_eq!(result, expected);
        }
    }
} 