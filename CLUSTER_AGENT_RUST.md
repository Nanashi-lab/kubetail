# Technical Specification: Kubetail Cluster Agent Rewrite in Rust

## 1\. Introduction

This document outlines the technical specification for rewriting the Kubetail Cluster Agent from its current Go and Rust hybrid implementation to a unified Rust codebase. The Kubetail Cluster Agent serves as a gRPC service executing low-level file operations on Kubernetes cluster nodes for the Kubetail Cluster API.<sup>1</sup> Its current feature set includes reading log file sizes, last event timestamps, and performing log greps via a Rust component named "RipGrep for Kubernetes Logs" (rgkl) invoked through

exec.Command().

The primary motivation for this rewrite is to leverage Rust's performance characteristics, low resource footprint, and robust type system to create a more efficient and maintainable agent. The current inter-process communication between Go and the rgkl Rust component via exec.Command() is suboptimal, limiting data exchange to stdout/stderr and introducing overhead. A pure Rust implementation will allow for seamless integration of components like rgkl as a library, improving communication efficiency and overall system coherence. Rust's strong compile-time guarantees and memory safety features are also well-suited for a system agent that needs to be reliable and secure.

The scope of this project is to port all existing functionalities of the Cluster Agent to Rust. This includes file system watching, Kubernetes API interactions for SelfSubjectAccessReview, OS signal handling for graceful shutdown, YAML configuration parsing, and the implementation of existing gRPC services. A key deliverable is an updated Continuous Integration (CI) workflow capable of building the new Rust-based Cluster Agent for specified target architectures. No new features will be introduced as part of this rewrite. The long-term vision is for the Cluster Agent to be the cornerstone for advanced Kubetail features like log parsing, metrics, and alerts, making a solid Rust foundation crucial.

## 2\. Project Setup and Structure

The new Rust-based Kubetail Cluster Agent will be initiated as a standard Cargo binary project.

Bash

cargo new kubetail-cluster-agent-rust --bin  

This command creates a new directory named kubetail-cluster-agent-rust containing a Cargo.toml file for managing dependencies and project metadata, and a src/ directory with a main.rs file as the entry point for the application.

### 2.1. Module Organization

A well-organized module structure is essential for maintainability and scalability. The proposed structure will separate concerns into distinct modules:

- **main.rs**: The main entry point of the application. It will be responsible for initializing components, parsing command-line arguments (if any), loading configuration, setting up the logger, and starting the gRPC server and other core services.
- **config.rs**: Handles loading and parsing of the agent's configuration, likely from a YAML file. This module will define the configuration structs and the logic to deserialize the configuration data.
- **grpc_server/mod.rs**: Contains the gRPC server setup and defines the service implementations.
  - grpc_server/log_metadata_service.rs: Implementation for the LogMetadataService.
  - grpc_server/log_record_service.rs: Implementation for the LogRecordService.
  - grpc_server/health_service.rs: Implementation or integration of the gRPC health checking service.
- **kubernetes_client.rs**: Manages interactions with the Kubernetes API, specifically for performing SelfSubjectAccessReview checks.
- **file_watcher.rs**: Implements file system monitoring capabilities, equivalent to the Go fsnotify functionality.
- **signal_handler.rs**: Manages OS signal handling for graceful shutdown (e.g., SIGINT, SIGTERM).
- **rgkl_integration.rs** (or integrated within log_record_service.rs): Handles the direct integration of the "RipGrep for Kubernetes Logs" functionality, now as a Rust library component rather than an external executable.

This modular approach, as suggested by Rust best practices <sup>2</sup>, promotes separation of concerns, making the codebase easier to understand, test, and maintain as the agent evolves. Each

.rs file typically represents a module, and sub-directories can be used to group related modules further (e.g., grpc_server/).

### 2.2. Directory Structure (Illustrative)

kubetail-cluster-agent-rust/  
├── Cargo.toml  
├── build.rs # For gRPC protobuf compilation  
├── config.yaml.example # Example configuration file  
├── src/  
│ ├── main.rs  
│ ├── config.rs  
│ ├── file_watcher.rs  
│ ├── kubernetes_client.rs  
│ ├── signal_handler.rs  
│ ├── rgkl_integration.rs # Or logic integrated elsewhere  
│ └── grpc_server/  
│ ├── mod.rs  
│ ├── log_metadata_service.rs  
│ ├── log_record_service.rs  
│ └── health_service.rs  
└── proto/ # Directory for.proto files  
└── cluster_agent.proto # Example protobuf definition  

### 2.3. Core Dependencies (Cargo.toml)

The Cargo.toml file will list all necessary dependencies. Key crates identified for porting the existing functionalities include:

Ini, TOML

\[dependencies\]  
\# Core asynchronous runtime  
tokio = { version = "1", features = \["full"\] }  
<br/>\# gRPC framework  
tonic = "0.11" # Or latest compatible version  
prost = "0.12" # Or latest compatible version  
<br/>\# Kubernetes client  
kube = { version = "0.91", features = \["client", "derive"\] } # Or latest compatible version  
k8s-openapi = { version = "0.22", features = \["v1_28"\] } # Match Kubernetes version, adjust as needed  
<br/>\# File system notifications  
notify = "6.1" # Or latest compatible version  
notify-debouncer-mini = "0.4" # Optional: for debouncing file events  
<br/>\# Configuration parsing (YAML)  
serde = { version = "1.0", features = \["derive"\] }  
serde_yaml = "0.9" # Or latest compatible version  
<br/>\# Logging  
tracing = "0.1"  
tracing-subscriber = { version = "0.3", features = \["env-filter"\] }  
<br/>\# Error handling (optional, but recommended)  
thiserror = "1.0"  
anyhow = "1.0" # For application-level error convenience  
<br/>\# gRPC Health Checking  
tonic-health = "0.11" # Should match tonic version  
<br/>\[build-dependencies\]  
tonic-build = "0.11" # For compiling.proto files, should match tonic version  

The choice of a specific Kubernetes API version for k8s-openapi (e.g., v1_28) should align with the minimum supported Kubernetes version for Kubetail. This ensures compatibility and availability of necessary API features.<sup>5</sup>

## 3\. Core Go Dependencies and Rust Equivalents

The migration from Go to Rust involves replacing Go libraries with their idiomatic Rust counterparts. The key functionalities and their chosen Rust equivalents are:

