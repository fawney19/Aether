-- Name: idx_apikey_provider_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_apikey_provider_enabled ON public.api_key_provider_mappings USING btree (api_key_id, is_enabled);



--
-- Name: idx_endpoint_format_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_endpoint_format_active ON public.provider_endpoints USING btree (api_format, is_active);



--
-- Name: idx_provider_endpoints_provider_api_format; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_endpoints_provider_api_format ON public.provider_endpoints USING btree (provider_id, api_format);



--
-- Name: idx_gemini_file_mappings_expires; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_gemini_file_mappings_expires ON public.gemini_file_mappings USING btree (expires_at);



--
-- Name: idx_gemini_file_mappings_source_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_gemini_file_mappings_source_hash ON public.gemini_file_mappings USING btree (source_hash);



--
-- Name: idx_management_tokens_is_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_management_tokens_is_active ON public.management_tokens USING btree (is_active);



--
-- Name: idx_management_tokens_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_management_tokens_user_id ON public.management_tokens USING btree (user_id);



--
-- Name: idx_model_provider_model_aliases_gin; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_model_provider_model_aliases_gin ON public.models USING gin (provider_model_mappings jsonb_path_ops) WHERE (is_active = true);



--
-- Name: idx_model_provider_model_name; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_model_provider_model_name ON public.models USING btree (provider_model_name) WHERE (is_active = true);



--
-- Name: idx_payment_callbacks_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_callbacks_created ON public.payment_callbacks USING btree (created_at);



--
-- Name: idx_payment_callbacks_gateway_order; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_callbacks_gateway_order ON public.payment_callbacks USING btree (gateway_order_id);



--
-- Name: idx_payment_callbacks_order; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_callbacks_order ON public.payment_callbacks USING btree (order_no);



--
-- Name: idx_payment_orders_gateway_order_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_gateway_order_id ON public.payment_orders USING btree (gateway_order_id);



--
-- Name: idx_payment_orders_kind_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_kind_status ON public.payment_orders USING btree (order_kind, status);



--
-- Name: idx_payment_orders_product; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_product ON public.payment_orders USING btree (product_id);



--
-- Name: idx_payment_orders_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_status ON public.payment_orders USING btree (status);



--
-- Name: idx_payment_orders_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_user_created ON public.payment_orders USING btree (user_id, created_at);



--
-- Name: idx_payment_orders_wallet_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_payment_orders_wallet_created ON public.payment_orders USING btree (wallet_id, created_at);



--
-- Name: idx_billing_plans_enabled_sort; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_billing_plans_enabled_sort ON public.billing_plans USING btree (enabled, sort_order);



--
-- Name: idx_user_plan_entitlements_user_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_plan_entitlements_user_active ON public.user_plan_entitlements USING btree (user_id, status, expires_at);



--
-- Name: idx_user_plan_entitlements_order; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_plan_entitlements_order ON public.user_plan_entitlements USING btree (payment_order_id);



--
-- Name: idx_entitlement_usage_user_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_entitlement_usage_user_date ON public.entitlement_usage_ledgers USING btree (user_id, usage_date);



--
-- Name: idx_entitlement_usage_entitlement_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_entitlement_usage_entitlement_date ON public.entitlement_usage_ledgers USING btree (user_entitlement_id, usage_date);



--
-- Name: idx_provider_api_keys_provider_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_active ON public.provider_api_keys USING btree (provider_id, is_active);



--
-- Name: idx_provider_api_keys_provider_default_sort; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_default_sort ON public.provider_api_keys USING btree (provider_id, internal_priority, name, id);



--
-- Name: idx_provider_api_keys_provider_active_priority_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_active_priority_id ON public.provider_api_keys USING btree (provider_id, internal_priority, id) WHERE is_active IS TRUE;



--
-- Name: idx_provider_api_keys_provider_created_at_desc; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_created_at_desc ON public.provider_api_keys USING btree (provider_id, created_at DESC NULLS LAST, name, id);



--
-- Name: idx_provider_api_keys_provider_last_used_at_desc; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_last_used_at_desc ON public.provider_api_keys USING btree (provider_id, last_used_at DESC NULLS LAST, name, id);



