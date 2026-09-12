import { renderRequest, renderResponse, type RenderBlock, type RenderResult } from '../conversation'
import { getRawTextChunk, JsonPageReader, JSON_PAGE_SIZE, JSON_TEXT_CHUNK_SIZE } from './json-viewer'
import {
  BodyDocumentError, MAX_BODY_BYTES, MAX_ENCODED_BODY_BYTES,
  type BodyConversationOptions, type BodyConversationPage, type BodyEncoding,
  type BodyJsonOptions, type BodyJsonPage,
} from './body-document-protocol'

export async function decodeBody(bytes: ArrayBuffer, encoding: BodyEncoding, limit = MAX_BODY_BYTES) {
  if (bytes.byteLength > MAX_ENCODED_BODY_BYTES) throw new BodyDocumentError('too_large')
  if (encoding === 'gzip' && typeof globalThis.DecompressionStream === 'undefined') {
    throw new BodyDocumentError('unsupported')
  }
  const source = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new Uint8Array(bytes))
      controller.close()
    },
  })
  const stream = encoding === 'gzip' ? source.pipeThrough(new DecompressionStream('gzip')) : source
  const reader = stream.getReader()
  const decoder = new TextDecoder('utf-8', { fatal: true })
  const parts: string[] = []
  let byteLength = 0
  try {
    while (true) {
      const chunk = await reader.read()
      if (chunk.done) break
      byteLength += chunk.value.byteLength
      if (byteLength > limit) throw new BodyDocumentError('too_large')
      parts.push(decoder.decode(chunk.value, { stream: true }))
    }
    parts.push(decoder.decode())
    return { value: JSON.parse(parts.join('')) as unknown, byteLength }
  } catch (error) {
    await reader.cancel().catch(() => undefined)
    if (error instanceof BodyDocumentError) throw error
    throw new BodyDocumentError('decode_failed')
  } finally {
    reader.releaseLock()
  }
}

export class BodyDocumentEngine {
  private conversation?: { key: string, result: RenderResult }
  private jsonReader?: { key: string, reader: JsonPageReader }

  constructor(private readonly value: unknown) {}

  json(options: BodyJsonOptions = {}): BodyJsonPage {
    const record = this.value && typeof this.value === 'object' ? this.value as Record<string, unknown> : null
    const metadata = record?.metadata as Record<string, unknown> | undefined
    const parseError = record?.raw_response && metadata?.parse_error ? String(metadata.parse_error) : undefined
    const text = typeof this.value === 'string' ? this.value : parseError ? String(record?.raw_response) : undefined
    if (text !== undefined) {
      return { lines: [], ...getRawTextChunk(text, options.page), parseError: parseError?.slice(0, 2000) }
    }
    const pageSize = Math.min(JSON_PAGE_SIZE, Math.max(1, Math.trunc(options.pageSize ?? JSON_PAGE_SIZE)))
    const key = JSON.stringify([pageSize, options.expandDepth, [...(options.foldOverrides ?? [])]])
    if (this.jsonReader?.key !== key) {
      this.jsonReader = { key, reader: new JsonPageReader(this.value, { ...options, pageSize, indexPaths: true, stringChunkSize: JSON_TEXT_CHUNK_SIZE }) }
    }
    return this.jsonReader.reader.read(options.page)
  }

  private render(options: BodyConversationOptions): RenderResult {
    const key = `${options.kind}:${options.apiFormat ?? ''}`
    if (this.conversation?.key !== key) {
      this.conversation = { key, result: options.kind === 'request'
        ? renderRequest(this.value, undefined, options.apiFormat)
        : renderResponse(this.value, undefined, options.apiFormat) }
    }
    return this.conversation.result
  }

  conversationPage(options: BodyConversationOptions): BodyConversationPage {
    const result = this.render(options)
    const first = Math.max(0, options.page ?? 0) * 10
    let remaining = Math.min(Math.max(options.previewLimit ?? 64_000, 1000), 1_024_000)
    let remainingBlocks = 200
    let truncated = false
    const clipString = (value: string) => {
      const preview = value.slice(0, remaining)
      remaining -= preview.length
      if (preview.length === value.length) return value
      truncated = true
      return `${preview}…（内容较长，复制可获取完整内容）`
    }
    const clipBlocks = (blocks: RenderBlock[], depth = 0): RenderBlock[] => {
      const previews: RenderBlock[] = []
      for (const block of blocks) {
        if (remainingBlocks <= 0 || remaining <= 0 || depth > 30) { truncated = true; break }
        remainingBlocks -= 1
        const preview = { ...block }
        for (const key of Object.keys(preview)) {
          const record = preview as unknown as Record<string, unknown>
          const value = record[key]
          if (typeof value === 'string' && key !== 'type' && key !== 'role') {
            if (key === 'src' && value.length > remaining) {
              record[key] = undefined
              record.alt = '图片较大，请复制正文查看完整内容'
              truncated = true
            } else {
              record[key] = clipString(value)
            }
          } else if (Array.isArray(value)) {
            record[key] = clipBlocks(value, depth + 1)
          }
        }
        previews.push(preview)
      }
      return previews
    }
    return {
      result: { blocks: clipBlocks(result.blocks.slice(first, first + 10)), isStream: result.isStream, error: result.error?.slice(0, 2000) },
      hasNext: result.blocks.length > first + 10,
      truncated,
    }
  }

  copy(conversation?: BodyConversationOptions): string {
    if (!conversation) return JSON.stringify(this.value, null, 2)
    const result = this.render(conversation)
    if (result.error) return `[Error] ${result.error}`
    return result.blocks.map(formatBlockAsText).filter(Boolean).join('\n\n---\n\n')
  }
}

function formatBlockAsText(block: RenderBlock): string {
  switch (block.type) {
    case 'text': return block.content
    case 'code': return `\`\`\`${block.language || ''}\n${block.code}\n\`\`\``
    case 'collapsible': return `[${block.title}]\n${block.content.map(formatBlockAsText).filter(Boolean).join('\n')}`
    case 'error': return `[Error${block.code ? `: ${block.code}` : ''}] ${block.message}`
    case 'image': return `[Image: ${block.mimeType || block.alt || 'unknown'}]`
    case 'tool_use': return `[Tool: ${block.toolName}]\n${block.input}`
    case 'tool_result': return `[Tool Result${block.isError ? ' (Error)' : ''}]\n${block.content}`
    case 'message': return `[${block.roleLabel || block.role}]\n${block.content.map(formatBlockAsText).filter(Boolean).join('\n\n')}`
    case 'container': return block.children.map(formatBlockAsText).filter(Boolean).join('\n')
    case 'label': return `${block.label}: ${block.value}`
    case 'divider': return '---'
    case 'badge': return ''
  }
}