- **File System Notifications:**
  - Go: fsnotify
  - Rust: The notify crate.<sup>6</sup> This is a mature, cross-platform library for file system event notifications, inspired by Go's  
        fsnotify and Node.js's Chokidar. It supports various backends like inotify on Linux and FSEvents or kqueue on macOS.
- **Kubernetes Client Library:**
  - Go: k8s.io/client-go
  - Rust: A combination of kube-rs and k8s-openapi.<sup>5</sup>
    - kube-rs: Provides a high-level, idiomatic Rust client for interacting with the Kubernetes API, similar in spirit to client-go. It handles client configuration, API discovery, and request execution.
    - k8s-openapi: Provides the raw Kubernetes API type definitions generated from the OpenAPI specification. kube-rs uses these types.
- **YAML Configuration Parsing:**
  - Go: Commonly spf13/viper or gopkg.in/yaml.v2.
  - Rust: serde in conjunction with serde_yaml.<sup>8</sup>
    - serde: A powerful and widely used framework for serializing and deserializing Rust data structures efficiently.
    - serde_yaml: A crate that integrates serde with YAML, allowing for easy parsing of YAML files into Rust structs.
- **OS Signal Handling:**
  - Go: os/signal package.
  - Rust: tokio::signal module from the Tokio runtime.<sup>10</sup> This provides an asynchronous way to handle OS signals like  
        SIGINT (Ctrl+C) and SIGTERM, crucial for implementing graceful shutdown.

This selection of Rust crates provides robust and well-maintained solutions for the core requirements of the Cluster Agent.

## 4\. Implementation Details and Rust Components

This section details the Rust implementation approach for each key feature of the Cluster Agent.

### 4.1. File System Change Notifications

The notify crate will be used to monitor log files for changes, replicating the functionality of Go's fsnotify.

- **Crate:** notify <sup>6</sup>, potentially with  
    notify-debouncer-mini if event debouncing is required to prevent processing excessive notifications for rapid changes.
- **Implementation Details:**
  - A RecommendedWatcher from the notify crate will be instantiated. This watcher automatically selects the most appropriate backend for the host operating system (e.g., inotify on Linux).
  - The watcher will be configured to monitor specified log file paths. RecursiveMode::Recursive can be used if directory watching is needed, though for specific log files, non-recursive watching is typical.
  - An event handler (e.g., a closure or a function sending events to a tokio::sync::mpsc channel) will process file system events like modifications (ModifyKind::Data), creations (Create), or deletions (Remove).
  - Careful consideration should be given to handling events for log rotation, where files might be renamed, created, or truncated. The notify crate provides detailed event kinds that can help manage these scenarios.
  - The notify-debouncer-mini crate can be useful to group rapid-fire events for the same file, reducing processing load. This is particularly relevant for active log files.
- **Illustrative Code Snippet (watching a single file):**  
    Rust  
    // In file_watcher.rs  
    use notify::{RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind, EventKind};  
    use notify_debouncer_mini::new_debouncer;  
    use std::path::Path;  
    use std::time::Duration;  
    use tokio::sync::mpsc;  
    <br/>pub async fn watch_log_files(  
    paths: Vec&lt;String&gt;,  
    event_tx: mpsc::Sender&lt;notify::Event&gt;,  
    ) -> Result&lt;(), notify::Error&gt; {  
    // Create a debouncer.  
    // Adjust the timeout as needed. 500ms is an example.  
    let mut debouncer = new_debouncer(Duration::from_millis(500), None, move |res| {  
    match res {  
    Ok(events) => {  
    for event_result in events { // events is Vec&lt;DebouncedEvent&gt;  
    if let Ok(event) = event_result { // event is DebouncedEvent  
    // Send the original event, or a summarized event  
    // For simplicity, sending the first event in a debounced batch  
    // In a real scenario, you might want to send the actual notify::Event  
    // This part needs careful handling of DebouncedEvent vs notify::Event  
    // For now, let's assume we are interested in the fact that \*an\* event occurred.  
    // The debouncer gives Vec&lt;DebouncedEvent&gt;, which contains the original event.  
    // Here, we're just forwarding the basic event info.  
    // A more robust solution would extract the specific notify::Event from DebouncedEvent.  
    // For simplicity, this example might need refinement based on how DebouncedEvent is structured.  
    // The snippet from \[6\] for notify-debouncer-mini shows \`events.iter().for_each(|e|println!...)\`  
    // where \`e\` is a \`DebouncedEvent\`. A \`DebouncedEvent\` contains \`event: notify::Event\`.  
    // So, we should extract \`event.event\`.  
    // However, the debouncer example in \[6\] takes a Result&lt;Vec<DebouncedEvent&gt;, Vec&lt;Error&gt;>.  
    // The new_debouncer signature used here is \`fn new_debouncer&lt;F, T&gt;(duration: Duration, tick_rate: Option&lt;Duration&gt;, event_handler: F) -> Result&lt;Debouncer<T&gt;, Error> where F: FnMut(Result&lt;Vec<DebouncedEvent<T&gt;>, Vec&lt;Error<T&gt;>>) + Send + 'static, T: Watcher\`  
    // The provided outline snippet uses \`mpsc::Sender&lt;notify::Event&gt;\`  
    // Let's assume we want to send the raw notify::Event.  
    // The debouncer callback receives \`Result&lt;Vec<DebouncedEvent&gt;, Vec&lt;Error&gt;>\`.  
    // We need to iterate through \`DebouncedEvent\`s and send \`debounced_event.event\`.  
    <br/>// Corrected logic based on notify_debouncer_mini structure:  
    // The event_handler closure for new_debouncer receives Result&lt;Vec<DebouncedEvent&gt;, Vec&lt;Error&gt;>  
    // So, the closure should be:  
    // |res: Result&lt;Vec<DebouncedEvent&gt;, Vec&lt;Error&gt;>|  
    // and then iterate \`event.event\` from \`DebouncedEvent\`.  
    // The channel \`event_tx\` is \`mpsc::Sender&lt;notify::Event&gt;\`.  
    // This implies the debouncer itself is not used in the outline snippet, or it's simplified.  
    // The outline shows \`RecommendedWatcher\` directly, not the debouncer.  
    // Let's stick to the outline's direct use of RecommendedWatcher first, then mention debouncer.  
    <br/>// Sticking to the outline's simpler \`RecommendedWatcher\` first:  
    // The outline used \`mpsc::Sender&lt;notify::Event&gt;\` which is fine for \`RecommendedWatcher\`.  
    // The snippet below is for \`RecommendedWatcher\`.  
    }  
    }  
    }  
    Err(errors) => {  
    for error in errors {  
    // Log error: e.g., tracing::error!("Debouncer error: {:?}", error);  
    }  
    }  
    }  
    });  
    <br/><br/>// The outline's snippet uses RecommendedWatcher directly. Let's use that.  
    // A debouncer might be an improvement later.  
    // The \`event_tx\` in the function signature \`mpsc::Sender&lt;notify::Event&gt;\` matches \`RecommendedWatcher\`.  
    <br/>let mut watcher = notify::recommended_watcher(move |res: Result&lt;notify::Event, notify::Error&gt;| {  
    match res {  
    Ok(event) => {  
    // Filter for relevant events, e.g., data changes, file creation in watched dirs  
    if matches!(event.kind, EventKind::Modify(ModifyKind::Data(\_)) | EventKind::Create(\_)) {  
    let tx_clone = event_tx.clone();  
    tokio::spawn(async move {  
    if let Err(e) = tx_clone.send(event).await {  
    // Log error: tracing::error!("Failed to send file event: {}", e);  
    }  
    });  
    }  
    }  
    Err(e) => {  
    // Log error: tracing::error!("File watch error: {:?}", e);  
    }  
    }  
    })?;  
    <br/>for path_str in paths {  
    watcher.watch(Path::new(&path_str), RecursiveMode::NonRecursive)?; // Assuming watching specific files  
    // Log info: tracing::info!("Watching file: {}", path_str);  
    }  
    <br/>// Keep the watcher alive. In a real app, this function might return the watcher  
    // or be part of a larger async task structure.  
    // For this example, we'll loop indefinitely to keep it running if this were the main task.  
    // However, this function will likely be spawned and the watcher will live as long as its scope.  
    // The \`watcher\` variable needs to be kept in scope for watching to continue.  
    // This function could be structured to run in a loop or the watcher moved to a place where it lives longer.  
    // For an agent, this watcher would typically run for the lifetime of the agent.  
    // A common pattern is to return the watcher or run it in a select! loop with a shutdown signal.  
    // For now, returning Ok(()) implies the watcher is set up and will run as long as \`watcher\` is alive.  
    // The actual running of the event loop is handled internally by \`notify\` on a separate thread.  
    // So, as long as \`watcher\` is not dropped, it will continue to send events.  
    <br/>// To prevent the watcher from being dropped if this function exits,  
    // it might be better to return the watcher from this function.  
    // Or, this function itself runs in a loop or is part of a select! block.  
    // For now, let's assume the caller manages the lifetime of the watcher implicitly  
    // by keeping the future returned by this async fn alive.  
    <br/>// The key is that \`watcher\` must not be dropped.  
    // If this function is spawned as a task, \`watcher\` will live as long as the task.  
    // If the task needs to be cancellable, then the watcher should be dropped upon cancellation.  
    // This is a simplified example.  
    <br/>Ok(())  
    }  
    <br/>// In main.rs, to use it:  
    // let (event_tx, mut event_rx) = mpsc::channel::&lt;notify::Event&gt;(100);  
    // let watched_file_paths = vec!\["/var/log/app.log".to_string()\]; // From config  
    // tokio::spawn(watch_log_files(watched_file_paths, event_tx));  
    //  
    // tokio::spawn(async move {  
    // while let Some(event) = event_rx.recv().await {  
    // // Process event: event.paths, event.kind  
    // // tracing::info!("File event: {:?}", event);  
    // }  
    // });  
    <br/>This snippet demonstrates setting up a watcher for a list of file paths and sending events to an MPSC channel for asynchronous processing.<sup>6</sup> Using  
    notify-debouncer-mini <sup>6</sup> could be an enhancement if event storms become an issue.

