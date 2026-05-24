const MODEL_TEST_RESPONSE_PREVIEW_MAX_LENGTH = 160
const MODEL_TEST_IMAGE_PREVIEW_MAX_ITEMS = 6

type JsonRecord = Record<string, unknown>

export type ModelTestImagePreview = {
  src: string
  label: string
  source: 'base64' | 'url'
}

export function extractModelTestResponsePreview(responseBody: unknown): string | null {
  const normalizedBody = normalizeModelTestResponseBody(responseBody)
  const text = extractResponseText(normalizedBody)
  if (text) return text

  const reasoning = extractResponseReasoning(normalizedBody)
  if (reasoning) return `推理：${reasoning}`

  const image = extractImagePreview(normalizedBody)
  if (image) return image

  const summary = extractResponseSummary(normalizedBody)
  if (summary) return summary

  return null
}

export function extractModelTestImagePreviews(responseBody: unknown): ModelTestImagePreview[] {
  const previews: ModelTestImagePreview[] = []
  collectImagePreviews(normalizeModelTestResponseBody(responseBody), previews, new Set(), 0)
  return previews
}

function normalizeModelTestResponseBody(value: unknown): unknown {
  if (typeof value !== 'string') return value
  const trimmed = value.trim()
  if (!trimmed) return value

  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    try {
      return JSON.parse(trimmed)
    } catch {
      return value
    }
  }

  return value
}

function isJsonRecord(value: unknown): value is JsonRecord {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}

function compactPreviewText(value: unknown): string | null {
  if (typeof value !== 'string') return null

  const normalized = value.replace(/\s+/g, ' ').trim()
  if (!normalized) return null

  if (normalized.length <= MODEL_TEST_RESPONSE_PREVIEW_MAX_LENGTH) {
    return normalized
  }
  return `${normalized.slice(0, MODEL_TEST_RESPONSE_PREVIEW_MAX_LENGTH - 3)}...`
}

function joinPreviewParts(parts: string[]): string | null {
  return compactPreviewText(parts.filter(Boolean).join(' '))
}

function extractTextFromContentParts(value: unknown, depth = 0): string | null {
  if (depth > 4) return null

  const directText = compactPreviewText(value)
  if (directText) return directText

  if (!Array.isArray(value)) return null

  const parts = value.flatMap((part) => {
    if (typeof part === 'string') return [part]
    if (!isJsonRecord(part)) return []

    const text = compactPreviewText(part.text)
      ?? compactPreviewText(part.content)
      ?? extractTextFromContentParts(part.parts, depth + 1)
    return text ? [text] : []
  })

  return joinPreviewParts(parts)
}

function extractResponseText(responseBody: unknown, depth = 0): string | null {
  if (depth > 4 || !isJsonRecord(responseBody)) return null

  const wrappedText = extractResponseText(responseBody.response, depth + 1)
    ?? extractResponseText(responseBody.body, depth + 1)
  if (wrappedText) return wrappedText

  const outputText = compactPreviewText(responseBody.output_text)
  if (outputText) return outputText

  const topLevelContentText = extractTextFromContentParts(responseBody.content, depth + 1)
  if (topLevelContentText) return topLevelContentText

  const choicesText = extractChoicesText(responseBody.choices, depth + 1)
  if (choicesText) return choicesText

  const outputTextParts = extractOutputText(responseBody.output, depth + 1)
  if (outputTextParts) return outputTextParts

  const candidateText = extractGeminiCandidateText(responseBody.candidates, depth + 1)
  if (candidateText) return candidateText

  return null
}

