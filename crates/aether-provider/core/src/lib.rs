//! Provider domain contracts independent from transport and persistence.

mod capability;
mod catalog;
mod health;
mod policy;

pub use capability::ProviderCapability;
pub use catalog::{ProviderConfigError, ProviderDescriptor};
pub use health::ProviderHealth;
pub use policy::ProviderEligibilityPolicy;
