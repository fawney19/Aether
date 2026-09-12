use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aether_crypto::{
    decrypt_python_fernet_ciphertext, encrypt_python_fernet_plaintext, DEVELOPMENT_ENCRYPTION_KEY,
};
use aether_data::repository::auth::{
    InMemoryAuthApiKeySnapshotRepository, StoredAuthApiKeySnapshot,
};
use aether_data::repository::auth_modules::{
    InMemoryAuthModuleReadRepository, StoredLdapModuleConfig, StoredOAuthProviderModuleConfig,
};
use aether_data::repository::management_tokens::{
    InMemoryManagementTokenRepository, ManagementTokenReadRepository, StoredManagementToken,
    StoredManagementTokenUserSummary, StoredManagementTokenWithUser,
};
use aether_data::repository::oauth_providers::{
    InMemoryOAuthProviderRepository, OAuthProviderReadRepository, StoredOAuthProviderConfig,
};
use aether_data::repository::provider_catalog::InMemoryProviderCatalogReadRepository;
use aether_data::repository::proxy_nodes::{
    InMemoryProxyNodeRepository, ProxyNodeReadRepository, StoredProxyNode, StoredProxyNodeEvent,
};
use aether_data::repository::quota::InMemoryProviderQuotaRepository;
use aether_data::repository::users::InMemoryUserReadRepository;
use aether_data::repository::wallet::InMemoryWalletRepository;
use aether_data_contracts::repository::{
    candidates::{RequestCandidateStatus, StoredRequestCandidate},
    global_models::{
        GlobalModelReadRepository, StoredAdminGlobalModel, StoredAdminProviderModel,
        StoredProviderActiveGlobalModel, StoredProviderModelStats, StoredPublicGlobalModel,
    },
    provider_catalog::{
        ProviderCatalogReadRepository, StoredProviderCatalogEndpoint, StoredProviderCatalogKey,
        StoredProviderCatalogProvider,
    },
    quota::StoredProviderQuotaSnapshot,
};
use axum::body::{to_bytes, Body, Bytes};
use axum::response::Response;
use axum::routing::{any, post};
use axum::{extract::Request, Json, Router};
use http::header::{HeaderName, HeaderValue};
use http::{HeaderMap, StatusCode};
use serde_json::json;

mod admin;
mod helpers;
mod internal;
mod proxy;

pub(super) use helpers::issue_test_admin_access_token as issue_shared_test_admin_access_token;

use super::{
    build_router, build_router_with_execution_runtime_override, build_router_with_state,
    build_state_with_execution_runtime_override, start_server, wait_until, AppState,
    FrontdoorCorsConfig, FrontdoorUserRpmConfig, GatewayFallbackMetricKind, GatewayFallbackReason,
    Infallible, UsageRuntimeConfig, VideoTaskTruthSourceMode,
};
use crate::constants::*;
use crate::data::GatewayDataState;
use helpers::*;
