WITH endpoint_url_parts AS (
    SELECT
        e.id,
        CASE
            WHEN instr(e.base_url, '?') > 0 THEN rtrim(substr(e.base_url, 1, instr(e.base_url, '?') - 1), '/')
            ELSE rtrim(e.base_url, '/')
        END AS base_without_query,
        CASE
            WHEN instr(e.base_url, '?') > 0 THEN substr(e.base_url, instr(e.base_url, '?'))
            ELSE ''
        END AS query_suffix,
        lower(trim(e.api_format)) AS normalized_api_format,
        lower(trim(coalesce(e.custom_path, ''))) AS normalized_custom_path,
        lower(CASE
            WHEN instr(e.base_url, '?') > 0 THEN rtrim(substr(e.base_url, 1, instr(e.base_url, '?') - 1), '/')
            ELSE rtrim(e.base_url, '/')
        END) AS normalized_base,
        lower(trim(coalesce(p.provider_type, ''))) AS provider_type
    FROM provider_endpoints e
    LEFT JOIN providers p ON p.id = e.provider_id
    WHERE lower(trim(e.api_format)) IN (
        'openai:chat',
        'openai:responses',
        'openai:responses:compact',
        'openai:embedding',
        'openai:rerank',
        'openai:image',
        'openai:video',
        'jina:embedding',
        'jina:rerank',
        'claude:messages',
        'gemini:generate_content',
        'gemini:embedding',
        'gemini:video'
    )
),
endpoint_api_root_updates AS (
    SELECT
        id,
        base_without_query
            || CASE
                WHEN normalized_api_format IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
                    THEN '/v1beta'
                ELSE '/v1'
            END
            || query_suffix AS next_base_url
    FROM endpoint_url_parts
    WHERE provider_type NOT IN (
        'codex',
        'chatgpt_web',
        'claude_code',
        'kiro',
        'gemini_cli',
        'vertex_ai',
        'antigravity',
        'grok',
        'windsurf'
    )
      AND normalized_base NOT GLOB '*/v[0-9]'
      AND normalized_base NOT GLOB '*/v[0-9][0-9]'
      AND normalized_base NOT GLOB '*/v[0-9]/*'
      AND normalized_base NOT GLOB '*/v[0-9][0-9]/*'
      AND normalized_base NOT GLOB '*/v[0-9]beta*'
      AND normalized_base NOT GLOB '*/v[0-9][0-9]beta*'
      AND (
          (
              normalized_api_format IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
              AND normalized_custom_path LIKE '/v1beta/%'
          )
          OR (
              normalized_api_format NOT IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
              AND normalized_custom_path LIKE '/v1/%'
          )
          OR normalized_custom_path = ''
      )
)
UPDATE provider_endpoints
SET base_url = (
    SELECT next_base_url
    FROM endpoint_api_root_updates
    WHERE endpoint_api_root_updates.id = provider_endpoints.id
)
WHERE id IN (SELECT id FROM endpoint_api_root_updates);

UPDATE provider_endpoints
SET custom_path = CASE
        WHEN lower(trim(api_format)) = 'openai:chat'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/chat/completions'
            THEN NULL
        WHEN lower(trim(api_format)) = 'openai:responses'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/responses'
            THEN NULL
        WHEN lower(trim(api_format)) = 'openai:responses:compact'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/responses/compact'
            THEN NULL
        WHEN lower(trim(api_format)) = 'claude:messages'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/messages'
            THEN NULL
        WHEN lower(trim(api_format)) IN ('openai:embedding', 'jina:embedding')
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/embeddings'
            THEN NULL
        WHEN lower(trim(api_format)) IN ('openai:rerank', 'jina:rerank')
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/rerank'
            THEN NULL
        WHEN lower(trim(api_format)) = 'openai:image'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/images/generations'
            THEN NULL
        WHEN lower(trim(api_format)) = 'openai:video'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1/videos'
            THEN NULL
        WHEN lower(trim(api_format)) = 'gemini:generate_content'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1beta/models/{model}:{action}'
            THEN NULL
        WHEN lower(trim(api_format)) = 'gemini:embedding'
            AND lower(trim(coalesce(custom_path, ''))) IN ('/v1beta/models/{model}:embedcontent', '/v1beta/models/{model}:{action}')
            THEN NULL
        WHEN lower(trim(api_format)) = 'gemini:video'
            AND lower(trim(coalesce(custom_path, ''))) = '/v1beta/models/{model}:predictlongrunning'
            THEN NULL
        WHEN lower(trim(api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            THEN '/' || substr(trim(custom_path), 9)
        ELSE '/' || substr(trim(custom_path), 5)
    END
WHERE lower(trim(api_format)) IN (
        'openai:chat',
        'openai:responses',
        'openai:responses:compact',
        'openai:embedding',
        'openai:rerank',
        'openai:image',
        'openai:video',
        'jina:embedding',
        'jina:rerank',
        'claude:messages',
        'gemini:generate_content',
        'gemini:embedding',
        'gemini:video'
    )
  AND NOT EXISTS (
        SELECT 1
        FROM providers p
        WHERE p.id = provider_endpoints.provider_id
          AND lower(trim(coalesce(p.provider_type, ''))) IN (
              'codex',
              'chatgpt_web',
              'claude_code',
              'kiro',
              'gemini_cli',
              'vertex_ai',
              'antigravity',
              'grok',
              'windsurf'
          )
    )
  AND (
        (
            lower(trim(api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND lower(trim(coalesce(custom_path, ''))) LIKE '/v1beta/%'
        )
        OR (
            lower(trim(api_format)) NOT IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND lower(trim(coalesce(custom_path, ''))) LIKE '/v1/%'
        )
    )
  AND (
        lower(rtrim(CASE WHEN instr(base_url, '?') > 0 THEN substr(base_url, 1, instr(base_url, '?') - 1) ELSE base_url END, '/')) GLOB '*/v[0-9]'
        OR lower(rtrim(CASE WHEN instr(base_url, '?') > 0 THEN substr(base_url, 1, instr(base_url, '?') - 1) ELSE base_url END, '/')) GLOB '*/v[0-9][0-9]'
        OR lower(rtrim(CASE WHEN instr(base_url, '?') > 0 THEN substr(base_url, 1, instr(base_url, '?') - 1) ELSE base_url END, '/')) GLOB '*/v[0-9]/*'
        OR lower(rtrim(CASE WHEN instr(base_url, '?') > 0 THEN substr(base_url, 1, instr(base_url, '?') - 1) ELSE base_url END, '/')) GLOB '*/v[0-9][0-9]/*'
        OR lower(rtrim(CASE WHEN instr(base_url, '?') > 0 THEN substr(base_url, 1, instr(base_url, '?') - 1) ELSE base_url END, '/')) GLOB '*/v[0-9]beta*'
        OR lower(rtrim(CASE WHEN instr(base_url, '?') > 0 THEN substr(base_url, 1, instr(base_url, '?') - 1) ELSE base_url END, '/')) GLOB '*/v[0-9][0-9]beta*'
    );

UPDATE provider_endpoints
SET custom_path = NULL
WHERE custom_path IS NOT NULL
  AND trim(custom_path) = '';
