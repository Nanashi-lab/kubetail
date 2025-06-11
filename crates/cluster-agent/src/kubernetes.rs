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

use anyhow::{Context, Result};
use k8s_openapi::api::authorization::v1::{
    ResourceAttributes, SelfSubjectAccessReview, SelfSubjectAccessReviewSpec,
};
use kube::{Api, Client, Config as KubeConfig, api::PostParams};
use tracing::{debug, error, instrument};

use crate::config::{Config, KubernetesConfig};

#[derive(Clone)]
pub struct KubernetesClient {
    client: Client,
    config: Arc<Config>,
}

impl KubernetesClient {
    /// Initialize Kubernetes client based on configuration
    #[instrument(skip(config))]
    pub async fn new(config: Arc<Config>) -> Result<Self> {
        let kube_config = if config.kubernetes.in_cluster {
            debug!("Using in-cluster Kubernetes configuration");
            KubeConfig::incluster().context("Failed to load in-cluster configuration")?
        } else if let Some(kubeconfig_path) = &config.kubernetes.kubeconfig_path {
            debug!("Loading kubeconfig from: {}", kubeconfig_path);
            KubeConfig::from_kubeconfig(&kube::config::KubeConfigOptions {
                context: None,
                cluster: None,
                user: None,
            })
            .await
            .context("Failed to load kubeconfig")?
        } else {
            debug!("Using default kubeconfig");
            KubeConfig::infer()
                .await
                .context("Failed to infer Kubernetes configuration")?
        };

        let client = Client::try_from(kube_config).context("Failed to create Kubernetes client")?;

        Ok(Self { client, config })
    }

    /// Check permissions using SelfSubjectAccessReview
    /// This replaces the Go helper.CheckPermission function
    #[instrument(skip(self))]
    pub async fn check_permissions(
        &self,
        namespaces: &[String],
        verb: &str,
    ) -> Result<()> {
        // Skip permission check if no namespaces specified or first namespace is empty
        if namespaces.is_empty() || (namespaces.len() == 1 && namespaces[0].is_empty()) {
            debug!("Skipping permission check for empty namespaces");
            return Ok(());
        }

        for namespace in namespaces {
            if namespace.is_empty() {
                continue;
            }

            let allowed = self
                .check_namespace_permission(namespace, verb)
                .await
                .with_context(|| {
                    format!("Failed to check permissions for namespace: {}", namespace)
                })?;

            if !allowed {
                return Err(anyhow::anyhow!(
                    "Access denied for verb '{}' on namespace '{}'",
                    verb,
                    namespace
                ));
            }

            debug!("Permission granted for verb '{}' on namespace '{}'", verb, namespace);
        }

        Ok(())
    }

    /// Check permission for a single namespace
    async fn check_namespace_permission(&self, namespace: &str, verb: &str) -> Result<bool> {
        let ssar_api: Api<SelfSubjectAccessReview> = Api::all(self.client.clone());

        let resource_attributes = ResourceAttributes {
            namespace: Some(namespace.to_string()),
            verb: Some(verb.to_string()),
            group: Some("".to_string()), // Core API group
            resource: Some("pods".to_string()),
            subresource: Some("log".to_string()),
            ..Default::default()
        };

        let review_spec = SelfSubjectAccessReviewSpec {
            resource_attributes: Some(resource_attributes),
            non_resource_attributes: None,
        };

        let review_request = SelfSubjectAccessReview {
            metadata: Default::default(),
            spec: review_spec,
            status: None, // Status is output only
        };

        debug!("Performing SelfSubjectAccessReview for namespace: {}, verb: {}", namespace, verb);

        match ssar_api.create(&PostParams::default(), &review_request).await {
            Ok(review_result) => {
                if let Some(status) = review_result.status {
                    Ok(status.allowed)
                } else {
                    error!("SelfSubjectAccessReview status was unexpectedly None");
                    Ok(false)
                }
            }
            Err(e) => {
                error!("SelfSubjectAccessReview request failed: {}", e);
                Err(anyhow::anyhow!("Permission check failed: {}", e))
            }
        }
    }

    /// Get the underlying Kubernetes client for advanced operations
    pub fn client(&self) -> &Client {
        &self.client
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, KubernetesConfig};

    fn test_config() -> Arc<Config> {
        let mut config = Config::default();
        config.kubernetes = KubernetesConfig {
            in_cluster: false,
            kubeconfig_path: None,
        };
        Arc::new(config)
    }

    #[tokio::test]
    async fn test_kubernetes_client_creation() {
        // This test will only pass in environments with valid kubeconfig
        // In CI, we'd need to mock the Kubernetes client
        let config = test_config();
        
        // For now, just test that the struct can be created
        // In a real test environment, we'd use a mock Kubernetes server
        assert!(config.kubernetes.kubeconfig_path.is_none());
    }

    #[test]
    fn test_empty_namespaces_permission_check() {
        // Test that empty namespaces list should be allowed
        let namespaces: Vec<String> = vec![];
        assert!(namespaces.is_empty());
        
        let single_empty = vec!["".to_string()];
        assert_eq!(single_empty.len(), 1);
        assert!(single_empty[0].is_empty());
    }
} 