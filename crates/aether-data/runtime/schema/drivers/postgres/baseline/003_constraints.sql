-- Name: announcement_reads announcement_reads_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT announcement_reads_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcements announcements_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcements
    ADD CONSTRAINT announcements_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings api_key_provider_mappings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT api_key_provider_mappings_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_keys api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: audit_logs audit_logs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: billing_rules billing_rules_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.billing_rules
    ADD CONSTRAINT billing_rules_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: dimension_collectors dimension_collectors_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.dimension_collectors
    ADD CONSTRAINT dimension_collectors_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_file_name_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_file_name_key UNIQUE (file_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: global_models global_models_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.global_models
    ADD CONSTRAINT global_models_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: ldap_configs ldap_configs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.ldap_configs
    ADD CONSTRAINT ldap_configs_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: management_tokens management_tokens_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.management_tokens
    ADD CONSTRAINT management_tokens_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models models_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: oauth_providers oauth_providers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.oauth_providers
    ADD CONSTRAINT oauth_providers_pkey PRIMARY KEY (provider_type);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_callbacks payment_callbacks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_callbacks
    ADD CONSTRAINT payment_callbacks_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders payment_orders_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT payment_orders_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_api_keys provider_api_keys_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_api_keys
    ADD CONSTRAINT provider_api_keys_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: pool_member_scores pool_member_scores_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.pool_member_scores
    ADD CONSTRAINT pool_member_scores_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_endpoints provider_endpoints_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_endpoints
    ADD CONSTRAINT provider_endpoints_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_usage_tracking provider_usage_tracking_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_usage_tracking
    ADD CONSTRAINT provider_usage_tracking_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: providers providers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.providers
    ADD CONSTRAINT providers_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_node_events proxy_node_events_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_node_events
    ADD CONSTRAINT proxy_node_events_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_nodes proxy_nodes_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_nodes
    ADD CONSTRAINT proxy_nodes_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_api_key stats_daily_api_key_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_api_key
    ADD CONSTRAINT stats_daily_api_key_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_error stats_daily_error_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_error
    ADD CONSTRAINT stats_daily_error_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_model stats_daily_model_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_model
    ADD CONSTRAINT stats_daily_model_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily stats_daily_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily
    ADD CONSTRAINT stats_daily_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_provider stats_daily_provider_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_provider
    ADD CONSTRAINT stats_daily_provider_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_model stats_hourly_model_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_model
    ADD CONSTRAINT stats_hourly_model_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly stats_hourly_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly
    ADD CONSTRAINT stats_hourly_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_provider stats_hourly_provider_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_provider
    ADD CONSTRAINT stats_hourly_provider_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_user stats_hourly_user_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_user
    ADD CONSTRAINT stats_hourly_user_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_summary stats_summary_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_summary
    ADD CONSTRAINT stats_summary_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_user_daily stats_user_daily_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_user_daily
    ADD CONSTRAINT stats_user_daily_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: system_configs system_configs_key_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.system_configs
    ADD CONSTRAINT system_configs_key_key UNIQUE (key);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: system_configs system_configs_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.system_configs
    ADD CONSTRAINT system_configs_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings uq_apikey_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT uq_apikey_provider UNIQUE (api_key_id, provider_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: management_tokens uq_management_tokens_user_name; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.management_tokens
    ADD CONSTRAINT uq_management_tokens_user_name UNIQUE (user_id, name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links uq_oauth_provider_user; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT uq_oauth_provider_user UNIQUE (provider_type, provider_user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_callbacks uq_payment_callbacks_callback_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_callbacks
    ADD CONSTRAINT uq_payment_callbacks_callback_key UNIQUE (callback_key);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders uq_payment_orders_order_no; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT uq_payment_orders_order_no UNIQUE (order_no);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_endpoints uq_provider_api_format; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_endpoints
    ADD CONSTRAINT uq_provider_api_format UNIQUE (provider_id, api_format);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models uq_provider_model; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT uq_provider_model UNIQUE (provider_id, provider_model_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_nodes uq_proxy_node_ip_port; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_nodes
    ADD CONSTRAINT uq_proxy_node_ip_port UNIQUE (ip, port);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests uq_refund_requests_idempotency_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT uq_refund_requests_idempotency_key UNIQUE (idempotency_key);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests uq_refund_requests_refund_no; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT uq_refund_requests_refund_no UNIQUE (refund_no);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates uq_request_candidate_with_retry; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT uq_request_candidate_with_retry UNIQUE (request_id, candidate_index, retry_index);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_api_key uq_stats_daily_api_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_api_key
    ADD CONSTRAINT uq_stats_daily_api_key UNIQUE (api_key_id, date);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_error uq_stats_daily_error; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_error
    ADD CONSTRAINT uq_stats_daily_error UNIQUE (date, error_category, provider_name, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_model uq_stats_daily_model; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_model
    ADD CONSTRAINT uq_stats_daily_model UNIQUE (date, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_provider uq_stats_daily_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_provider
    ADD CONSTRAINT uq_stats_daily_provider UNIQUE (date, provider_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly uq_stats_hourly_hour; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly
    ADD CONSTRAINT uq_stats_hourly_hour UNIQUE (hour_utc);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_model uq_stats_hourly_model; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_model
    ADD CONSTRAINT uq_stats_hourly_model UNIQUE (hour_utc, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_provider uq_stats_hourly_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_provider
    ADD CONSTRAINT uq_stats_hourly_provider UNIQUE (hour_utc, provider_name);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_hourly_user uq_stats_hourly_user; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_hourly_user
    ADD CONSTRAINT uq_stats_hourly_user UNIQUE (hour_utc, user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_user_daily uq_stats_user_daily; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_user_daily
    ADD CONSTRAINT uq_stats_user_daily UNIQUE (user_id, date);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcement_reads uq_user_announcement; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT uq_user_announcement UNIQUE (user_id, announcement_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_model_usage_counts uq_user_model_usage_count; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_model_usage_counts
    ADD CONSTRAINT uq_user_model_usage_count UNIQUE (user_id, model);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links uq_user_oauth_provider; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT uq_user_oauth_provider UNIQUE (user_id, provider_type);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks uq_video_tasks_request_id; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT uq_video_tasks_request_id UNIQUE (request_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_daily_usage_ledgers uq_wallet_daily_usage_ledgers_wallet_date_tz; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_daily_usage_ledgers
    ADD CONSTRAINT uq_wallet_daily_usage_ledgers_wallet_date_tz UNIQUE (wallet_id, billing_date, billing_timezone);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets uq_wallets_api_key_id; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT uq_wallets_api_key_id UNIQUE (api_key_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets uq_wallets_user_id; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT uq_wallets_user_id UNIQUE (user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_model_usage_counts user_model_usage_counts_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_model_usage_counts
    ADD CONSTRAINT user_model_usage_counts_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links user_oauth_links_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT user_oauth_links_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_user_id_key; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_user_id_key UNIQUE (user_id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_sessions user_sessions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: users users_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.users
    ADD CONSTRAINT users_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_daily_usage_ledgers wallet_daily_usage_ledgers_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_daily_usage_ledgers
    ADD CONSTRAINT wallet_daily_usage_ledgers_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_transactions wallet_transactions_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_transactions
    ADD CONSTRAINT wallet_transactions_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets wallets_pkey; Type: CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT wallets_pkey PRIMARY KEY (id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