--
-- Name: idx_provider_api_keys_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_api_keys_provider_id ON public.provider_api_keys USING btree (provider_id);



--
-- Name: idx_provider_family_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_family_kind ON public.provider_endpoints USING btree (provider_id, api_family, endpoint_kind);



--
-- Name: idx_provider_window; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_provider_window ON public.provider_usage_tracking USING btree (provider_id, window_start);



--
-- Name: idx_proxy_node_events_node_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_proxy_node_events_node_created ON public.proxy_node_events USING btree (node_id, created_at);



--
-- Name: idx_rc_provider_status_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_rc_provider_status_created ON public.request_candidates USING btree (provider_id, status, created_at);



--
-- Name: idx_rc_request_id_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_rc_request_id_status ON public.request_candidates USING btree (request_id, status);



--
-- Name: idx_refund_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refund_status ON public.refund_requests USING btree (status);



--
-- Name: idx_refund_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refund_user_created ON public.refund_requests USING btree (user_id, created_at);



--
-- Name: idx_refund_wallet_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_refund_wallet_created ON public.refund_requests USING btree (wallet_id, created_at);



--
-- Name: idx_request_candidates_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_created_at ON public.request_candidates USING btree (created_at);



--
-- Name: idx_request_candidates_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_provider_id ON public.request_candidates USING btree (provider_id);



--
-- Name: idx_request_candidates_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_request_id ON public.request_candidates USING btree (request_id);



--
-- Name: idx_request_candidates_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_request_candidates_status ON public.request_candidates USING btree (status);



--
-- Name: idx_stats_daily_api_key_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_date ON public.stats_daily_api_key USING btree (date);



--
-- Name: idx_stats_daily_api_key_date_cost; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_date_cost ON public.stats_daily_api_key USING btree (date, total_cost);



--
-- Name: idx_stats_daily_api_key_date_requests; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_date_requests ON public.stats_daily_api_key USING btree (date, total_requests);



--
-- Name: idx_stats_daily_api_key_key_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_api_key_key_date ON public.stats_daily_api_key USING btree (api_key_id, date);



--
-- Name: idx_stats_daily_error_category; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_error_category ON public.stats_daily_error USING btree (date, error_category);



--
-- Name: idx_stats_daily_error_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_error_date ON public.stats_daily_error USING btree (date);



--
-- Name: idx_stats_daily_model_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_model_date ON public.stats_daily_model USING btree (date);



--
-- Name: idx_stats_daily_model_date_model; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_model_date_model ON public.stats_daily_model USING btree (date, model);



--
-- Name: idx_stats_daily_provider_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_provider_date ON public.stats_daily_provider USING btree (date);



--
-- Name: idx_stats_daily_provider_date_provider; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_daily_provider_date_provider ON public.stats_daily_provider USING btree (date, provider_name);



--
-- Name: idx_stats_hourly_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_hour ON public.stats_hourly USING btree (hour_utc);



--
-- Name: idx_stats_hourly_model_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_model_hour ON public.stats_hourly_model USING btree (hour_utc);



--
-- Name: idx_stats_hourly_model_model_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_model_model_hour ON public.stats_hourly_model USING btree (model, hour_utc);



--
-- Name: idx_stats_hourly_provider_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_provider_hour ON public.stats_hourly_provider USING btree (hour_utc);



--
-- Name: idx_stats_hourly_user_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_user_hour ON public.stats_hourly_user USING btree (hour_utc);



--
-- Name: idx_stats_hourly_user_user_hour; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_hourly_user_user_hour ON public.stats_hourly_user USING btree (user_id, hour_utc);



--
-- Name: idx_stats_user_daily_user_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_stats_user_daily_user_date ON public.stats_user_daily USING btree (user_id, date);



--
-- Name: idx_usage_api_family; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_api_family ON public.usage USING btree (api_family);



--
-- Name: idx_usage_apikey_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_apikey_created ON public.usage USING btree (api_key_id, created_at);