function extractImagePreview(responseBody: unknown, depth = 0): string | null {
  if (depth > 4 || !isJsonRecord(responseBody)) return null

  const wrappedPreview = extractImagePreview(responseBody.response, depth + 1)
    ?? extractImagePreview(responseBody.body, depth + 1)
  if (wrappedPreview) return wrappedPreview

  const dataPreview = extractImagePreviewFromCollection(responseBody.data, depth + 1)
  if (dataPreview) return dataPreview

  const outputPreview = extractImagePreviewFromCollection(responseBody.output, depth + 1)
  if (outputPreview) return outputPreview

  const imagesPreview = extractImagePreviewFromCollection(responseBody.images, depth + 1)
  if (imagesPreview) return imagesPreview

  const contentPreview = extractImagePreviewFromContentParts(responseBody.content, depth + 1)
  if (contentPreview) return contentPreview

  return null
}

function collectImagePreviews(
  value: unknown,
  previews: ModelTestImagePreview[],
  seen: Set<string>,
  depth: number,
) {
  if (depth > 5 || previews.length >= MODEL_TEST_IMAGE_PREVIEW_MAX_ITEMS || value == null) return

  if (Array.isArray(value)) {
    for (const item of value) {
      collectImagePreviews(item, previews, seen, depth + 1)
      if (previews.length >= MODEL_TEST_IMAGE_PREVIEW_MAX_ITEMS) return
    }
    return
  }

  if (!isJsonRecord(value)) return

  collectImagePreviewFromRecord(value, previews, seen, depth)

  const nestedValues = [
    value.response,
    value.body,
    value.data,
    value.output,
    value.images,
    value.content,
    value.item,
  ]
  for (const nested of nestedValues) {
    collectImagePreviews(nested, previews, seen, depth + 1)
    if (previews.length >= MODEL_TEST_IMAGE_PREVIEW_MAX_ITEMS) return
  }
}

function collectImagePreviewFromRecord(
  value: JsonRecord,
  previews: ModelTestImagePreview[],
  seen: Set<string>,
  depth: number,
) {
  const mime = imageMimeFromRecord(value)

  const imageUrl = value.image_url
  if (typeof imageUrl === 'string') {
    pushImagePreview(previews, seen, imageUrlToPreview(imageUrl, 'url'))
  } else if (isJsonRecord(imageUrl)) {
    pushImagePreview(previews, seen, imageUrlToPreview(imageUrl.url, 'url'))
    pushImagePreview(previews, seen, base64ImageToPreview(imageUrl.b64_json, imageMimeFromRecord(imageUrl)))
  }

  pushImagePreview(previews, seen, imageUrlToPreview(value.url, 'url'))
  pushImagePreview(previews, seen, base64ImageToPreview(value.b64_json, mime))
  pushImagePreview(previews, seen, base64ImageToPreview(value.data, mime))
  if (value.type === 'image_generation_call') {
    pushImagePreview(previews, seen, base64ImageToPreview(value.result, mime))
  }

  if (depth <= 4) {
    collectImagePreviews(value.source, previews, seen, depth + 1)
  }
}

function pushImagePreview(
  previews: ModelTestImagePreview[],
  seen: Set<string>,
  preview: ModelTestImagePreview | null,
) {
  if (!preview || seen.has(preview.src) || previews.length >= MODEL_TEST_IMAGE_PREVIEW_MAX_ITEMS) {
    return
  }
  seen.add(preview.src)
  previews.push({
    ...preview,
    label: `图片 ${previews.length + 1}`,
  })
}

function imageUrlToPreview(value: unknown, source: 'url'): ModelTestImagePreview | null {
  if (typeof value !== 'string') return null
  const url = value.trim()
  if (!url) return null
  if (url.startsWith('data:image/')) {
    return { src: url, label: 'base64', source: 'base64' }
  }
  if (url.startsWith('/')) {
    return { src: url, label: 'URL', source }
  }

  try {
    const parsed = new URL(url)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return null
  } catch {
    return null
  }

  return { src: url, label: 'URL', source }
}

