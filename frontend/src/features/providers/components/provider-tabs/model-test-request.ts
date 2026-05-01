const DEFAULT_MODEL_TEST_MESSAGE = 'Hello! This is a test message.'

/** Pool-managed provider runs concurrent checks; single-key provider does not. */
export const POOL_TEST_CONCURRENCY = 5
export const SINGLE_TEST_CONCURRENCY = 1

export function buildDefaultModelTestRequestBody(modelName: string): string {
  return JSON.stringify({
    model: modelName,
    messages: [
      {
        role: 'user',
        content: DEFAULT_MODEL_TEST_MESSAGE,
      },
    ],
    max_tokens: 30,
    stream: true,
  }, null, 2)
}

export function buildDefaultModelTestRequestHeaders(): string {
  return JSON.stringify({}, null, 2)
}

function parseModelTestJsonObjectDraft(
  draft: string,
  options: {
    emptyValue: Record<string, unknown> | null
    emptyError: string | null
    invalidTypeError: string
  },
): { value: Record<string, unknown> | null; error: string | null } {
  const normalized = draft.trim()
  if (!normalized) {
    return {
      value: options.emptyValue,
      error: options.emptyError,
    }
  }

  try {
    const parsed = JSON.parse(normalized)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return {
        value: null,
        error: options.invalidTypeError,
      }
    }
    return {
      value: parsed as Record<string, unknown>,
      error: null,
    }
  } catch (error) {
    return {
      value: null,
      error: error instanceof Error ? error.message : '无效的 JSON',
    }
  }
}

export function parseModelTestRequestBodyDraft(
  draft: string,
): { value: Record<string, unknown> | null; error: string | null } {
  return parseModelTestJsonObjectDraft(draft, {
    emptyValue: null,
    emptyError: '测试请求体不能为空',
    invalidTypeError: '测试请求体必须是 JSON 对象',
  })
}

export function parseModelTestRequestHeadersDraft(
  draft: string,
): { value: Record<string, unknown> | null; error: string | null } {
  return parseModelTestJsonObjectDraft(draft, {
    emptyValue: {},
    emptyError: null,
    invalidTypeError: '测试请求头必须是 JSON 对象',
  })
}
