-- Name: announcement_reads announcement_reads_announcement_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT announcement_reads_announcement_id_fkey FOREIGN KEY (announcement_id) REFERENCES public.announcements(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcement_reads announcement_reads_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcement_reads
    ADD CONSTRAINT announcement_reads_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: announcements announcements_author_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.announcements
    ADD CONSTRAINT announcements_author_id_fkey FOREIGN KEY (author_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings api_key_provider_mappings_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT api_key_provider_mappings_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_key_provider_mappings api_key_provider_mappings_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_key_provider_mappings
    ADD CONSTRAINT api_key_provider_mappings_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: api_keys api_keys_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.api_keys
    ADD CONSTRAINT api_keys_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: audit_logs audit_logs_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.audit_logs
    ADD CONSTRAINT audit_logs_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: billing_rules billing_rules_global_model_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.billing_rules
    ADD CONSTRAINT billing_rules_global_model_id_fkey FOREIGN KEY (global_model_id) REFERENCES public.global_models(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: billing_rules billing_rules_model_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.billing_rules
    ADD CONSTRAINT billing_rules_model_id_fkey FOREIGN KEY (model_id) REFERENCES public.models(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_api_keys fk_provider_api_keys_provider; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_api_keys
    ADD CONSTRAINT fk_provider_api_keys_provider FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_key_id_fkey FOREIGN KEY (key_id) REFERENCES public.provider_api_keys(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: gemini_file_mappings gemini_file_mappings_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.gemini_file_mappings
    ADD CONSTRAINT gemini_file_mappings_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: management_tokens management_tokens_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.management_tokens
    ADD CONSTRAINT management_tokens_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models models_global_model_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_global_model_id_fkey FOREIGN KEY (global_model_id) REFERENCES public.global_models(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: models models_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.models
    ADD CONSTRAINT models_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_callbacks payment_callbacks_payment_order_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_callbacks
    ADD CONSTRAINT payment_callbacks_payment_order_id_fkey FOREIGN KEY (payment_order_id) REFERENCES public.payment_orders(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders payment_orders_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT payment_orders_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: payment_orders payment_orders_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.payment_orders
    ADD CONSTRAINT payment_orders_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE RESTRICT;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_endpoints provider_endpoints_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_endpoints
    ADD CONSTRAINT provider_endpoints_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: provider_usage_tracking provider_usage_tracking_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.provider_usage_tracking
    ADD CONSTRAINT provider_usage_tracking_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_node_events proxy_node_events_node_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_node_events
    ADD CONSTRAINT proxy_node_events_node_id_fkey FOREIGN KEY (node_id) REFERENCES public.proxy_nodes(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: proxy_nodes proxy_nodes_registered_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.proxy_nodes
    ADD CONSTRAINT proxy_nodes_registered_by_fkey FOREIGN KEY (registered_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_approved_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_approved_by_fkey FOREIGN KEY (approved_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_payment_order_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_payment_order_id_fkey FOREIGN KEY (payment_order_id) REFERENCES public.payment_orders(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_processed_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_processed_by_fkey FOREIGN KEY (processed_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_requested_by_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_requested_by_fkey FOREIGN KEY (requested_by) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: refund_requests refund_requests_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.refund_requests
    ADD CONSTRAINT refund_requests_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE RESTRICT;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_endpoint_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_endpoint_id_fkey FOREIGN KEY (endpoint_id) REFERENCES public.provider_endpoints(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: request_candidates request_candidates_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.request_candidates
    ADD CONSTRAINT request_candidates_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_daily_api_key stats_daily_api_key_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_daily_api_key
    ADD CONSTRAINT stats_daily_api_key_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: stats_user_daily stats_user_daily_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.stats_user_daily
    ADD CONSTRAINT stats_user_daily_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_provider_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_provider_api_key_id_fkey FOREIGN KEY (provider_api_key_id) REFERENCES public.provider_api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_provider_endpoint_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_provider_endpoint_id_fkey FOREIGN KEY (provider_endpoint_id) REFERENCES public.provider_endpoints(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: usage usage_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.usage
    ADD CONSTRAINT usage_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_model_usage_counts user_model_usage_counts_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_model_usage_counts
    ADD CONSTRAINT user_model_usage_counts_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links user_oauth_links_provider_type_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT user_oauth_links_provider_type_fkey FOREIGN KEY (provider_type) REFERENCES public.oauth_providers(provider_type) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_oauth_links user_oauth_links_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_oauth_links
    ADD CONSTRAINT user_oauth_links_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_default_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_default_provider_id_fkey FOREIGN KEY (default_provider_id) REFERENCES public.providers(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_preferences user_preferences_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_preferences
    ADD CONSTRAINT user_preferences_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: user_sessions user_sessions_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.user_sessions
    ADD CONSTRAINT user_sessions_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_endpoint_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_endpoint_id_fkey FOREIGN KEY (endpoint_id) REFERENCES public.provider_endpoints(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_key_id_fkey FOREIGN KEY (key_id) REFERENCES public.provider_api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_provider_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_provider_id_fkey FOREIGN KEY (provider_id) REFERENCES public.providers(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_remixed_from_task_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_remixed_from_task_id_fkey FOREIGN KEY (remixed_from_task_id) REFERENCES public.video_tasks(id);
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: video_tasks video_tasks_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.video_tasks
    ADD CONSTRAINT video_tasks_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_daily_usage_ledgers wallet_daily_usage_ledgers_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_daily_usage_ledgers
    ADD CONSTRAINT wallet_daily_usage_ledgers_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_transactions wallet_transactions_operator_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_transactions
    ADD CONSTRAINT wallet_transactions_operator_id_fkey FOREIGN KEY (operator_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallet_transactions wallet_transactions_wallet_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallet_transactions
    ADD CONSTRAINT wallet_transactions_wallet_id_fkey FOREIGN KEY (wallet_id) REFERENCES public.wallets(id) ON DELETE CASCADE;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets wallets_api_key_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT wallets_api_key_id_fkey FOREIGN KEY (api_key_id) REFERENCES public.api_keys(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;



--
-- Name: wallets wallets_user_id_fkey; Type: FK CONSTRAINT; Schema: public; Owner: -
--

DO $mig$ BEGIN
  ALTER TABLE ONLY public.wallets
    ADD CONSTRAINT wallets_user_id_fkey FOREIGN KEY (user_id) REFERENCES public.users(id) ON DELETE SET NULL;
EXCEPTION
  WHEN duplicate_object THEN NULL;
  WHEN duplicate_table THEN NULL;
  WHEN invalid_table_definition THEN NULL;
END $mig$;
