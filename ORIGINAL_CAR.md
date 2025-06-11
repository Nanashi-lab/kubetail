Overview

The Kubetail Cluster Agent is a small Go-based gRPC service that's designed to execute low-level file operations on Kubernetes cluster nodes on behalf of the Kubetail Cluster API. Currently, the agent has a limited feature set (reads log file sizes, last event timestamps, performs log grep) but we plan on adding more features in the future.

Originally, the Cluster Agent was written purely in Go but to add search we introduced a custom Rust component called "RipGrep for Kubernetes Logs" (rgkl). The reason we chose Rust for this feature was to take advantage of the ripgrep library which is extremely fast and very robust. Currently, the way we call rgkl from the Cluster Agent Go code is using exec.Command() which works but isn't ideal because we can only communiate with it through stdout/stderr.

Long-term, our goal is for the Cluster Agent to be the tool that executes all the low-level file operations we need to implement advanced features for Kubetail like log parsing, metrics statistics and alerts. Mixing Go and Rust using exec.Command() as an interface isn't ideal so we'd like to choose either one language or the other for the entire tool. For our use-case Rust is appealing because it's very fast and we can use it to make a lightweight tool that won't use much node resources.

The purpose of this tech spec is to help us think through the pros/cons of re-writing the Cluster Agent in Rust and scope out what a project to do so would look like.
Goals

Implement a gRPC server in Rust
Implement the LogMetadataService in Rust
Implement the LogRecordService in Rust
Update dev environment to use new Rust-based Cluster Agent

    Update CI workflow to build new Rust-based Cluster Agent

Non-Goals

    No new features

Design Overview

Some of the key features that will have to be ported from Go to Rust are:

    The use of fsnotify to subscribe to file changes in linux amd64/arm64
    The use of the Kubernetes client library to execute SelfSubjectAccessReviews
    An OS signal listener to capture SIGINT and SIGTERM signals and implement graceful shutdown
    A config parser that supports at least yaml
    A gRPC health service implementation

Alternatives Considered

No response
Risks and Mitigations

No response
Open Questions

No response
Future Work

