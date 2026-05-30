UPDATE provider_endpoints e
LEFT JOIN providers p ON p.id = e.provider_id
SET e.base_url = CONCAT(
    TRIM(TRAILING '/' FROM SUBSTRING_INDEX(e.base_url, '?', 1)),
    CASE
        WHEN LOWER(TRIM(e.api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            THEN '/v1beta'
        ELSE '/v1'
    END,
    IF(LOCATE('?', e.base_url) > 0, SUBSTRING(e.base_url, LOCATE('?', e.base_url)), '')
)
WHERE LOWER(TRIM(e.api_format)) IN (
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
  AND COALESCE(LOWER(TRIM(p.provider_type)), '') NOT IN (
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
  AND LOWER(TRIM(TRAILING '/' FROM SUBSTRING_INDEX(e.base_url, '?', 1))) NOT REGEXP '/v[0-9]+(beta[0-9]*)?(/|$)'
  AND (
        (
            LOWER(TRIM(e.api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) LIKE '/v1beta/%'
        )
        OR (
            LOWER(TRIM(e.api_format)) NOT IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) LIKE '/v1/%'
        )
        OR COALESCE(TRIM(e.custom_path), '') = ''
    );

UPDATE provider_endpoints e
LEFT JOIN providers p ON p.id = e.provider_id
SET e.custom_path = CASE
        WHEN LOWER(TRIM(e.api_format)) = 'openai:chat'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/chat/completions'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'openai:responses'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/responses'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'openai:responses:compact'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/responses/compact'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'claude:messages'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/messages'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) IN ('openai:embedding', 'jina:embedding')
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/embeddings'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) IN ('openai:rerank', 'jina:rerank')
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/rerank'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'openai:image'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/images/generations'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'openai:video'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1/videos'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'gemini:generate_content'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1beta/models/{model}:{action}'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'gemini:embedding'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) IN ('/v1beta/models/{model}:embedcontent', '/v1beta/models/{model}:{action}')
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) = 'gemini:video'
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) = '/v1beta/models/{model}:predictlongrunning'
            THEN NULL
        WHEN LOWER(TRIM(e.api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            THEN CONCAT('/', SUBSTRING(TRIM(e.custom_path), 9))
        ELSE CONCAT('/', SUBSTRING(TRIM(e.custom_path), 5))
    END
WHERE LOWER(TRIM(e.api_format)) IN (
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
  AND COALESCE(LOWER(TRIM(p.provider_type)), '') NOT IN (
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
            LOWER(TRIM(e.api_format)) IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) LIKE '/v1beta/%'
        )
        OR (
            LOWER(TRIM(e.api_format)) NOT IN ('gemini:generate_content', 'gemini:embedding', 'gemini:video')
            AND LOWER(TRIM(COALESCE(e.custom_path, ''))) LIKE '/v1/%'
        )
    )
  AND LOWER(TRIM(TRAILING '/' FROM SUBSTRING_INDEX(e.base_url, '?', 1))) REGEXP '/v[0-9]+(beta[0-9]*)?(/|$)';

UPDATE provider_endpoints
SET custom_path = NULL
WHERE custom_path IS NOT NULL
  AND TRIM(custom_path) = '';
