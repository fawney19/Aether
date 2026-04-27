use super::types::{
    SchedulerRankableCandidate, SchedulerRankingMode, SchedulerTunnelAffinityBucket,
};

pub const RANKING_REASON_CACHED_AFFINITY: &str = "cached_affinity";
pub const RANKING_REASON_LOCAL_TUNNEL: &str = "local_tunnel";
pub const RANKING_REASON_CROSS_FORMAT: &str = "cross_format";

pub fn promoted_by(
    candidate: &SchedulerRankableCandidate,
    ranking_mode: SchedulerRankingMode,
) -> Option<&'static str> {
    if ranking_mode == SchedulerRankingMode::CacheAffinity && candidate.cached_affinity_match {
        return Some(RANKING_REASON_CACHED_AFFINITY);
    }
    if ranking_mode == SchedulerRankingMode::CacheAffinity
        && candidate.tunnel_bucket == SchedulerTunnelAffinityBucket::LocalTunnel
    {
        return Some(RANKING_REASON_LOCAL_TUNNEL);
    }
    None
}

pub fn demoted_by(candidate: &SchedulerRankableCandidate) -> Option<&'static str> {
    candidate
        .demote_cross_format
        .then_some(RANKING_REASON_CROSS_FORMAT)
}