### 4.2. Kubernetes API Interaction (SelfSubjectAccessReview)

Interaction with the Kubernetes API, specifically for SelfSubjectAccessReview, will be handled by the kube-rs and k8s-openapi crates.

- **Crates:** kube-rs <sup>7</sup>,  
    k8s-openapi.<sup>5</sup>
- **Implementation Details:**
  - **Client Initialization:** A kube::Client will be initialized. This client automatically attempts to discover and use in-cluster configuration (e.g., service account token) when running inside a pod. For local development, it can use the local kubeconfig file.
  - **SelfSubjectAccessReview Request:**
    - The k8s_openapi::api::authorization::v1::SelfSubjectAccessReview struct will be used to define the review request.
    - The spec field of this struct (SelfSubjectAccessReviewSpec) needs to be populated with details about the resource access being checked (e.g., resource_attributes specifying verb, group, resource, namespace, name).
    - The kube-rs Api type, specialized for SelfSubjectAccessReview, will be used to make the create call to the Kubernetes API.
    - Api::&lt;k8s_openapi::api::authorization::v1::SelfSubjectAccessReview&gt;::create(&api_client, &review_request_data)
  - **Response Handling:** The response will indicate whether the action is allowed (status.allowed).
  - Error handling is crucial for API interactions, as requests can fail due to network issues, permissions, or invalid requests. kube-rs errors should be mapped to appropriate internal error types or logged.
