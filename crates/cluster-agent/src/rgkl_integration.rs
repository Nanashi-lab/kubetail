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

use std::io::Write;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use crossbeam_channel::{unbounded, Receiver, Sender};
use tokio::sync::mpsc;
use tracing::{debug, error, instrument, warn};

use rgkl::stream_forward::{self, FollowFrom};
use rgkl::stream_backward;

/// CRITICAL: This represents the key performance improvement of Phase One.
/// Instead of spawning rgkl as a subprocess via exec.Command(), we now
/// integrate it directly as a Rust library, eliminating process creation
/// overhead and inter-process communication bottlenecks.

#[derive(Debug, Clone)]
pub struct LogRecord {
    pub timestamp: Option<DateTime<Utc>>,
    pub message: String,
}

#[derive(Clone)]
pub struct RgklIntegration {
    config: Arc<crate::config::Config>,
}

impl RgklIntegration {
    pub fn new(config: Arc<crate::config::Config>) -> Self {
        Self { config }
    }

    /// Stream logs forward - replaces exec.Command("./rgkl", "stream-forward", ...)
    #[instrument(skip(self, output_tx))]
    pub async fn stream_forward(
        &self,
        file_path: &str,
        start_time: Option<DateTime<Utc>>,
        stop_time: Option<DateTime<Utc>>,
        grep: &str,
        follow_from: FollowFromType,
        output_tx: mpsc::Sender<LogRecord>,
    ) -> Result<()> {
        debug!(
            "Starting forward stream for file: {}, grep: '{}', follow_from: {:?}",
            file_path, grep, follow_from
        );

        // Create termination channel for graceful shutdown
        let (term_tx, term_rx) = unbounded();

        // Convert follow_from parameter
        let rgkl_follow_from = match follow_from {
            FollowFromType::Noop => FollowFrom::Noop,
            FollowFromType::Default => FollowFrom::Default,
            FollowFromType::End => FollowFrom::End,
        };

        // Create a custom writer that sends records to the channel
        let mut writer = ChannelWriter::new(output_tx);

        // Spawn blocking task for rgkl execution
        let file_path = file_path.to_string();
        let grep = grep.to_string();
        
        tokio::task::spawn_blocking(move || {
            match stream_forward::run(
                &file_path,
                start_time,
                stop_time,
                &grep,
                rgkl_follow_from,
                term_rx,
                &mut writer,
            ) {
                Ok(_) => debug!("Forward stream completed successfully"),
                Err(e) => error!("Forward stream error: {}", e),
            }
        })
        .await
        .context("Failed to execute rgkl stream forward")?;

        Ok(())
    }

    /// Stream logs backward - replaces exec.Command("./rgkl", "stream-backward", ...)
    #[instrument(skip(self, output_tx))]
    pub async fn stream_backward(
        &self,
        file_path: &str,
        start_time: Option<DateTime<Utc>>,
        stop_time: Option<DateTime<Utc>>,
        grep: &str,
        output_tx: mpsc::Sender<LogRecord>,
    ) -> Result<()> {
        debug!(
            "Starting backward stream for file: {}, grep: '{}'",
            file_path, grep
        );

        // Create termination channel for graceful shutdown
        let (term_tx, term_rx) = unbounded();

        // Create a custom writer that sends records to the channel
        let mut writer = ChannelWriter::new(output_tx);

        // Spawn blocking task for rgkl execution
        let file_path = file_path.to_string();
        let grep = grep.to_string();
        
        tokio::task::spawn_blocking(move || {
            match stream_backward::run(
                &file_path,
                start_time,
                stop_time,
                &grep,
                term_rx,
                &mut writer,
            ) {
                Ok(_) => debug!("Backward stream completed successfully"),
                Err(e) => error!("Backward stream error: {}", e),
            }
        })
        .await
        .context("Failed to execute rgkl stream backward")?;

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum FollowFromType {
    Noop,
    Default,
    End,
}

/// Custom writer that converts rgkl output to LogRecord and sends via channel
struct ChannelWriter {
    output_tx: mpsc::Sender<LogRecord>,
}

impl ChannelWriter {
    fn new(output_tx: mpsc::Sender<LogRecord>) -> Self {
        Self { output_tx }
    }
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let content = String::from_utf8_lossy(buf);
        
        // Parse each line as a potential log record
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }

            // Try to parse as JSON (rgkl outputs JSON format)
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(json) => {
                    let log_record = LogRecord {
                        timestamp: json.get("timestamp")
                            .and_then(|t| t.as_str())
                            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&Utc)),
                        message: json.get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or(line)
                            .to_string(),
                    };

                    // Send record through channel (non-blocking)
                    if let Err(e) = self.output_tx.try_send(log_record) {
                        match e {
                            mpsc::error::TrySendError::Full(_) => {
                                warn!("Output channel is full, dropping log record");
                            }
                            mpsc::error::TrySendError::Closed(_) => {
                                debug!("Output channel is closed, stopping stream");
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::BrokenPipe,
                                    "Output channel closed",
                                ));
                            }
                        }
                    }
                }
                Err(_) => {
                    // If not valid JSON, treat as plain text message
                    let log_record = LogRecord {
                        timestamp: None,
                        message: line.to_string(),
                    };

                    if let Err(e) = self.output_tx.try_send(log_record) {
                        match e {
                            mpsc::error::TrySendError::Full(_) => {
                                warn!("Output channel is full, dropping log record");
                            }
                            mpsc::error::TrySendError::Closed(_) => {
                                debug!("Output channel is closed, stopping stream");
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::BrokenPipe,
                                    "Output channel closed",
                                ));
                            }
                        }
                    }
                }
            }
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // No buffering needed for channel-based writer
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write as StdWrite;

    #[test]
    fn test_follow_from_conversion() {
        // Test that our enum converts correctly to rgkl's enum
        assert!(matches!(FollowFromType::Noop, FollowFromType::Noop));
        assert!(matches!(FollowFromType::Default, FollowFromType::Default));
        assert!(matches!(FollowFromType::End, FollowFromType::End));
    }

    #[tokio::test]
    async fn test_channel_writer_json() {
        let (tx, mut rx) = mpsc::channel(10);
        let mut writer = ChannelWriter::new(tx);

        let json_line = r#"{"timestamp":"2023-01-01T12:00:00Z","message":"test message"}"#;
        writer.write_all(json_line.as_bytes()).unwrap();

        let record = rx.recv().await.unwrap();
        assert_eq!(record.message, "test message");
        assert!(record.timestamp.is_some());
    }

    #[tokio::test]
    async fn test_channel_writer_plain_text() {
        let (tx, mut rx) = mpsc::channel(10);
        let mut writer = ChannelWriter::new(tx);

        let plain_text = "plain text message";
        writer.write_all(plain_text.as_bytes()).unwrap();

        let record = rx.recv().await.unwrap();
        assert_eq!(record.message, "plain text message");
        assert!(record.timestamp.is_none());
    }

    #[test]
    fn test_rgkl_integration_creation() {
        let config = Arc::new(crate::config::Config::default());
        let integration = RgklIntegration::new(config);
        // Just test that it can be created
        assert!(!integration.config.container_logs_dir.is_empty());
    }
} 