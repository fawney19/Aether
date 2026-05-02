use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

pub struct OAuthAdapterRegistry<T: ?Sized> {
    adapters: BTreeMap<String, Arc<T>>,
}

impl<T: ?Sized> fmt::Debug for OAuthAdapterRegistry<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthAdapterRegistry")
            .field("provider_types", &self.adapters.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl<T: ?Sized> Clone for OAuthAdapterRegistry<T> {
    fn clone(&self) -> Self {
        Self {
            adapters: self.adapters.clone(),
        }
    }
}

impl<T: ?Sized> Default for OAuthAdapterRegistry<T> {
    fn default() -> Self {
        Self {
            adapters: BTreeMap::new(),
        }
    }
}

impl<T: ?Sized> OAuthAdapterRegistry<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, provider_type: &str, adapter: Arc<T>) {
        let key = provider_type.trim().to_ascii_lowercase();
        if !key.is_empty() {
            self.adapters.insert(key, adapter);
        }
    }

    pub fn get(&self, provider_type: &str) -> Option<Arc<T>> {
        self.adapters
            .get(provider_type.trim().to_ascii_lowercase().as_str())
            .cloned()
    }

    pub fn provider_types(&self) -> impl Iterator<Item = &str> {
        self.adapters.keys().map(String::as_str)
    }
}
