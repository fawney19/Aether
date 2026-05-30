WITH endpoint_url_parts AS (
    SELECT
        e.id,
        e.base_url,
        rtrim(split_part(e.base_url, '?', 1), '/') AS base_without_query,
        CASE
            WHEN position('?' IN e.base_url) > 0 THEN substring(e.base_url FROM position('?' IN e.base_url))
            ELSE ''
        END AS query_suffix,
        lower(trim(e.api_format)) AS normalized_api_format,
        lower(trim(coalesce(e.custom_path, ''))) AS normalized_custom_path,
        lower(rtrim(split_part(e.base_url, '?', 1), '/')) AS normalized_base,
        lower(trim(coalesce(p.provider_type, ''))) AS provider_type
    FROM public.provider_endpoints e
    LEFT JOIN public.providers p ON p.id = e.provider_id
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
      AND normalized_base !~ '/v[0-9]+(beta[0-9]*)?(/|$)'
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
UPDATE public.provider_endpoints e
SET base_url = u.next_base_url
FROM endpoint_api_root_updates u
WHERE e.id = u.id
  AND e.base_url IS DISTINCT FROM u.next_base_url;

UPDATE public.provider_endpoints e
SET custom_path = CASE
        WHEN lower(trim(e.api_format)) = 'openai:chat'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/chat/completions'
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'openai:responses'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/responses'
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'openai:responses:compact'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/responses/compact'
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'claude:messages'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/messages'
            THEN NULL
        WHEN lower(trim(e.api_format)) IN ('openai:embedding', 'jina:embedding')
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/embeddings'
            THEN NULL
        WHEN lower(trim(e.api_format)) IN ('openai:rerank', 'jina:rerank')
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/rerank'
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'openai:image'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/images/generations'
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'openai:video'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1/videos'
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'gemini:generate_content'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1beta/models/{model}:{action}'
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'gemini:embedding'
            AND lower(trim(coalesce(e.custom_path, ''))) IN ('/v1beta/models/{model}:embedcontent', '/v1beta/models/{model}:{action}')
            THEN NULL
        WHEN lower(trim(e.api_format)) = 'gemini:video'
            AND lower(trim(coalesce(e.custom_path, ''))) = '/v1beta/models/{model}:predictlongrunning'
            THEN NULL
        WHEN lower(trim(e.api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            THEN regexp_replace(trim(e.custom_path), '^/v1beta(?=/)', '', 'i')
        ELSE regexp_replace(trim(e.custom_path), '^/v1(?=/)', '', 'i')
    END
FROM public.providers p
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
  AND p.id = e.provider_id
  AND lower(trim(coalesce(p.provider_type, ''))) NOT IN (
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
  AND (
        (
            lower(trim(e.api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND lower(trim(coalesce(e.custom_path, ''))) LIKE '/v1beta/%'
        )
        OR (
            lower(trim(e.api_format)) NOT IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND lower(trim(coalesce(e.custom_path, ''))) LIKE '/v1/%'
        )
    )
  AND lower(rtrim(split_part(e.base_url, '?', 1), '/')) ~ '/v[0-9]+(beta[0-9]*)?(/|$)';

UPDATE public.provider_endpoints
SET custom_path = NULL
WHERE custom_path IS NOT NULL
  AND trim(custom_path) = '';
