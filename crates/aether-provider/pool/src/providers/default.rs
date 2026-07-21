use crate::provider::ProviderPoolAdapter;

#[derive(Debug, Clone, Default)]
pub struct DefaultProviderPoolAdapter;

impl ProviderPoolAdapter for DefaultProviderPoolAdapter {
    fn provider_type(&self) -> &'static str {
        "default"
    }
}
