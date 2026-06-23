ALTER TABLE api_keys
    ADD COLUMN billing_multiplier DOUBLE NOT NULL DEFAULT 1.0 AFTER total_cost_usd;