--
-- Name: idx_usage_billing_finalized_wallet; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_billing_finalized_wallet ON public.usage USING btree (billing_status, finalized_at, wallet_id);



--
-- Name: idx_usage_billing_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_billing_status ON public.usage USING btree (billing_status);



--
-- Name: idx_usage_endpoint_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_endpoint_kind ON public.usage USING btree (endpoint_kind);



--
-- Name: idx_usage_error_category; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_error_category ON public.usage USING btree (error_category);



--
-- Name: idx_usage_family_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_family_kind ON public.usage USING btree (api_family, endpoint_kind);



--
-- Name: idx_usage_model_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_model_created ON public.usage USING btree (model, created_at);



--
-- Name: idx_usage_provider_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_provider_created ON public.usage USING btree (provider_name, created_at);



--
-- Name: idx_usage_provider_key; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_provider_key ON public.usage USING btree (provider_id, provider_api_key_id);



--
-- Name: idx_usage_provider_model_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_provider_model_created ON public.usage USING btree (provider_name, model, created_at);



--
-- Name: idx_usage_status_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_status_user_created ON public.usage USING btree (status, user_id, created_at);



--
-- Name: idx_usage_user_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_user_created ON public.usage USING btree (user_id, created_at);



--
-- Name: idx_usage_wallet_finalized; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_usage_wallet_finalized ON public.usage USING btree (wallet_id, finalized_at);



--
-- Name: ix_usage_counter_deltas_request_kind; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_request_kind ON public.usage_counter_deltas USING btree (request_id, kind, target_id);



--
-- Name: ix_usage_counter_deltas_processed; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_processed ON public.usage_counter_deltas USING btree (processed_at, created_at, id) WHERE processed_at IS NOT NULL;



--
-- Name: ix_usage_counter_deltas_unprocessed; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_counter_deltas_unprocessed ON public.usage_counter_deltas USING btree (created_at, id) WHERE processed_at IS NULL;



--
-- Name: idx_user_model_usage_model; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_model_usage_model ON public.user_model_usage_counts USING btree (model);



--
-- Name: idx_user_model_usage_user; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_model_usage_user ON public.user_model_usage_counts USING btree (user_id);



--
-- Name: idx_user_sessions_user_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_sessions_user_active ON public.user_sessions USING btree (user_id, revoked_at, expires_at);



--
-- Name: idx_user_sessions_user_device; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_sessions_user_device ON public.user_sessions USING btree (user_id, client_device_id);



--
-- Name: idx_user_referrals_inviter; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_referrals_inviter ON public.user_referrals USING btree (inviter_user_id, created_at DESC);



--
-- Name: idx_user_referrals_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_referrals_created ON public.user_referrals USING btree (created_at DESC);



--
-- Name: idx_user_referrals_invite_code; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_user_referrals_invite_code ON public.user_referrals USING btree (invite_code_snapshot);



--
-- Name: idx_referral_rewards_inviter_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_referral_rewards_inviter_status ON public.referral_rewards USING btree (inviter_user_id, status, created_at DESC);



--
-- Name: idx_referral_rewards_inviter_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_referral_rewards_inviter_created ON public.referral_rewards USING btree (inviter_user_id, created_at DESC);



--
-- Name: idx_referral_rewards_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_referral_rewards_created ON public.referral_rewards USING btree (created_at DESC);



--
-- Name: idx_referral_rewards_source_order; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_referral_rewards_source_order ON public.referral_rewards USING btree (source_order_id);



--
-- Name: idx_video_tasks_external_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_external_id ON public.video_tasks USING btree (external_task_id);



--
-- Name: idx_video_tasks_next_poll; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_next_poll ON public.video_tasks USING btree (next_poll_at);



--
-- Name: idx_video_tasks_due_poll; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_due_poll ON public.video_tasks USING btree (status, next_poll_at, updated_at) WHERE next_poll_at IS NOT NULL;



--
-- Name: idx_video_tasks_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_request_id ON public.video_tasks USING btree (request_id);



--
-- Name: idx_video_tasks_user_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_video_tasks_user_status ON public.video_tasks USING btree (user_id, status);