function base64ImageToPreview(value: unknown, mime: string): ModelTestImagePreview | null {
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  if (!trimmed) return null
  if (trimmed.startsWith('data:image/')) {
    return { src: trimmed, label: 'base64', source: 'base64' }
  }

  const normalized = trimmed.replace(/\s+/g, '')
  if (!normalized) return null
  return {
    src: `data:${mime};base64,${normalized}`,
    label: 'base64',
    source: 'base64',
  }
}

function imageMimeFromRecord(value: JsonRecord): string {
  const outputFormat = value.output_format
  if (typeof outputFormat === 'string') {
    const normalized = outputFormat.trim().toLowerCase()
    if (/^[a-z0-9.+-]+$/.test(normalized)) return `image/${normalized}`
  }

  const raw = [
    value.mime_type,
    value.mime,
    value.media_type,
    value.content_type,
    value.type,
  ].find(candidate => typeof candidate === 'string' && candidate.trim().startsWith('image/'))

  if (typeof raw === 'string') {
    const normalized = raw.trim().toLowerCase()
    if (/^image\/[a-z0-9.+-]+$/.test(normalized)) return normalized
  }
  return 'image/png'
}

function extractResponseReasoning(responseBody: unknown, depth = 0): string | null {
  if (depth > 4 || !isJsonRecord(responseBody)) return null

  const wrappedReasoning = extractResponseReasoning(responseBody.response, depth + 1)
    ?? extractResponseReasoning(responseBody.body, depth + 1)
  if (wrappedReasoning) return wrappedReasoning

  const directReasoning = compactPreviewText(responseBody.reasoning_content)
    ?? compactPreviewText(responseBody.thinking)
  if (directReasoning) return directReasoning

  const topLevelReasoning = extractReasoningFromContentParts(responseBody.content, depth + 1)
  if (topLevelReasoning) return topLevelReasoning

  const choicesReasoning = extractChoicesReasoning(responseBody.choices, depth + 1)
  if (choicesReasoning) return choicesReasoning

  const outputReasoning = extractOutputReasoning(responseBody.output, depth + 1)
  if (outputReasoning) return outputReasoning

  return null
}

function extractChoicesText(value: unknown, depth: number): string | null {
  if (!Array.isArray(value)) return null

  for (const choice of value) {
    if (!isJsonRecord(choice)) continue

    const messageText = isJsonRecord(choice.message)
      ? extractTextFromContentParts(choice.message.content, depth + 1)
      : null
    const deltaText = isJsonRecord(choice.delta)
      ? extractTextFromContentParts(choice.delta.content, depth + 1)
      : null
    const text = messageText ?? deltaText ?? extractTextFromContentParts(choice.text, depth + 1)
    if (text) return text
  }

  return null
}

function extractChoicesReasoning(value: unknown, depth: number): string | null {
  if (!Array.isArray(value)) return null

  for (const choice of value) {
    if (!isJsonRecord(choice)) continue

    const messageReasoning = isJsonRecord(choice.message)
      ? extractReasoningFromMessage(choice.message, depth + 1)
      : null
    const deltaReasoning = isJsonRecord(choice.delta)
      ? extractReasoningFromMessage(choice.delta, depth + 1)
      : null
    const reasoning = messageReasoning ?? deltaReasoning
    if (reasoning) return reasoning
  }

  return null
}

function extractReasoningFromMessage(message: JsonRecord, depth: number): string | null {
  return compactPreviewText(message.reasoning_content)
    ?? compactPreviewText(message.thinking)
    ?? extractReasoningFromContentParts(message.content, depth + 1)
}

function extractOutputText(value: unknown, depth: number): string | null {
  if (!Array.isArray(value)) return null

  for (const outputItem of value) {
    if (!isJsonRecord(outputItem)) continue

    const contentText = extractTextFromContentParts(outputItem.content, depth + 1)
      ?? extractResponseText(outputItem.response, depth + 1)
    if (contentText) return contentText
  }

  return null
}

