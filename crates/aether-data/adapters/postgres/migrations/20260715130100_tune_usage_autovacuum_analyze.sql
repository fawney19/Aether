-- These two append-heavy tables otherwise wait for roughly ten percent of
-- their multi-million-row population to change before refreshing planner
-- statistics, which leaves dashboard range estimates badly stale after bulk
-- imports.
ALTER TABLE usage SET (
  autovacuum_analyze_scale_factor = 0.02,
  autovacuum_analyze_threshold = 10000
);

ALTER TABLE usage_settlement_snapshots SET (
  autovacuum_analyze_scale_factor = 0.02,
  autovacuum_analyze_threshold = 10000
);