--
-- Name: idx_wallet_daily_usage_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_daily_usage_date ON public.wallet_daily_usage_ledgers USING btree (billing_date);



--
-- Name: idx_wallet_daily_usage_wallet_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_daily_usage_wallet_date ON public.wallet_daily_usage_ledgers USING btree (wallet_id, billing_date);



--
-- Name: idx_wallet_tx_category_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_category_created ON public.wallet_transactions USING btree (category, created_at);



--
-- Name: idx_wallet_tx_link; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_link ON public.wallet_transactions USING btree (link_type, link_id);



--
-- Name: idx_wallet_tx_reason_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_reason_created ON public.wallet_transactions USING btree (reason_code, created_at);



--
-- Name: idx_wallet_tx_wallet_created; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallet_tx_wallet_created ON public.wallet_transactions USING btree (wallet_id, created_at);



--
-- Name: idx_wallets_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_wallets_status ON public.wallets USING btree (status);



--
-- Name: idx_window_time; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS idx_window_time ON public.provider_usage_tracking USING btree (window_start, window_end);



--
-- Name: ix_announcement_reads_announcement_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcement_reads_announcement_id ON public.announcement_reads USING btree (announcement_id);



--
-- Name: ix_announcement_reads_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcement_reads_id ON public.announcement_reads USING btree (id);



--
-- Name: ix_announcements_author_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_author_id ON public.announcements USING btree (author_id);



--
-- Name: ix_announcements_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_created_at ON public.announcements USING btree (created_at);



--
-- Name: ix_announcements_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_id ON public.announcements USING btree (id);



--
-- Name: ix_announcements_is_active; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_announcements_is_active ON public.announcements USING btree (is_active);



--
-- Name: ix_api_key_provider_mappings_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_key_provider_mappings_api_key_id ON public.api_key_provider_mappings USING btree (api_key_id);



--
-- Name: ix_api_key_provider_mappings_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_key_provider_mappings_id ON public.api_key_provider_mappings USING btree (id);



--
-- Name: ix_api_key_provider_mappings_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_key_provider_mappings_provider_id ON public.api_key_provider_mappings USING btree (provider_id);



--
-- Name: ix_api_keys_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_keys_id ON public.api_keys USING btree (id);



--
-- Name: ix_api_keys_key_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_api_keys_key_hash ON public.api_keys USING btree (key_hash);



--
-- Name: ix_api_keys_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_api_keys_user_id ON public.api_keys USING btree (user_id);



--
-- Name: ix_audit_logs_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_created_at ON public.audit_logs USING btree (created_at);



--
-- Name: ix_audit_logs_event_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_event_type ON public.audit_logs USING btree (event_type);



--
-- Name: ix_audit_logs_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_id ON public.audit_logs USING btree (id);



--
-- Name: ix_audit_logs_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_request_id ON public.audit_logs USING btree (request_id);



--
-- Name: ix_audit_logs_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_audit_logs_user_id ON public.audit_logs USING btree (user_id);



--
-- Name: ix_gemini_file_mappings_file_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_gemini_file_mappings_file_name ON public.gemini_file_mappings USING btree (file_name);



--
-- Name: ix_gemini_file_mappings_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_gemini_file_mappings_id ON public.gemini_file_mappings USING btree (id);



--
-- Name: ix_gemini_file_mappings_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_gemini_file_mappings_key_id ON public.gemini_file_mappings USING btree (key_id);



--
-- Name: ix_gemini_file_mappings_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_gemini_file_mappings_user_id ON public.gemini_file_mappings USING btree (user_id);



--
-- Name: ix_global_models_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_global_models_id ON public.global_models USING btree (id);



--
-- Name: ix_global_models_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_global_models_name ON public.global_models USING btree (name);



--
-- Name: ix_global_models_usage_count; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_global_models_usage_count ON public.global_models USING btree (usage_count);



--
-- Name: ix_management_tokens_token_hash; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_management_tokens_token_hash ON public.management_tokens USING btree (token_hash);



--
-- Name: ix_models_global_model_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_models_global_model_id ON public.models USING btree (global_model_id);



