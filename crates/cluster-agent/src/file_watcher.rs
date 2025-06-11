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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use regex::Regex;
use tokio::sync::mpsc;
use tracing::{debug, error, info, instrument, warn};

use crate::config::Config;

// Log file pattern matching the Go implementation:
// {pod}_{namespace}_{container}-{containerID}.log
static LOG_FILE_REGEX: &str = r"^([^_]+)_([^_]+)_([^-]+)-([^.]+)\.log$";

#[derive(Debug, Clone)]
pub struct LogFileEvent {
    pub event_type: LogFileEventType,
    pub namespace: String,
    pub pod_name: String,
    pub container_name: String,
    pub container_id: String,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogFileEventType {
    Created,
    Modified,
    Deleted,
}

pub struct ContainerLogsWatcher {
    _watcher: RecommendedWatcher,
    event_receiver: mpsc::Receiver<LogFileEvent>,
    _debouncer_handle: tokio::task::JoinHandle<()>,
}

impl ContainerLogsWatcher {
    /// Create a new container logs watcher
    #[instrument(skip(config))]
    pub async fn new(
        config: Arc<Config>,
        allowed_namespaces: Vec<String>,
    ) -> Result<Self> {
        let container_logs_dir = config.container_logs_dir.clone();
        let debounce_duration = Duration::from_millis(config.file_watcher.debounce_ms);
        
        info!("Starting file watcher for directory: {}", container_logs_dir);
        
        // Create channels for raw file system events
        let (raw_tx, raw_rx) = crossbeam_channel::unbounded();
        
        // Create channel for processed log file events
        let (event_tx, event_rx) = mpsc::channel(config.file_watcher.buffer_size);
        
        // Create the file system watcher
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                match res {
                    Ok(event) => {
                        if let Err(e) = raw_tx.send(event) {
                            error!("Failed to send file system event: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("File system watcher error: {}", e);
                    }
                }
            },
            notify::Config::default(),
        )
        .context("Failed to create file system watcher")?;

        // Start watching the container logs directory
        watcher
            .watch(
                Path::new(&container_logs_dir),
                RecursiveMode::NonRecursive,
            )
            .with_context(|| {
                format!("Failed to watch container logs directory: {}", container_logs_dir)
            })?;

        // Spawn debouncer task
        let debouncer_handle = tokio::spawn(Self::debouncer_task(
            raw_rx,
            event_tx,
            allowed_namespaces,
            debounce_duration,
            container_logs_dir,
        ));

        Ok(Self {
            _watcher: watcher,
            event_receiver: event_rx,
            _debouncer_handle: debouncer_handle,
        })
    }

    /// Get the next log file event
    pub async fn next_event(&mut self) -> Option<LogFileEvent> {
        self.event_receiver.recv().await
    }