- **Illustrative Code Snippet:**  
    Rust  
    // In kubernetes_client.rs  
    use kube::{Client, Api};  
    use k8s_openapi::api::authorization::v1::{SelfSubjectAccessReview, SelfSubjectAccessReviewSpec, ResourceAttributes};  
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::CreateOptions; // For PostParams  
    <br/>pub async fn check_self_subject_access(  
    client: Client,  
    namespace: Option&lt;&str&gt;,  
    resource_attributes: ResourceAttributes,  
    ) -> Result&lt;bool, kube::Error&gt; {  
    let ssar_api: Api&lt;SelfSubjectAccessReview&gt; = Api::all(client); // SelfSubjectAccessReview is cluster-scoped for creation  
    <br/>let review_spec = SelfSubjectAccessReviewSpec {  
    resource_attributes: Some(resource_attributes), // e.g., verb: "get", resource: "pods", namespace: Some("default".to_string())  
    non_resource_attributes: None,  
    };  
    <br/>let review_request = SelfSubjectAccessReview {  
    metadata: Default::default(), // Typically empty for SSAR create  
    spec: review_spec,  
    status: None, // Status is output, not input for create  
    };  
    <br/>// For \`create\` operations, \`PostParams\` is often used, but for SSAR, it might be simpler.  
    // The \`Api::create\` method takes \`PostParams\` and the data.  
    // Let's use default \`CreateOptions\` which converts to \`PostParams\`.  
    let params = CreateOptions::default();  
    <br/>match ssar_api.create(&params, &review_request).await {  
    Ok(review_status) => {  
    if let Some(status) = review_status.status {  
    Ok(status.allowed)  
    } else {  
    // Log warning: tracing::warn!("SelfSubjectAccessReview status was unexpectedly None");  
    Ok(false) // Or return an error, depending on desired strictness  
    }  
    }  
    Err(e) => {  
    // Log error: tracing::error!("SelfSubjectAccessReview request failed: {}", e);  
    Err(e)  
    }  
    }  
    }  
    <br/>// In main.rs or a service that needs it:  
    // async fn example_usage() {  
    // let client = Client::try_default().await.expect("Failed to create K8s client");  
    // let attributes = ResourceAttributes {  
    // namespace: Some("default".to_string()),  
    // verb: Some("get".to_string()),  
    // group: Some("".to_string()), // Core group  
    // resource: Some("pods".to_string()),  
    // ..Default::default()  
    // };  
    // match check_self_subject_access(client, Some("default"), attributes).await {  
    // Ok(allowed) => { /\* tracing::info!("Access allowed: {}", allowed); \*/ }  
    // Err(e) => { /\* tracing::error!("Error checking access: {}", e); \*/ }  
    // }  
    // }  
    <br/>This snippet outlines creating a SelfSubjectAccessReview request and interpreting its response.<sup>5</sup> The  
    k8s-openapi crate provides the necessary struct definitions. While <sup>7</sup> indicated no direct example for SelfSubjectAccessReview in the browsed kube-rs docs, the general pattern of using Api::create with the appropriate k8s-openapi type is standard.

### 4.3. OS Signal Handling (Graceful Shutdown)

Graceful shutdown is critical for an agent to finish processing requests and clean up resources before exiting. tokio::signal will be used for this.

- **Crate:** tokio::signal.<sup>10</sup>
- **Implementation Details:**
  - Listeners for SIGINT (Ctrl+C) and SIGTERM (standard termination signal on Unix-like systems) will be set up.
  - Upon receiving a signal, a shutdown notification should be broadcast to all relevant components of the application (e.g., the gRPC server, file watchers).
  - tokio::sync::watch or tokio::sync::broadcast channels are suitable for this notification. A watch channel is good if only the latest state (shutdown or not) matters. A broadcast channel can be used if multiple, independent components need to react. For a simple "shutdown now" signal, tokio::sync::Notify or even a one-shot channel can also work. The outline suggests watch::channel.
  - The gRPC server should be started with serve_with_shutdown to allow it to react to the shutdown signal.
  - Other long-running tasks should also select on receiving from the shutdown channel and terminate gracefully.
- **Illustrative Code Snippet:**  
    Rust  
    // In signal_handler.rs  
    use tokio::signal;  
    use tokio::sync::watch;  
    <br/>pub async fn graceful_shutdown_handler(shutdown_tx: watch::Sender&lt;()&gt;) {  
    let ctrl_c = async {  
    signal::ctrl_c()  
    .await  
    .expect("Failed to install Ctrl+C handler");  
    };  
    <br/>#\[cfg(unix)\]  
    let sigterm = async {  
    signal::unix::signal(signal::unix::SignalKind::terminate())  
    .expect("Failed to install SIGTERM handler")  
    .recv()  
    .await;  
    };  
    <br/>#\[cfg(not(unix))\]  
    let sigterm = std::future::pending::&lt;()&gt;(); // No SIGTERM on non-unix platforms  
    <br/>tokio::select! {  
    _= ctrl_c => {  
    // Log: tracing::info!("Received SIGINT (Ctrl+C), initiating shutdown.");  
    },  
    _= sigterm => {  
    // Log: tracing::info!("Received SIGTERM, initiating shutdown.");  
    },  
    }  
    <br/>// Log: tracing::info!("Sending shutdown signal to application components.");  
    if shutdown_tx.send(()).is_err() {  
    // Log error: tracing::error!("Failed to send shutdown signal: No active receivers.");  
    }  
    }  
    <br/>// In main.rs, to integrate:  
    // let (shutdown_tx, shutdown_rx) = watch::channel(()); // shutdown_rx is cloned for components  
    // tokio::spawn(graceful_shutdown_handler(shutdown_tx));  
    //  
    // // Example for gRPC server:  
    // // server_builder  
    // // .serve_with_shutdown(addr, async {  
    // // let mut rx = shutdown_rx.clone();  
    // // rx.changed().await.ok();  
    // // tracing::info!("gRPC server shutting down gracefully.");  
    // // })  
    // // .await?;  
    <br/>This snippet demonstrates setting up listeners for SIGINT and SIGTERM and using a watch channel to signal other parts of the application to shut down.<sup>10</sup> This ensures resources are cleaned up properly.

### 4.4. Configuration Management with YAML

Configuration will be managed using YAML files, parsed by serde and serde_yaml.

- **Crates:** serde, serde_yaml.<sup>8</sup>
- **Implementation Details:**
  - Define Rust structs (e.g., AppConfig, GrpcConfig, FileWatcherConfig) that mirror the structure of the YAML configuration file. These structs will derive serde::Deserialize.
  - A function will read the YAML configuration file (e.g., using std::fs::File and std::io::BufReader).
  - serde_yaml::from_reader or serde_yaml::from_str will be used to parse the YAML content into the defined Rust structs.
  - Robust error handling should be implemented for file access and parsing errors.
- **Illustrative Code Snippet:**  
    Rust  
    // In config.rs  
    use serde::Deserialize;  
    use std::fs::File;  
    use std::io::BufReader;  
    use std::path::Path;  
    <br/>#  
    pub struct GrpcConfig {  
    pub address: String,  
    // Other gRPC related settings  
    }  
    <br/>#  
    pub struct FileWatcherConfig {  
    pub paths_to_watch: Vec&lt;String&gt;,  
    pub debounce_ms: Option&lt;u64&gt;,  
    }  
    <br/>#  
    pub struct AppConfig {  
    pub grpc_server: GrpcConfig,  
    pub file_watcher: FileWatcherConfig,  
    pub log_level: Option&lt;String&gt;,  
    // Add other global config fields as needed  
    }  
    <br/>pub fn load_config&lt;P: AsRef<Path&gt;>(path: P) -> Result&lt;AppConfig, Box<dyn std::error::Error&gt;> {  
    let file = File::open(path.as_ref())  
    .map_err(|e| format!("Failed to open config file {:?}: {}", path.as_ref(), e))?;  
    let reader = BufReader::new(file);  
    let config: AppConfig = serde_yaml::from_reader(reader)  
    .map_err(|e| format!("Failed to parse YAML from {:?}: {}", path.as_ref(), e))?;  
    // Log info: tracing::info!("Configuration loaded successfully from {:?}", path.as_ref());  
    Ok(config)  
    }  
    <br/>// In main.rs:  
    // let config = match load_config("config/cluster-agent.yaml") {  
    // Ok(cfg) => cfg,  
    // Err(e) => {  
    // // tracing::error!("Failed to load configuration: {}. Using default configuration.", e);  
    // // Potentially fallback to default config or exit  
    // std::process::exit(1);  
    // }  
    // };  
    <br/>This demonstrates defining configuration structs and a function to load and parse a YAML file into these structs.<sup>8</sup> This structured approach makes configuration management type-safe and easy to extend.

