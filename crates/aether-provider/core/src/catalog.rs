use serde::{Deserialize, Serialize};

use crate::{ProviderCapability, ProviderHealth};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub provider_id: String,
    pub display_name: String,
    pub capabilities: Vec<ProviderCapability>,
    pub health: ProviderHealth,
}

impl ProviderDescriptor {
    pub fn supports(&self, capability: ProviderCapability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn validate(&self) -> Result<(), ProviderConfigError> {
        if self.provider_id.trim().is_empty() {
            return Err(ProviderConfigError::MissingProviderId);
        }
        if self.display_name.trim().is_empty() {
            return Err(ProviderConfigError::MissingDisplayName);
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProviderConfigError {
    #[error("provider id cannot be empty")]
    MissingProviderId,
    #[error("provider display name cannot be empty")]
    MissingDisplayName,
}

#[cfg(test)]
mod tests {
    use super::ProviderDescriptor;
    use crate::{ProviderCapability, ProviderHealth};

    #[test]
    fn descriptor_exposes_capability_without_transport_dependencies() {
        let descriptor = ProviderDescriptor {
            provider_id: "openai".to_string(),
            display_name: "OpenAI".to_string(),
            capabilities: vec![ProviderCapability::Chat],
            health: ProviderHealth::Healthy,
        };
        assert!(descriptor.validate().is_ok());
        assert!(descriptor.supports(ProviderCapability::Chat));
    }
}