--
-- Name: ix_models_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_models_id ON public.models USING btree (id);



--
-- Name: ix_payment_callbacks_payment_order_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_payment_callbacks_payment_order_id ON public.payment_callbacks USING btree (payment_order_id);



--
-- Name: ix_provider_api_keys_circuit_breaker_by_format; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_api_keys_circuit_breaker_by_format ON public.provider_api_keys USING gin (circuit_breaker_by_format);



--
-- Name: ix_provider_api_keys_health_by_format; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_api_keys_health_by_format ON public.provider_api_keys USING gin (health_by_format);



--
-- Name: ix_provider_api_keys_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_api_keys_id ON public.provider_api_keys USING btree (id);



--
-- Name: pool_member_scores_rank_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS pool_member_scores_rank_idx ON public.pool_member_scores USING btree (pool_kind, pool_id, capability, scope_kind, scope_id, hard_state, score DESC);



--
-- Name: pool_member_scores_scheduler_account_rank_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS pool_member_scores_scheduler_account_rank_idx ON public.pool_member_scores USING btree (pool_kind, pool_id, capability, scope_kind, score DESC, last_ranked_at DESC NULLS LAST, member_id, id) WHERE scope_id IS NULL AND hard_state IN ('available', 'unknown');



--
-- Name: pool_member_scores_member_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS pool_member_scores_member_idx ON public.pool_member_scores USING btree (pool_kind, pool_id, member_kind, member_id);



--
-- Name: pool_member_scores_probe_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS pool_member_scores_probe_idx ON public.pool_member_scores USING btree (pool_kind, pool_id, probe_status, last_probe_success_at);



--
-- Name: pool_member_scores_updated_at_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS pool_member_scores_updated_at_idx ON public.pool_member_scores USING btree (updated_at);



--
-- Name: ix_provider_endpoints_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_endpoints_id ON public.provider_endpoints USING btree (id);



--
-- Name: ix_provider_usage_tracking_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_usage_tracking_id ON public.provider_usage_tracking USING btree (id);



--
-- Name: ix_provider_usage_tracking_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_usage_tracking_provider_id ON public.provider_usage_tracking USING btree (provider_id);



--
-- Name: ix_provider_usage_tracking_window_start; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_provider_usage_tracking_window_start ON public.provider_usage_tracking USING btree (window_start);



--
-- Name: ix_providers_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_providers_id ON public.providers USING btree (id);



--
-- Name: ix_providers_name; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_providers_name ON public.providers USING btree (name);



--
-- Name: ix_proxy_node_events_node_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_proxy_node_events_node_id ON public.proxy_node_events USING btree (node_id);



--
-- Name: ix_proxy_nodes_registered_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_proxy_nodes_registered_by ON public.proxy_nodes USING btree (registered_by);



--
-- Name: ix_refund_requests_approved_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_approved_by ON public.refund_requests USING btree (approved_by);



--
-- Name: ix_refund_requests_payment_order_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_payment_order_id ON public.refund_requests USING btree (payment_order_id);



--
-- Name: ix_refund_requests_processed_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_processed_by ON public.refund_requests USING btree (processed_by);



--
-- Name: ix_refund_requests_requested_by; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_refund_requests_requested_by ON public.refund_requests USING btree (requested_by);



--
-- Name: ix_request_candidates_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_api_key_id ON public.request_candidates USING btree (api_key_id);



--
-- Name: ix_request_candidates_endpoint_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_endpoint_id ON public.request_candidates USING btree (endpoint_id);



--
-- Name: ix_request_candidates_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_key_id ON public.request_candidates USING btree (key_id);



--
-- Name: ix_request_candidates_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_request_id ON public.request_candidates USING btree (request_id);



--
-- Name: ix_request_candidates_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_request_candidates_user_id ON public.request_candidates USING btree (user_id);



--
-- Name: ix_stats_daily_date; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_stats_daily_date ON public.stats_daily USING btree (date);



--
-- Name: ix_stats_user_daily_date; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_stats_user_daily_date ON public.stats_user_daily USING btree (date);



