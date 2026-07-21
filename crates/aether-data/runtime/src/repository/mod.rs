//! Runtime repository facade and in-memory implementations.
//!
//! Repository contracts and shared DTOs are re-exported from
//! `aether-data-contracts`. Selected database implementations are re-exported
//! from the adapter crates to preserve existing import paths; concrete SQL does
//! not belong in this facade.

pub mod announcements;
pub mod audit;
pub mod auth;
pub mod auth_modules;
pub mod background_tasks;
pub mod billing;
pub mod candidate_selection;
pub mod candidates;
pub mod gemini_file_mappings;
pub mod global_models;
pub mod management_tokens;
pub mod oauth_providers;
pub mod pool_scores;
pub mod provider_catalog;
pub mod provider_oauth;
pub mod proxy_nodes;
pub mod quota;
pub mod routing_profiles;
pub mod settlement;
pub mod system;
pub mod usage;
pub mod users;
pub mod video_tasks;
pub mod wallet;