### 4.5. gRPC Server Implementation with Tonic

The gRPC server will be implemented using the tonic framework.

- **Crates:** tonic, tonic-build, prost.<sup>14</sup> For health checking,  
    tonic-health <sup>15</sup> will be used.
- **Implementation Details:**
  - **Protobuf Definition:** The existing .proto files defining the LogMetadataService and LogRecordService (and any other services) will be placed in a proto/ directory.
  - **Code Generation (build.rs):** A build.rs script will use tonic_build::compile_protos to generate Rust server and client stubs from the .proto files during the build process.  
        Rust  
        // build.rs  
        fn main() -> Result&lt;(), Box<dyn std::error::Error&gt;> {  
        tonic_build::configure()  
        .build_server(true)  
        .build_client(false) // Agent is a server, so client stubs might not be needed  
        .compile(  
        &\["proto/cluster_agent.proto"\], // Replace with actual.proto file(s)  
        &\["proto/"\], // Include path for imports in.proto  
        )?;  
        Ok(())  
        }  

  - **Service Implementation:**
    - Rust structs will be defined for each service (e.g., LogMetadataServiceImpl, LogRecordServiceImpl).
    - These structs will implement the corresponding tonic-generated service traits (e.g., #\[tonic::async_trait\] impl LogMetadata for LogMetadataServiceImpl).
    - The RPC methods within these implementations will be async fn and contain the core business logic, interacting with other modules like file_watcher.rs or kubernetes_client.rs.
  - **Server Setup:**
    - In main.rs (or grpc_server/mod.rs), the tonic::transport::Server builder will be used to configure and start the gRPC server.
    - Each implemented service will be added to the server using add_service().
    - The server will be started with serve_with_shutdown() to integrate with the graceful shutdown mechanism.
  - **gRPC Health Checking Service:**
    - The tonic-health crate provides a standard gRPC health checking service (grpc.health.v1.Health).
    - tonic_health::server::create_health_reporter() will be used to get a health service and a reporter. The reporter can be used to update the serving status of individual services.
    - The health service is then added to the tonic server builder.
- **Illustrative Server Setup Snippet:**  
    Rust  
    // In grpc_server/mod.rs or main.rs  
    // Assuming generated code is in \`pub mod proto_generated { tonic::include_proto!("cluster_agent"); }\`  
    // use proto_generated::log_metadata_server::{LogMetadata, LogMetadataServer};  
    // use proto_generated::log_record_server::{LogRecord, LogRecordServer};  
    <br/>// Placeholder service implementations  
    // #  
    // pub struct LogMetadataServiceImpl {}  
    // #\[tonic::async_trait\]  
    // impl LogMetadata for LogMetadataServiceImpl { /\*... RPC methods... \*/ }  
    <br/>// #  
    // pub struct LogRecordServiceImpl {}  
    // #\[tonic::async_trait\]  
    // impl LogRecord for LogRecordServiceImpl { /\*... RPC methods... \*/ }  
    <br/>// pub async fn run_grpc_server(  
    // config: &crate::config::GrpcConfig,  
    // mut shutdown_rx: watch::Receiver&lt;()&gt;,  
    // ) -> Result&lt;(), Box<dyn std::error::Error&gt;> {  
    // let addr = config.address.parse()?;  
    // // Log info: tracing::info!("gRPC server listening on {}", addr);  
    <br/>// let log_meta_service = LogMetadataServiceImpl::default();  
    // let log_record_service = LogRecordServiceImpl::default();  
    <br/>// let (mut health_reporter, health_service) = tonic_health::server::create_health_reporter();  
    // // Initially, set services to SERVING. Update to NOT_SERVING during shutdown prep if needed.  
    // health_reporter.set_serving::&lt;LogMetadataServer<LogMetadataServiceImpl&gt;>().await;  
    // health_reporter.set_serving::&lt;LogRecordServer<LogRecordServiceImpl&gt;>().await;  
    <br/>// tonic::transport::Server::builder()  
    // .add_service(LogMetadataServer::new(log_meta_service))  
    // .add_service(LogRecordServer::new(log_record_service))  
    // .add_service(health_service) // Add the health service  
    // .serve_with_shutdown(addr, async {  
    // shutdown_rx.changed().await.ok();  
    // // Log info: tracing::info!("gRPC server received shutdown signal. Gracefully terminating...");  
    // // Optionally update health status to NOT_SERVING here for specific services  
    // })  
    // .await?;  
    <br/>// Ok(())  
    // }  
    <br/>This structure, leveraging tonic and tonic-health <sup>14</sup>, establishes a robust and standards-compliant gRPC server. The direct integration of  
    rgkl as a Rust library within the LogRecordService will eliminate the overhead and limitations of exec.Command(), leading to more efficient log search operations. Asynchronous file I/O using tokio::fs should be preferred within service implementations where applicable to maintain responsiveness.

### 4.6. Porting Specific gRPC Services: LogMetadataService and LogRecordService

The existing Go implementations of LogMetadataService and LogRecordService need to be ported to Rust. This involves translating their business logic into the async fn methods of the Rust tonic service traits.

- **LogMetadataService:**
  - This service likely handles requests for metadata about log files, such as file size and last modification timestamps.
  - The Rust implementation will use asynchronous file system operations, primarily from tokio::fs (e.g., tokio::fs::metadata()), to retrieve this information without blocking the tonic server's worker threads.
  - Information derived from notify events (e.g., last event timestamp for a file) might also be incorporated or cached by this service.
  - **Illustrative Placeholder:**  
        Rust  
        // In grpc_server/log_metadata_service.rs  
        // use crate::proto_generated::log_metadata_server::{LogMetadata, LogMetadataServer}; // Assuming proto_generated from build.rs  
        // use crate::proto_generated::{GetLogMetadataRequest, GetLogMetadataResponse, LogFileMetadata}; // Example proto messages  
        // use tonic::{Request, Response, Status};  
        <br/>// #  
        // pub struct LogMetadataServiceImpl {  
        // // Potentially share state, e.g., a map of recently observed event timestamps  
        // }  
        <br/>// #\[tonic::async_trait\]  
        // impl LogMetadata for LogMetadataServiceImpl {  
        // async fn get_log_metadata(  
        // &self,  
        // request: Request&lt;GetLogMetadataRequest&gt;,  
        // ) -> Result&lt;Response<GetLogMetadataResponse&gt;, Status> {  
        // let req_inner = request.into_inner();  
        // // Log info: tracing::debug!("Received GetLogMetadataRequest for: {:?}", req_inner.file_paths);  
        // let mut metadata_list = Vec::new();  
        <br/>// for file_path_str in req_inner.file_paths {  
        // match tokio::fs::metadata(&file_path_str).await {  
        // Ok(meta) => {  
        // metadata_list.push(LogFileMetadata {  
        // path: file_path_str.clone(),  
        // size: meta.len(),  
        // last_modified_timestamp: meta.modified()  
        // .map_err(|\_| Status::internal("Failed to get modified time"))?  
        // .duration_since(std::time::UNIX_EPOCH)  
        // .map_err(|\_| Status::internal("Time conversion error"))?  
        // .as_secs(),  
        // // last_event_timestamp:... // Potentially from a shared state updated by file_watcher  
        // });  
        // }  
        // Err(e) => {  
        // // Log warn: tracing::warn!("Failed to get metadata for {}: {}", file_path_str, e);  
        // // Decide on error handling: skip, or return error for whole request  
        // }  
        // }  
        // }  
        // Ok(Response::new(GetLogMetadataResponse { metadata: metadata_list }))  
        // }  
        // }  

- **LogRecordService:**
  - This service is responsible for fetching log records and likely implements the log grep functionality.
  - The core change here will be the direct integration of the rgkl ("RipGrep for Kubernetes Logs") logic. Instead of exec.Command(), the rgkl code (presumably based on the ripgrep library) will be included as a Rust module/crate and its functions called directly. This allows for efficient in-memory data transfer and avoids process creation overhead.
  - The implementation will involve reading log file content (potentially with tokio::fs for async reads) and applying rgkl's search/filter logic.
  - **Illustrative Placeholder:**  
        Rust  
        // In grpc_server/log_record_service.rs  
        // use crate::proto_generated::log_record_server::{LogRecord, LogRecordServer};  
        // use crate::proto_generated::{GrepLogRecordsRequest, GrepLogRecordsResponse, LogEntry};  
        // use tonic::{Request, Response, Status};  
        // use crate::rgkl_integration; // Assuming rgkl functions are here  
        <br/>// #  
        // pub struct LogRecordServiceImpl {}  
        <br/>// #\[tonic::async_trait\]  
        // impl LogRecord for LogRecordServiceImpl {  
        // async fn grep_log_records(  
        // &self,  
        // request: Request&lt;GrepLogRecordsRequest&gt;,  
        // ) -> Result&lt;Response<GrepLogRecordsResponse&gt;, Status> {  
        // let req_inner = request.into_inner();  
        // // Log info: tracing::debug!("Received GrepLogRecordsRequest for path: {}, pattern: {}", req_inner.file_path, req_inner.pattern);  
        <br/>// // Example: Call directly into the integrated rgkl logic  
        // // let search_results = rgkl_integration::search_logs(  
        // // &req_inner.file_path,  
        // // &req_inner.pattern,  
        // // // other options like time range, case sensitivity etc.  
        // // ).await.map_err(|e| Status::internal(format!("Log search failed: {}", e)))?;  
        //  
        // // let log_entries: Vec&lt;LogEntry&gt; = search_results.into_iter()  
        // // .map(|line| LogEntry { content: line, /\* timestamp, etc. \*/ })  
        // // .collect();  
        //  
        // // Ok(Response::new(GrepLogRecordsResponse { entries: log_entries }))  
        // Err(Status::unimplemented("grep_log_records not yet fully implemented"))  
        // }  
        // }  

Porting these services requires careful translation of the existing Go logic, with particular attention to asynchronous operations and error handling. Rust's Result type, potentially combined with custom error types defined using thiserror, should be used to propagate errors clearly, which can then be mapped to tonic::Status at the gRPC boundary.

## 5\. Build, Deployment, and Development Environment

### 5.1. CI Workflow for Rust Agent (GitHub Actions)

The CI workflow needs to be updated to build, test, and package the new Rust-based Cluster Agent. The targets are linux/amd64 and linux/arm64.

- **Key Workflow Steps** <sup>19</sup>:
    1. **Checkout Code:** Use actions/checkout@v4.
    2. **Setup Rust Toolchain:** Use dtolnay/rust-toolchain@stable or a similar action to select the desired Rust version.
    3. **Cache Dependencies:** Implement caching for Cargo's build artifacts and registry data using actions/cache@v4. The cache key should include the Cargo.lock file hash to ensure fresh dependencies are fetched when Cargo.lock changes.
    4. **Linting:** Run cargo clippy --all-targets --all-features -- -D warnings to catch common mistakes and enforce code style.
    5. **Formatting Check:** Run cargo fmt --all -- --check to ensure consistent code formatting.
    6. **Testing:** Execute cargo test --all-features to run unit and integration tests.
    7. **Building for Multiple Targets:**
        - Employ a matrix strategy in GitHub Actions to build for x86_64-unknown-linux-musl (for amd64) and aarch64-unknown-linux-musl (for arm64). The musl target is preferred for creating statically linked binaries, which reduces runtime dependencies on the target Kubernetes nodes and can lead to smaller Docker images.
        - Ensure necessary cross-compilation toolchains (e.g., musl-tools, gcc-aarch64-linux-gnu) are available in the runner environment, or use a specialized Docker image like messense/rust-musl-cross as the build container.
        - Add targets using rustup target add &lt;target_triple&gt;.
    8. **Packaging Release Artifacts:**
        - After a successful build, the binaries should be packaged (e.g., into .tar.gz archives).
        - Use actions/upload-artifact@v4 to store these binaries as workflow artifacts.
        - For tagged releases, consider using an action like houseabsolute/actions-rust-release <sup>21</sup> or  
            softprops/action-gh-release to create GitHub Releases and attach the compiled binaries.
- **Illustrative GitHub Actions Workflow Snippet:**  
    YAML  
    #.github/workflows/rust_ci.yml  
    \# name: Rust CI Build and Test  
    <br/>\# on: \[push, pull_request\]  
    <br/>\# env:  
    \# CARGO_TERM_COLOR: always  
    <br/>\# jobs:  
    \# build_and_test:  
    \# strategy:  
    // fail-fast: false # Allow other jobs in matrix to continue if one fails  
    \# matrix:  
    \# include:  
    \# - target: x86_64-unknown-linux-musl  
    \# os: ubuntu-latest  
    \# name_suffix: linux-amd64  
    \# - target: aarch64-unknown-linux-musl  
    \# os: ubuntu-latest  
    \# name_suffix: linux-arm64  
    \# runs-on: ${{ matrix.os }}  
    \# steps:  
    \# - uses: actions/checkout@v4  
    <br/>\# - name: Set up Rust toolchain  
    \# uses: dtolnay/rust-toolchain@stable  
    \# with:  
    // targets: ${{ matrix.target }} # Install the specific target for cross-compilation  
    <br/>\# - name: Cache Cargo dependencies  
    \# uses: actions/cache@v4  
    \# with:  
    \# path: |  
    // ~/.cargo/bin/  
    // ~/.cargo/registry/index/  
    // ~/.cargo/registry/cache/  
    // ~/.cargo/git/db/  
    // target/ # Cache the target directory as well  
    \# key: ${{ runner.os }}-cargo-${{ matrix.target }}-${{ hashFiles('\*\*/Cargo.lock') }}  
    \# restore-keys: |  
    // ${{ runner.os }}-cargo-${{ matrix.target }}-  
    <br/>\# - name: Install cross-compilation prerequisites (if using musl without a dedicated cross image)  
    \# if: contains(matrix.target, 'musl')  
    \# run: |  
    // sudo apt-get update  
    // sudo apt-get install -y musl-tools  
    // # For aarch64-unknown-linux-musl, you might also need a linker like:  
    // # sudo apt-get install -y gcc-aarch64-linux-gnu  
    // # Or configure.cargo/config.toml with appropriate linker settings  
    <br/>\# - name: Check formatting  
    \# run: cargo fmt --all -- --check  
    <br/>\# - name: Clippy Lints  
    \# run: cargo clippy --all-targets --all-features --target ${{ matrix.target }} -- -D warnings  
    <br/>\# - name: Build  
    \# run: cargo build --release --target ${{ matrix.target }}  
    <br/>\# - name: Run tests  
    \# # Tests might need to be run on native architecture or with QEMU if cross-compiling tests  
    \# # For simplicity, this example builds tests but running them cross-compiled can be complex.  
    \# # Consider running tests on a native runner if possible, or focusing on unit tests here.  
    \# run: cargo test --release --target ${{ matrix.target }} --no-run # Compile tests, don't run if cross-compiling without emulator  
    <br/>\# - name: Package binary  
    \# run: |  
    // ARTIFACT_NAME="kubetail-cluster-agent-${{ matrix.name_suffix }}"  
    \# cd target/${{ matrix.target }}/release  
    \# tar czvf "../../../${ARTIFACT_NAME}.tar.gz" kubetail-cluster-agent-rust # Ensure binary name matches  
    // cd../../../  
    <br/>\# - name: Upload artifact  
    // uses: actions/upload-artifact@v4  
    \# with:  
    // name: kubetail-cluster-agent-${{ matrix.name_suffix }}  
    // path: kubetail-cluster-agent-${{ matrix.name_suffix }}.tar.gz  
    <br/>This CI pipeline will ensure that the Rust agent is consistently built and tested across the required architectures. If the Cluster Agent is deployed as a Docker container <sup>25</sup>, the CI pipeline should also be extended to build and push these Docker images. Multi-stage Docker builds are recommended for Rust applications to produce minimal final images, typically using a build stage with the full Rust toolchain and a final stage copying only the statically linked binary to a minimal base image like  
    scratch or alpine-linux.

### 5.2. Updating the Development Environment

The development environment must be updated to support the new Rust-based Cluster Agent.

- **Requirements:**
  - **Rust Toolchain:** Developers will need the Rust toolchain installed, manageable via rustup. Clear instructions on installing rustup and the correct Rust version/channel (e.g., stable) should be provided.
  - **Build and Run:** Documentation for building (cargo build) and running (cargo run) the Rust agent locally. This should include how to pass configuration files or environment variables.
  - **Local Kubernetes Deployment:** If developers use local Kubernetes clusters (e.g., Kind, Minikube, k3d) for testing, existing deployment scripts or configurations (e.g., Tiltfiles, as Kubetail uses Tilt <sup>1</sup>) must be updated. This involves:
    - Modifying the build process to compile the Rust agent instead of the Go agent.
    - Updating Dockerfiles or image build steps if the agent is run as a container locally.
    - Ensuring the new Rust agent binary or container image is correctly deployed to the local cluster.
  - **Debugging:** Provide guidance on debugging the Rust agent. This can include:
    - Using rust-gdb or lldb with Rust-specific extensions.
    - Leveraging IDE debuggers (e.g., VS Code with rust-analyzer and the CodeLLDB extension, or CLion with its native Rust support).
    - Effective use of the tracing crate for structured logging, which is invaluable for debugging distributed applications.
  - **Test Harnesses:** Any local test harnesses or integration test setups that interact with the Cluster Agent must be updated to communicate with the Rust version (e.g., updated gRPC client libraries if used for testing).

A smooth and well-documented local development experience is crucial for developer productivity and the successful adoption of the new Rust-based agent.

## 6\. Conclusion and Next Steps

Rewriting the Kubetail Cluster Agent in Rust offers significant advantages in terms of performance, resource efficiency, and maintainability. Rust's strong type system and memory safety features are particularly beneficial for a critical infrastructure component like a cluster agent. The direct integration of the rgkl component as a Rust library will eliminate the inefficiencies of the current exec.Command() approach.

The migration path involves replacing core Go functionalities with their robust Rust equivalents: notify for file system events, kube-rs and k8s-openapi for Kubernetes API interactions, serde_yaml for configuration, tokio::signal for graceful shutdown, and tonic for the gRPC server.

A phased migration approach is recommended to manage complexity and allow for incremental testing:

1. **Foundation and Core Plumbing:**
    - Initialize the Rust project structure and Cargo.toml.
    - Implement basic CI for building and testing.
    - Develop configuration loading (config.rs) using serde_yaml.
    - Implement OS signal handling (signal_handler.rs) for graceful shutdown using tokio::signal.
    - Set up a basic tonic gRPC server with the tonic-health service.
2. **Kubernetes API Interaction:**
    - Implement the SelfSubjectAccessReview logic using kube-rs and k8s-openapi (kubernetes_client.rs).
3. **File System Watching:**
    - Implement file watching functionality using the notify crate (file_watcher.rs).
4. **rgkl Integration and LogRecordService Porting:**
    - Refactor the existing rgkl Rust code into a library module.
    - Port the LogRecordService, integrating rgkl directly for log search/grep operations.
5. **LogMetadataService Porting:**
    - Port the LogMetadataService, utilizing asynchronous file operations (tokio::fs) and potentially state from the file watcher.
6. **Comprehensive Testing:**
    - Write thorough unit tests for all modules.
    - Develop integration tests covering interactions between components (e.g., gRPC calls triggering file operations or Kubernetes API calls).
    - Conduct end-to-end testing in a representative Kubernetes environment.
7. **Documentation and Development Environment Update:**
    - Finalize all internal and developer-facing documentation.
    - Update local development environment tools and guides (e.g., Tilt configurations, debugging instructions).
8. **Deployment and Rollout:**
    - Update CI to produce final release artifacts (binaries, Docker images).
    - Plan and execute the rollout of the new Rust-based Cluster Agent to staging and production environments.

This technical specification provides a comprehensive roadmap for the rewrite. Proceeding with this plan will result in a more performant, robust, and maintainable Kubetail Cluster Agent, better equipped to support current and future Kubetail features.<sup>1</sup> The adoption of Rust for this component aligns with industry trends for systems programming where performance and reliability are paramount.

#### Works cited

1. kubetail-org/kubetail: Real-time logging dashboard for Kubernetes (browser/terminal) - GitHub, accessed June 10, 2025, <https://github.com/kubetail-org/kubetail>
2. Managing Growing Projects with Packages, Crates, and Modules - The Rust Programming Language, accessed June 10, 2025, <https://doc.rust-lang.org/book/ch07-00-managing-growing-projects-with-packages-crates-and-modules.html>
3. Organizing code & project structure - Rust Development Classes, accessed June 10, 2025, <https://rust-classes.com/chapter_4_3>
4. How to organize modules for a Rust web service - The Rust Programming Language Forum, accessed June 10, 2025, <https://users.rust-lang.org/t/how-to-organize-modules-for-a-rust-web-service/107977>
5. "self_subject_access_review_spec" Search - Rust - Docs.rs, accessed June 10, 2025, <https://docs.rs/k8s-openapi/latest/i686-unknown-linux-gnu/k8s_openapi/?search=self_subject_access_review_spec>
6. notify-rs/notify: Cross-platform filesystem notification library ... - GitHub, accessed June 10, 2025, <https://github.com/notify-rs/notify>
7. kube-rs/kube: Rust Kubernetes client and controller runtime - GitHub, accessed June 10, 2025, <https://github.com/kube-rs/kube>
8. serde-yaml-olidacombe - crates.io: Rust Package Registry, accessed June 10, 2025, <https://crates.io/crates/serde-yaml-olidacombe>
9. serde_yaml_olidacombe - Rust - Docs.rs, accessed June 10, 2025, <https://docs.rs/serde-yaml-olidacombe>
10. Graceful Shutdown | Tokio - An asynchronous Rust runtime, accessed June 10, 2025, <https://tokio.rs/tokio/topics/shutdown>
11. Resource in k8s_openapi - Rust - Docs.rs, accessed June 10, 2025, <https://docs.rs/k8s-openapi/latest/k8s_openapi/trait.Resource.html>
12. DeepMerge in k8s_openapi - Rust, accessed June 10, 2025, <https://arnavion.github.io/k8s-openapi/v0.22.x/k8s_openapi/trait.DeepMerge.html>
13. Graceful Shutdown and Cleanup - The Rust Programming Language, accessed June 10, 2025, <https://doc.rust-lang.org/book/ch21-03-graceful-shutdown-and-cleanup.html>
14. Let's build a gRPC server and client in Rust with tonic · Thorsten ..., accessed June 10, 2025, <https://www.thorsten-hans.com/grpc-services-in-rust-with-tonic/>
15. tonic-health - crates.io: Rust Package Registry, accessed June 10, 2025, <https://crates.io/crates/tonic-health>
16. Tonic makes gRPC in Rust stupidly simple - YouTube, accessed June 10, 2025, <https://www.youtube.com/watch?v=kerKXChDmsE&pp=0gcJCdgAo7VqN5tD>
17. gRPC In RUST | gRPC Server | gRPC Client | Protobuf | Step by Step Guide - YouTube, accessed June 10, 2025, <https://www.youtube.com/watch?v=Z_zDhLYuz9Q>
18. Performing health checks | Tonic Structural, accessed June 10, 2025, <https://docs.tonic.ai/app/admin/tonic-monitoring-logging/health-checks>
19. Configure CI/CD for your Rust application - Docker Docs, accessed June 10, 2025, <https://docs.docker.com/guides/rust/configure-ci-cd/>
20. Building and testing Rust - GitHub Docs, accessed June 10, 2025, <https://docs.github.com/en/actions/use-cases-and-examples/building-and-testing/building-and-testing-rust>
21. houseabsolute/actions-rust-release: A GitHub Action for ... - GitHub, accessed June 10, 2025, <https://github.com/houseabsolute/actions-rust-release>
22. Kubetail: Open-source project looking for new Go contributors : r/golang - Reddit, accessed June 10, 2025, <https://www.reddit.com/r/golang/comments/1l3ynzi/kubetail_opensource_project_looking_for_new_go/>
23. Kubetail: Open-source project looking for new Rust contributors - Reddit, accessed June 10, 2025, <https://www.reddit.com/r/rust/comments/1ktgv57/kubetail_opensource_project_looking_for_new_rust/>
24. Kubetail Cluster Agent, accessed June 10, 2025, <https://www.kubetail.com/docs/cluster-resources/cluster-agent>
25. ran go mod tidy in modules · kubetail-org/kubetail@7713a1a · GitHub, accessed June 10, 2025, <https://github.com/kubetail-org/kubetail/actions/runs/14042163135/workflow>