--
-- Name: ix_system_configs_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_system_configs_id ON public.system_configs USING btree (id);



--
-- Name: user_group_members_user_id_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS user_group_members_user_id_idx ON public.user_group_members USING btree (user_id);



--
-- Name: user_groups_priority_name_idx; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS user_groups_priority_name_idx ON public.user_groups USING btree (priority DESC, name, id);



--
-- Name: ix_usage_created_at; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_created_at ON public.usage USING btree (created_at);



--
-- Name: ix_usage_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_id ON public.usage USING btree (id);



--
-- Name: ix_usage_provider_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_provider_api_key_id ON public.usage USING btree (provider_api_key_id);



--
-- Name: ix_usage_provider_endpoint_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_provider_endpoint_id ON public.usage USING btree (provider_endpoint_id);



--
-- Name: ix_usage_request_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_usage_request_id ON public.usage USING btree (request_id);



--
-- Name: ix_usage_status; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_status ON public.usage USING btree (status);



--
-- Name: ix_usage_wallet_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_usage_wallet_id ON public.usage USING btree (wallet_id);



--
-- Name: ix_user_oauth_links_provider_type; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_oauth_links_provider_type ON public.user_oauth_links USING btree (provider_type);



--
-- Name: ix_user_oauth_links_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_oauth_links_user_id ON public.user_oauth_links USING btree (user_id);



--
-- Name: ix_user_preferences_default_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_preferences_default_provider_id ON public.user_preferences USING btree (default_provider_id);



--
-- Name: ix_user_preferences_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_preferences_id ON public.user_preferences USING btree (id);



--
-- Name: ix_user_sessions_client_device_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_sessions_client_device_id ON public.user_sessions USING btree (client_device_id);



--
-- Name: ix_user_sessions_user_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_user_sessions_user_id ON public.user_sessions USING btree (user_id);



--
-- Name: ix_users_email; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_users_email ON public.users USING btree (email);



--
-- Name: ix_users_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_users_id ON public.users USING btree (id);



--
-- Name: ix_users_ldap_dn; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_users_ldap_dn ON public.users USING btree (ldap_dn);



--
-- Name: ix_users_ldap_username; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_users_ldap_username ON public.users USING btree (ldap_username);



--
-- Name: ix_users_username; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_users_username ON public.users USING btree (username);



--
-- Name: ix_video_tasks_api_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_api_key_id ON public.video_tasks USING btree (api_key_id);



--
-- Name: ix_video_tasks_endpoint_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_endpoint_id ON public.video_tasks USING btree (endpoint_id);



--
-- Name: ix_video_tasks_key_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_key_id ON public.video_tasks USING btree (key_id);



--
-- Name: ix_video_tasks_provider_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_provider_id ON public.video_tasks USING btree (provider_id);



--
-- Name: ix_video_tasks_remixed_from_task_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_video_tasks_remixed_from_task_id ON public.video_tasks USING btree (remixed_from_task_id);



--
-- Name: ix_video_tasks_short_id; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS ix_video_tasks_short_id ON public.video_tasks USING btree (short_id);



--
-- Name: ix_wallet_transactions_operator_id; Type: INDEX; Schema: public; Owner: -
--

CREATE INDEX IF NOT EXISTS ix_wallet_transactions_operator_id ON public.wallet_transactions USING btree (operator_id);



--
-- Name: uq_billing_rules_global_model_task; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS uq_billing_rules_global_model_task ON public.billing_rules USING btree (global_model_id, task_type) WHERE ((is_enabled = true) AND (global_model_id IS NOT NULL));



--
-- Name: uq_billing_rules_model_task; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS uq_billing_rules_model_task ON public.billing_rules USING btree (model_id, task_type) WHERE ((is_enabled = true) AND (model_id IS NOT NULL));



--
-- Name: uq_dimension_collectors_enabled; Type: INDEX; Schema: public; Owner: -
--

CREATE UNIQUE INDEX IF NOT EXISTS uq_dimension_collectors_enabled ON public.dimension_collectors USING btree (api_format, task_type, dimension_name, priority) WHERE (is_enabled = true);



--
