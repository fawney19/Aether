-- request_candidates 记录请求路由的历史事实。API Key 删除后，已有记录与迟到的
-- 异步候选写入仍须保留原始身份快照，不能依赖当前认证目录中的可变行。

ALTER TABLE ONLY public.request_candidates
  DROP CONSTRAINT IF EXISTS request_candidates_api_key_id_fkey;