function extractOutputReasoning(value: unknown, depth: number): string | null {
  if (!Array.isArray(value)) return null

  for (const outputItem of value) {
    if (!isJsonRecord(outputItem)) continue

    const reasoning = extractReasoningFromContentParts(outputItem.content, depth + 1)
      ?? compactPreviewText(outputItem.reasoning_content)
      ?? compactPreviewText(outputItem.thinking)
      ?? extractResponseReasoning(outputItem.response, depth + 1)
    if (reasoning) return reasoning
  }

  return null
}

function extractImagePreviewFromCollection(value: unknown, depth: number): string | null {
  if (!Array.isArray(value)) return null

  for (const item of value) {
    if (!isJsonRecord(item)) continue

    const preview = extractImagePreviewFromRecord(item, depth + 1)
    if (preview) return preview
  }

  return null
}

function extractImagePreviewFromContentParts(value: unknown, depth: number): string | null {
  if (!Array.isArray(value)) return null

  for (const part of value) {
    if (!isJsonRecord(part)) continue

    const preview = extractImagePreviewFromRecord(part, depth + 1)
    if (preview) return preview
  }

  return null
}

function extractImagePreviewFromRecord(value: JsonRecord, depth: number): string | null {
  if (depth > 4) return null

  const imageUrl = value.image_url
  if (typeof imageUrl === 'string' && imageUrl.trim()) {
    return compactPreviewText(`图片：${imageUrl}`)
  }
  if (isJsonRecord(imageUrl)) {
    const nestedUrl = compactPreviewText(imageUrl.url)
    if (nestedUrl) return `图片：${nestedUrl}`
    if (compactPreviewText(imageUrl.b64_json)) {
      return '图片：base64'
    }
  }

  const url = compactPreviewText(value.url)
  if (url) return `图片：${url}`

  if (compactPreviewText(value.b64_json)) {
    return '图片：base64'
  }

  if (value.type === 'image_generation_call' && compactPreviewText(value.result)) {
    return '图片：base64'
  }

  return extractImagePreviewFromCollection(value.data, depth + 1)
    ?? extractImagePreviewFromCollection(value.images, depth + 1)
    ?? extractImagePreviewFromContentParts(value.content, depth + 1)
}

function extractGeminiCandidateText(value: unknown, depth: number): string | null {
  if (!Array.isArray(value)) return null

  for (const candidate of value) {
    if (!isJsonRecord(candidate) || !isJsonRecord(candidate.content)) continue

    const text = extractTextFromContentParts(candidate.content.parts, depth + 1)
    if (text) return text
  }

  return null
}

function extractReasoningFromContentParts(value: unknown, depth = 0): string | null {
  if (depth > 4 || !Array.isArray(value)) return null

  const parts = value.flatMap((part) => {
    if (!isJsonRecord(part)) return []

    const reasoning = compactPreviewText(part.reasoning_content)
      ?? compactPreviewText(part.thinking)
      ?? compactPreviewText(part.reasoning)
      ?? extractReasoningFromContentParts(part.content, depth + 1)
      ?? extractReasoningFromContentParts(part.parts, depth + 1)
    return reasoning ? [reasoning] : []
  })

  return joinPreviewParts(parts)
}

function extractResponseSummary(responseBody: unknown): string | null {
  if (!isJsonRecord(responseBody)) return null

  if (Array.isArray(responseBody.data)) {
    const embeddingDimensions = responseBody.data
      .map(item => isJsonRecord(item) && Array.isArray(item.embedding) ? item.embedding.length : null)
      .find((size): size is number => typeof size === 'number')
    if (embeddingDimensions != null) return `Embedding 维度：${embeddingDimensions}`
    if (responseBody.data.length > 0) return `返回数据：${responseBody.data.length} 条`
  }

  if (Array.isArray(responseBody.results)) return `Rerank 结果：${responseBody.results.length} 条`

  const model = compactPreviewText(responseBody.model)
  if (model) return `返回模型：${model}`

  return null
}