    /// Debouncer task that processes raw file system events
    async fn debouncer_task(
        raw_rx: Receiver<Event>,
        event_tx: mpsc::Sender<LogFileEvent>,
        allowed_namespaces: Vec<String>,
        debounce_duration: Duration,
        container_logs_dir: String,
    ) {
        let regex = match Regex::new(LOG_FILE_REGEX) {
            Ok(r) => r,
            Err(e) => {
                error!("Failed to compile log file regex: {}", e);
                return;
            }
        };

        let mut pending_events: HashMap<PathBuf, (Event, Instant)> = HashMap::new();
        let mut interval = tokio::time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                // Process raw file system events
                _ = async {
                    if let Ok(event) = raw_rx.recv() {
                        Self::process_raw_event(event, &mut pending_events);
                    }
                } => {},
                
                // Process debounced events periodically
                _ = interval.tick() => {
                    Self::process_debounced_events(
                        &mut pending_events,
                        &event_tx,
                        &allowed_namespaces,
                        &regex,
                        &container_logs_dir,
                        debounce_duration,
                    ).await;
                }
            }
        }
    }

    fn process_raw_event(event: Event, pending_events: &mut HashMap<PathBuf, (Event, Instant)>) {
        for path in &event.paths {
            // Only process .log files
            if let Some(extension) = path.extension() {
                if extension == "log" {
                    pending_events.insert(path.clone(), (event.clone(), Instant::now()));
                }
            }
        }
    }

    async fn process_debounced_events(
        pending_events: &mut HashMap<PathBuf, (Event, Instant)>,
        event_tx: &mpsc::Sender<LogFileEvent>,
        allowed_namespaces: &[String],
        regex: &Regex,
        container_logs_dir: &str,
        debounce_duration: Duration,
    ) {
        let now = Instant::now();
        let mut to_process = Vec::new();

        // Find events that have been pending long enough
        pending_events.retain(|path, (event, timestamp)| {
            if now.duration_since(*timestamp) >= debounce_duration {
                to_process.push((path.clone(), event.clone()));
                false // Remove from pending
            } else {
                true // Keep pending
            }
        });

        // Process the debounced events
        for (path, event) in to_process {
            if let Some(log_event) = Self::create_log_file_event(
                path,
                event,
                allowed_namespaces,
                regex,
                container_logs_dir,
            ) {
                if let Err(e) = event_tx.send(log_event).await {
                    error!("Failed to send log file event: {}", e);
                }
            }
        }
    }

    fn create_log_file_event(
        path: PathBuf,
        event: Event,
        allowed_namespaces: &[String],
        regex: &Regex,
        container_logs_dir: &str,
    ) -> Option<LogFileEvent> {
        let file_name = path.file_name()?.to_str()?;
        
        // Parse the log file name using regex
        let captures = regex.captures(file_name)?;
        let pod_name = captures.get(1)?.as_str().to_string();
        let namespace = captures.get(2)?.as_str().to_string();
        let container_name = captures.get(3)?.as_str().to_string();
        let container_id = captures.get(4)?.as_str().to_string();

        // Check if namespace is allowed
        if !allowed_namespaces.is_empty() 
            && !allowed_namespaces.contains(&namespace) 
            && !allowed_namespaces.contains(&"".to_string()) {
            debug!("Ignoring event for namespace '{}' (not in allowed list)", namespace);
            return None;
        }

        // Determine event type
        let event_type = match &event.kind {
            EventKind::Create(_) => LogFileEventType::Created,
            EventKind::Modify(_) => LogFileEventType::Modified,
            EventKind::Remove(_) => LogFileEventType::Deleted,
            _ => {
                debug!("Ignoring unsupported event kind: {:?}", event.kind);
                return None;
            }
        };

        debug!(
            "Processed log file event: {:?} for {}/{}/{} ({})",
            event_type, namespace, pod_name, container_name, container_id
        );

        Some(LogFileEvent {
            event_type,
            namespace,
            pod_name,
            container_name,
            container_id,
            file_path: path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::TempDir;

    fn test_config() -> Arc<Config> {
        let mut config = Config::default();
        config.file_watcher.debounce_ms = 100; // Short debounce for testing
        config.file_watcher.buffer_size = 100;
        Arc::new(config)
    }

    #[test]
    fn test_log_file_regex() {
        let regex = Regex::new(LOG_FILE_REGEX).unwrap();
        
        // Valid log file names
        let valid_cases = [
            ("test-pod_default_app-container123.log", ("test-pod", "default", "app", "container123")),
            ("web-server_production_nginx-abc456def.log", ("web-server", "production", "nginx", "abc456def")),
        ];

        for (filename, (pod, namespace, container, id)) in valid_cases {
            let captures = regex.captures(filename).unwrap();
            assert_eq!(captures.get(1).unwrap().as_str(), pod);
            assert_eq!(captures.get(2).unwrap().as_str(), namespace);
            assert_eq!(captures.get(3).unwrap().as_str(), container);
            assert_eq!(captures.get(4).unwrap().as_str(), id);
        }

        // Invalid log file names
        let invalid_cases = [
            "invalid.log",
            "pod_namespace.log",
            "pod_namespace_container.log",
            "not_a_log_file.txt",
        ];

        for filename in invalid_cases {
            assert!(regex.captures(filename).is_none());
        }
    }

    #[tokio::test]
    async fn test_create_log_file_event() {
        let regex = Regex::new(LOG_FILE_REGEX).unwrap();
        let allowed_namespaces = vec!["default".to_string(), "test".to_string()];
        let container_logs_dir = "/var/log/containers";

        let path = PathBuf::from("/var/log/containers/test-pod_default_app-container123.log");
        let event = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };

        let log_event = ContainerLogsWatcher::create_log_file_event(
            path,
            event,
            &allowed_namespaces,
            &regex,
            container_logs_dir,
        );

        assert!(log_event.is_some());
        let log_event = log_event.unwrap();
        assert_eq!(log_event.event_type, LogFileEventType::Created);
        assert_eq!(log_event.namespace, "default");
        assert_eq!(log_event.pod_name, "test-pod");
        assert_eq!(log_event.container_name, "app");
        assert_eq!(log_event.container_id, "container123");
    }

    #[tokio::test]
    async fn test_namespace_filtering() {
        let regex = Regex::new(LOG_FILE_REGEX).unwrap();
        let allowed_namespaces = vec!["allowed".to_string()];
        let container_logs_dir = "/var/log/containers";

        let path = PathBuf::from("/var/log/containers/test-pod_forbidden_app-container123.log");
        let event = Event {
            kind: EventKind::Create(notify::event::CreateKind::File),
            paths: vec![path.clone()],
            attrs: Default::default(),
        };

        let log_event = ContainerLogsWatcher::create_log_file_event(
            path,
            event,
            &allowed_namespaces,
            &regex,
            container_logs_dir,
        );

        // Should return None for namespace not in allowed list
        assert!(log_event.is_none());
    }
} 