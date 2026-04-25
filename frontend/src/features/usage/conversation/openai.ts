/**
 * OpenAI API 格式解析器
 */

import type {
  ApiFormatParser,
  ParsedConversation,
  ParsedMessage,
  ContentBlock,
  MessageRole,
} from './types'
import {
  createEmptyConversation,
  createMessage,
  createTextBlock,
  createToolUseBlock,
  createToolResultBlock,
  createImageBlock,
  isStreamResponse,
} from './types'
import type { RenderResult, RenderBlock, BadgeRenderBlock } from './render'
import {
  createTextBlock as createTextRenderBlock,
  createBadgeBlock,
  createImageBlock as createImageRenderBlock,
  createMessageBlock,
  createToolUseBlock as createToolUseRenderBlock,
  createToolResultBlock as createToolResultRenderBlock,
  createEmptyRenderResult,
} from './render'

/** Raw JSON object from API (loosely typed) */
type RawObject = Record<string, unknown>

type JsonParseResult =
  | { ok: true; value: unknown }
  | { ok: false }

const HTML_ENTITY_MAP: Record<string, string> = {
  amp: '&',
  apos: "'",
  gt: '>',
  lt: '<',
  nbsp: '\u00A0',
  quot: '"',
}

const parseJsonString = (input: string): JsonParseResult => {
  try {
    return { ok: true, value: JSON.parse(input) as unknown }
  } catch {
    return { ok: false }
  }
}

const decodeHtmlEntityToken = (entity: string): string => {
  const normalized = entity.toLowerCase()
  const named = HTML_ENTITY_MAP[normalized]
  if (named !== undefined) {
    return named
  }

  if (normalized.startsWith('#x')) {
    const codePoint = Number.parseInt(normalized.slice(2), 16)
    if (Number.isFinite(codePoint) && codePoint >= 0 && codePoint <= 0x10FFFF) {
      return String.fromCodePoint(codePoint)
    }
  }

  if (normalized.startsWith('#')) {
    const codePoint = Number.parseInt(normalized.slice(1), 10)
    if (Number.isFinite(codePoint) && codePoint >= 0 && codePoint <= 0x10FFFF) {
      return String.fromCodePoint(codePoint)
    }
  }

  return `&${entity};`
}

const decodeHtmlEntities = (input: string): string => {
  let decoded = input
  for (let pass = 0; pass < 3; pass += 1) {
    const next = decoded.replace(/&(#x[0-9a-f]+|#\d+|[a-z][a-z0-9]+);/gi, (_match, entity: string) => decodeHtmlEntityToken(entity))
    if (next === decoded) {
      break
    }
    decoded = next
  }
  return decoded
}

/**
 * OpenAI API 格式解析器
 */
export class OpenAIParser implements ApiFormatParser {
  readonly format = 'openai' as const
  readonly displayName = 'OpenAI'

  /**
   * 检测是否为 OpenAI 格式（包括 Chat Completions 和 CLI/Responses API）
   */
  detect(requestBody: unknown, responseBody: unknown, hint?: string): number {
    // 1. 后端提示优先
    if (hint) {
      const lowerHint = hint.toLowerCase()
      if (lowerHint.includes('openai')) return 100
      if (lowerHint.includes('claude') || lowerHint.includes('gemini')) return 0
    }

    const req = requestBody as RawObject | null | undefined

    // 2. 检查模型名
    const model = (typeof req?.model === 'string' ? req.model : '').toLowerCase()
    if (model.includes('gpt') || model.includes('o1') || model.includes('o3')) return 95

    // 3. 检查请求体结构
    // OpenAI CLI (Responses API) 使用 input 字段
    const isCliFormat = req?.input !== undefined || req?.instructions !== undefined
    // OpenAI Chat Completions 使用 messages 数组
    const isChatFormat = req?.messages && Array.isArray(req.messages)

    if (!isCliFormat && !isChatFormat) {
      return 0
    }

    // 4. 检查响应体特征
    const respBody = (isStreamResponse(responseBody)
      ? (responseBody.chunks?.[0] as RawObject | undefined)
      : responseBody) as RawObject | null | undefined

    if (respBody) {
      // OpenAI CLI 响应特征: type 字段为 response.* 格式
      if (this.isCliResponseEvent(respBody)) {
        return 95
      }
      // OpenAI Chat Completions 响应特征: choices 数组
      const respObject = typeof respBody.object === 'string' ? respBody.object : ''
      if (respBody.choices || respObject.includes('chat.completion')) {
        return 90
      }
      // 明确是 Claude 格式
      const respType = typeof respBody.type === 'string' ? respBody.type : ''
      if (respType === 'message' || respType.startsWith('content_block')) {
        return 0
      }
    }

    // 5. 检查 OpenAI 特有的请求结构
    if (isCliFormat) {
      return 80
    }

    // OpenAI 的 system 是在 messages 数组中作为 role: system
    const messages = req?.messages as RawObject[] | undefined
    const hasSystemInMessages = messages?.some(
      (m: RawObject) => m.role === 'system'
    )
    if (hasSystemInMessages) {
      return 60
    }

    return 0
  }

  /**
   * 检查是否为 OpenAI CLI (Responses API) 的响应事件
   */
  private isCliResponseEvent(chunk: RawObject | null | undefined): boolean {
    const type = chunk?.type
    if (typeof type !== 'string') return false
    return type.startsWith('response.') || chunk?.object === 'response'
  }

  /**
   * 解析请求体（支持 Chat Completions 和 CLI/Responses API 格式）
   */
  parseRequest(requestBody: unknown): ParsedConversation {
    if (!requestBody) {
      return createEmptyConversation('openai', '无请求体')
    }

    const body = requestBody as RawObject

    // 检测是否为 CLI 格式
    const isCliFormat = body.input !== undefined || body.instructions !== undefined

    if (isCliFormat) {
      return this.parseCliRequest(body)
    }

    return this.parseChatRequest(body)
  }

  /**
   * 解析 OpenAI Chat Completions 请求
   */
  private parseChatRequest(requestBody: RawObject): ParsedConversation {
    try {
      const result: ParsedConversation = {
        messages: [],
        isStream: requestBody.stream === true,
        apiFormat: 'openai',
        model: typeof requestBody.model === 'string' ? requestBody.model : undefined,
      }

      if (Array.isArray(requestBody.messages)) {
        for (const rawMsg of requestBody.messages) {
          const msg = rawMsg as RawObject
          // OpenAI 的 system 消息在 messages 数组中
          if (msg.role === 'system') {
            const systemText = typeof msg.content === 'string'
              ? msg.content
              : ''
            result.system = result.system
              ? `${result.system  }\n${  systemText}`
              : systemText
            continue
          }

          const parsedMsg = this.parseMessage(msg)
          if (parsedMsg) {
            result.messages.push(parsedMsg)
          }
        }
      }

      return result
    } catch (e) {
      return createEmptyConversation('openai', `解析失败: ${e}`)
    }
  }

  /**
   * 解析 OpenAI CLI (Responses API) 请求
   *
   * CLI 格式特点：
   * - 使用 input 字段（可以是字符串、消息数组或对象）
   * - 使用 instructions 字段作为系统指令
   */
  private parseCliRequest(requestBody: RawObject): ParsedConversation {
    try {
      const result: ParsedConversation = {
        messages: [],
        isStream: requestBody.stream === true,
        apiFormat: 'openai',
        model: typeof requestBody.model === 'string' ? requestBody.model : undefined,
      }

      // 处理 instructions（系统指令）
      if (typeof requestBody.instructions === 'string') {
        result.system = requestBody.instructions
      }

      // 处理 input
      const input = requestBody.input

      if (typeof input === 'string') {
        // 简单字符串输入
        result.messages.push(createMessage('user', [createTextBlock(input)]))
      } else if (Array.isArray(input)) {
        // 消息数组
        for (const item of input) {
          const parsedMsg = this.parseCliInputItem(item as RawObject)
          if (parsedMsg) {
            result.messages.push(parsedMsg)
          }
        }
      } else if (input && typeof input === 'object') {
        const inputObj = input as RawObject
        // 包装在对象中的消息数组
        if (Array.isArray(inputObj.messages)) {
          for (const item of inputObj.messages) {
            const parsedMsg = this.parseCliInputItem(item as RawObject)
            if (parsedMsg) {
              result.messages.push(parsedMsg)
            }
          }
        }
      }

      return result
    } catch (e) {
      return createEmptyConversation('openai', `CLI 格式解析失败: ${e}`)
    }
  }

  /**
   * 解析 CLI 格式的单个输入项
   */
  private parseCliInputItem(item: RawObject): ParsedMessage | null {
    if (!item) return null

    const itemType = item.type

    // 标准消息（有 role 字段）
    if (itemType === 'message' || item.role) {
      const role = this.mapRole(String(item.role || ''))
      const contentBlocks: ContentBlock[] = []

      const content = item.content
      if (typeof content === 'string') {
        contentBlocks.push(createTextBlock(content))
      } else if (Array.isArray(content)) {
        for (const rawPart of content) {
          const part = rawPart as RawObject
          if (part.type === 'input_text' || part.type === 'output_text' || part.type === 'text') {
            contentBlocks.push(createTextBlock(String(part.text || '')))
          }
        }
      }

      if (contentBlocks.length === 0) return null
      return createMessage(role, contentBlocks)
    }

    // function_call -> 工具调用
    if (itemType === 'function_call') {
      const toolId = String(item.call_id || item.id || '')
      const toolName = String(item.name || '')
      const args = String(item.arguments || '{}')
      return createMessage('assistant', [createToolUseBlock(toolId, toolName, args)])
    }

    // function_call_output -> 工具结果
    if (itemType === 'function_call_output') {
      const toolUseId = String(item.call_id || item.id || '')
      const output = typeof item.output === 'string'
        ? item.output
        : JSON.stringify(item.output, null, 2)
      return createMessage('tool', [createToolResultBlock(toolUseId, output)])
    }

    return null
  }

  /**
   * 解析响应体（支持 Chat Completions 和 CLI/Responses API 格式）
   */
  parseResponse(responseBody: unknown): ParsedConversation {
    if (!responseBody) {
      return createEmptyConversation('openai', '无响应体')
    }

    const body = responseBody as RawObject

    // 检测是否为 CLI 格式
    const isCliFormat = this.isCliResponseEvent(body) ||
      body.object === 'response' ||
      body.output !== undefined

    if (isCliFormat) {
      return this.parseCliResponse(body)
    }

    return this.parseChatResponse(body)
  }

  /**
   * 解析 OpenAI Chat Completions 响应
   */
  private parseChatResponse(responseBody: RawObject): ParsedConversation {
    try {
      const result: ParsedConversation = {
        messages: [],
        isStream: false,
        apiFormat: 'openai',
        model: typeof responseBody.model === 'string' ? responseBody.model : undefined,
      }

      // OpenAI 响应格式: { choices: [{ message: { role, content, tool_calls } }] }
      const choices = responseBody.choices as RawObject[] | undefined
      const firstChoice = choices?.[0] as RawObject | undefined
      const message = firstChoice?.message as RawObject | undefined
      if (message) {
        const contentBlocks: ContentBlock[] = []

        // 文本内容
        if (typeof message.content === 'string') {
          contentBlocks.push(createTextBlock(message.content))
        }

        // 工具调用
        if (Array.isArray(message.tool_calls)) {
          for (const rawCall of message.tool_calls) {
            const call = rawCall as RawObject
            const fn = call.function as RawObject | undefined
            contentBlocks.push(createToolUseBlock(
              String(call.id || ''),
              String(fn?.name || ''),
              String(fn?.arguments || '{}')
            ))
          }
        }

        if (contentBlocks.length > 0) {
          result.messages.push(createMessage('assistant', contentBlocks))
        }
      }

      return result
    } catch (e) {
      return createEmptyConversation('openai', `解析失败: ${e}`)
    }
  }

  /**
   * 解析 OpenAI CLI (Responses API) 响应
   *
   * CLI 响应格式: { output: [{ type: "message", content: [...] }] }
   */
  private parseCliResponse(responseBody: RawObject): ParsedConversation {
    try {
      const result: ParsedConversation = {
        messages: [],
        isStream: false,
        apiFormat: 'openai',
        model: typeof responseBody.model === 'string' ? responseBody.model : undefined,
      }

      const output = responseBody.output
      if (!Array.isArray(output)) {
        return result
      }

      for (const rawItem of output) {
        const item = rawItem as RawObject
        if (item?.type === 'message') {
          const contentBlocks: ContentBlock[] = []

          if (Array.isArray(item.content)) {
            for (const rawContent of item.content) {
              const content = rawContent as RawObject
              if (content?.type === 'output_text' && typeof content?.text === 'string') {
                contentBlocks.push(createTextBlock(content.text))
              }
            }
          }

          if (contentBlocks.length > 0) {
            result.messages.push(createMessage('assistant', contentBlocks))
          }
        }
      }

      return result
    } catch (e) {
      return createEmptyConversation('openai', `CLI 格式解析失败: ${e}`)
    }
  }

  /**
   * 解析流式响应（支持 Chat Completions 和 CLI/Responses API 格式）
   */
  parseStreamResponse(chunks: unknown[]): ParsedConversation {
    if (!chunks || chunks.length === 0) {
      return createEmptyConversation('openai', '无响应数据')
    }

    // 检测是否为 CLI 格式
    const isCliFormat = chunks.some(chunk => this.isCliResponseEvent(chunk as RawObject))

    if (isCliFormat) {
      return this.parseCliStreamResponse(chunks)
    }

    return this.parseChatStreamResponse(chunks)
  }

  /**
   * 解析 OpenAI Chat Completions 流式响应
   */
  private parseChatStreamResponse(chunks: unknown[]): ParsedConversation {
    try {
      const result: ParsedConversation = {
        messages: [],
        isStream: true,
        apiFormat: 'openai',
      }

      const textParts: string[] = []
      const toolCalls = new Map<number, { name: string; id: string; args: string[] }>()

      for (const rawChunk of chunks) {
        const chunk = rawChunk as RawObject
        // 提取模型名
        if (typeof chunk.model === 'string' && !result.model) {
          result.model = chunk.model
        }

        const choices = chunk.choices as RawObject[] | undefined
        const firstChoice = choices?.[0] as RawObject | undefined
        const delta = firstChoice?.delta as RawObject | undefined
        if (typeof delta?.content === 'string') {
          textParts.push(delta.content)
        }
        if (Array.isArray(delta?.tool_calls)) {
          for (const rawCall of delta.tool_calls as unknown[]) {
            const call = rawCall as RawObject
            const fn = call.function as RawObject | undefined
            const index = (typeof call.index === 'number' ? call.index : 0)
            if (!toolCalls.has(index)) {
              toolCalls.set(index, {
                name: String(fn?.name || ''),
                id: String(call.id || ''),
                args: [],
              })
            }
            const existing = toolCalls.get(index)
            if (existing) {
              if (typeof fn?.name === 'string') {
                existing.name = fn.name
              }
              if (typeof call.id === 'string') {
                existing.id = call.id
              }
              if (typeof fn?.arguments === 'string') {
                existing.args.push(fn.arguments)
              }
            }
          }
        }
      }

      const contentBlocks: ContentBlock[] = []

      // 文本内容
      if (textParts.length > 0) {
        contentBlocks.push(createTextBlock(textParts.join('')))
      }

      // 工具调用
      for (const [, call] of toolCalls) {
        contentBlocks.push(createToolUseBlock(
          call.id,
          call.name,
          call.args.join('')
        ))
      }

      if (contentBlocks.length > 0) {
        result.messages.push(createMessage('assistant', contentBlocks))
      }

      return result
    } catch (e) {
      return createEmptyConversation('openai', `解析失败: ${e}`)
    }
  }

  /**
   * 解析 OpenAI CLI (Responses API) 流式响应
   *
   * 支持的事件类型：
   * - response.created: 响应创建
   * - response.output_text.delta: 文本增量
   * - response.completed: 响应完成（包含完整响应和 usage）
   * - response.function_call_arguments.delta: 函数调用参数增量
   */
  private parseCliStreamResponse(chunks: unknown[]): ParsedConversation {
    try {
      const result: ParsedConversation = {
        messages: [],
        isStream: true,
        apiFormat: 'openai',
      }

      const textParts: string[] = []
      const toolCalls = new Map<string, { name: string; id: string; args: string[] }>()
      let currentToolId = ''
      let currentToolName = ''

      for (const rawChunk of chunks) {
        const chunk = rawChunk as RawObject
        const eventType = chunk.type

        // 从 response.created 或 response.completed 提取模型名
        if (!result.model) {
          const response = chunk.response as RawObject | undefined
          if (typeof response?.model === 'string') {
            result.model = response.model
          }
        }

        // 处理文本增量: response.output_text.delta
        // 兼容旧别名 response.outtext.delta
        if (eventType === 'response.output_text.delta' || eventType === 'response.outtext.delta') {
          const delta = chunk.delta
          if (typeof delta === 'string') {
            textParts.push(delta)
          } else if (delta && typeof delta === 'object') {
            const deltaObj = delta as RawObject
            if (typeof deltaObj.text === 'string') {
              textParts.push(deltaObj.text)
            }
          }
          continue
        }

        // 处理函数调用输出项添加: response.output_item.added
        if (eventType === 'response.output_item.added') {
          const item = chunk.item as RawObject | undefined
          if (item?.type === 'function_call') {
            currentToolId = String(item.call_id || item.id || '')
            currentToolName = String(item.name || '')
            if (currentToolId && !toolCalls.has(currentToolId)) {
              toolCalls.set(currentToolId, {
                name: currentToolName,
                id: currentToolId,
                args: [],
              })
            }
          }
          continue
        }

        // 处理函数调用参数增量: response.function_call_arguments.delta
        if (eventType === 'response.function_call_arguments.delta') {
          const delta = chunk.delta
          if (typeof delta === 'string' && currentToolId && toolCalls.has(currentToolId)) {
            toolCalls.get(currentToolId)?.args.push(delta)
          }
          continue
        }

        // 处理完成事件: response.completed
        // 如果之前没有收集到文本，从完成事件中提取
        if (eventType === 'response.completed') {
          const response = chunk.response as RawObject | undefined
          if (typeof response?.model === 'string' && !result.model) {
            result.model = response.model
          }

          // 从 output 中提取文本（备用方案）
          if (textParts.length === 0 && Array.isArray(response?.output)) {
            for (const rawItem of response.output as unknown[]) {
              const item = rawItem as RawObject
              if (item?.type === 'message' && Array.isArray(item?.content)) {
                for (const rawContent of item.content as unknown[]) {
                  const content = rawContent as RawObject
                  if (content?.type === 'output_text' && typeof content?.text === 'string') {
                    textParts.push(content.text)
                  }
                }
              }
            }
          }
          continue
        }
      }

      const contentBlocks: ContentBlock[] = []

      // 文本内容
      if (textParts.length > 0) {
        contentBlocks.push(createTextBlock(textParts.join('')))
      }

      // 工具调用
      for (const [, call] of toolCalls) {
        contentBlocks.push(createToolUseBlock(
          call.id,
          call.name,
          call.args.join('')
        ))
      }

      if (contentBlocks.length > 0) {
        result.messages.push(createMessage('assistant', contentBlocks))
      }

      return result
    } catch (e) {
      return createEmptyConversation('openai', `CLI 格式解析失败: ${e}`)
    }
  }

  /**
   * 解析单条消息
   */
  private parseMessage(msg: RawObject): ParsedMessage | null {
    if (!msg || !msg.role) return null

    const role = this.mapRole(String(msg.role))
    const contentBlocks: ContentBlock[] = []

    // 文本内容
    if (typeof msg.content === 'string') {
      contentBlocks.push(createTextBlock(msg.content))
    } else if (Array.isArray(msg.content)) {
      // Vision API 格式
      for (const rawPart of msg.content) {
        const part = rawPart as RawObject
        if (part.type === 'text') {
          contentBlocks.push(createTextBlock(String(part.text || '')))
        } else if (part.type === 'image_url') {
          const imageUrl = part.image_url as RawObject | undefined
          contentBlocks.push(createImageBlock('url', {
            url: typeof imageUrl?.url === 'string' ? imageUrl.url : undefined,
            alt: '[图片]',
          }))
        }
      }
    }

    // 工具调用（assistant 消息）
    if (Array.isArray(msg.tool_calls)) {
      for (const rawCall of msg.tool_calls) {
        const call = rawCall as RawObject
        const fn = call.function as RawObject | undefined
        contentBlocks.push(createToolUseBlock(
          String(call.id || ''),
          String(fn?.name || ''),
          String(fn?.arguments || '{}')
        ))
      }
    }

    // 工具结果（tool 消息）
    if (msg.tool_call_id) {
      const content = typeof msg.content === 'string'
        ? msg.content
        : JSON.stringify(msg.content, null, 2)
      contentBlocks.push(createToolResultBlock(
        String(msg.tool_call_id),
        content
      ))
    }

    if (contentBlocks.length === 0) return null

    return createMessage(role, contentBlocks)
  }

  /**
   * 映射角色
   */
  private mapRole(role: string): MessageRole {
    switch (role) {
      case 'user':
        return 'user'
      case 'assistant':
        return 'assistant'
      case 'system':
        return 'system'
      case 'tool':
        return 'tool'
      default:
        return 'user'
    }
  }

  // ============================================================
  // 渲染方法
  // ============================================================

  /**
   * 渲染请求体（支持 Chat Completions 和 CLI/Responses API 格式）
   */
  renderRequest(requestBody: unknown): RenderResult {
    if (!requestBody) {
      return createEmptyRenderResult('无请求体')
    }

    const body = requestBody as RawObject

    // 检测是否为 CLI 格式
    const isCliFormat = body.input !== undefined || body.instructions !== undefined

    if (isCliFormat) {
      return this.renderCliRequest(body)
    }

    return this.renderChatRequest(body)
  }

  /**
   * 渲染 OpenAI Chat Completions 请求
   */
  private renderChatRequest(requestBody: RawObject): RenderResult {
    try {
      const blocks: RenderBlock[] = []
      const isStream = requestBody.stream === true

      if (Array.isArray(requestBody.messages)) {
        for (const rawMsg of requestBody.messages) {
          const msg = rawMsg as RawObject
          // system 消息单独处理
          if (msg.role === 'system') {
            const systemText = typeof msg.content === 'string' ? msg.content : ''
            if (systemText) {
              blocks.push(createMessageBlock('system', [
                createTextRenderBlock(systemText),
              ], { roleLabel: 'System' }))
            }
            continue
          }

          const msgBlock = this.renderMessage(msg)
          if (msgBlock) {
            blocks.push(msgBlock)
          }
        }
      }

      return { blocks, isStream }
    } catch (e) {
      return createEmptyRenderResult(`渲染失败: ${e}`)
    }
  }

  /**
   * 渲染 OpenAI CLI (Responses API) 请求
   */
  private renderCliRequest(requestBody: RawObject): RenderResult {
    try {
      const blocks: RenderBlock[] = []
      const isStream = requestBody.stream === true

      // 渲染 instructions（系统指令）
      if (typeof requestBody.instructions === 'string') {
        blocks.push(createMessageBlock('system', [
          createTextRenderBlock(requestBody.instructions),
        ], { roleLabel: 'Instructions' }))
      }

      // 渲染 input
      const input = requestBody.input

      if (typeof input === 'string') {
        // 简单字符串输入
        blocks.push(createMessageBlock('user', [
          createTextRenderBlock(input),
        ], { roleLabel: 'User' }))
      } else if (Array.isArray(input)) {
        // 消息数组
        for (const item of input) {
          const msgBlock = this.renderCliInputItem(item as RawObject)
          if (msgBlock) {
            blocks.push(msgBlock)
          }
        }
      } else if (input && typeof input === 'object') {
        const inputObj = input as RawObject
        // 包装在对象中的消息数组
        if (Array.isArray(inputObj.messages)) {
          for (const item of inputObj.messages) {
            const msgBlock = this.renderCliInputItem(item as RawObject)
            if (msgBlock) {
              blocks.push(msgBlock)
            }
          }
        }
      }

      return { blocks, isStream }
    } catch (e) {
      return createEmptyRenderResult(`CLI 格式渲染失败: ${e}`)
    }
  }

  /**
   * 渲染 CLI 格式的单个输入项
   */
  private renderCliInputItem(item: RawObject): RenderBlock | null {
    if (!item) return null

    const itemType = item.type

    // 标准消息
    if (itemType === 'message' || item.role) {
      const role = this.mapRole(String(item.role || ''))
      const contentBlocks: RenderBlock[] = []

      const content = item.content
      if (typeof content === 'string') {
        contentBlocks.push(createTextRenderBlock(content))
      } else if (Array.isArray(content)) {
        for (const rawPart of content) {
          const part = rawPart as RawObject
          if (part.type === 'input_text' || part.type === 'output_text' || part.type === 'text') {
            contentBlocks.push(createTextRenderBlock(String(part.text || '')))
          }
        }
      }

      if (contentBlocks.length === 0) return null
      return createMessageBlock(role, contentBlocks, { roleLabel: this.getRoleLabel(role) })
    }

    // function_call -> 工具调用
    if (itemType === 'function_call') {
      const toolName = String(item.name || '工具调用')
      const args = this.formatJson(item.arguments)
      return createMessageBlock('assistant', [
        createToolUseRenderBlock(toolName, args, String(item.call_id || item.id || '')),
      ], { roleLabel: 'Assistant', badges: [createBadgeBlock('工具调用', 'outline')] })
    }

    // function_call_output -> 工具结果
    if (itemType === 'function_call_output') {
      const output = typeof item.output === 'string'
        ? item.output
        : JSON.stringify(item.output, null, 2)
      return createMessageBlock('tool', [
        createToolResultRenderBlock(output),
      ], { roleLabel: 'Tool', badges: [createBadgeBlock('工具结果', 'outline')] })
    }

    return null
  }

  /**
   * 渲染响应体（支持 Chat Completions 和 CLI/Responses API 格式）
   */
  renderResponse(responseBody: unknown): RenderResult {
    if (!responseBody) {
      return createEmptyRenderResult('无响应体')
    }

    // 检查是否为流式响应
    if (isStreamResponse(responseBody)) {
      return this.renderStreamResponse(responseBody.chunks || [])
    }

    const body = responseBody as RawObject

    // 检测是否为 CLI 格式
    const isCliFormat = this.isCliResponseEvent(body) ||
      body.object === 'response' ||
      body.output !== undefined

    if (isCliFormat) {
      return this.renderCliResponse(body)
    }

    return this.renderChatResponse(body)
  }

  /**
   * 渲染 OpenAI Chat Completions 响应
   */
  private renderChatResponse(responseBody: RawObject): RenderResult {
    try {
      const blocks: RenderBlock[] = []

      // OpenAI 响应格式: { choices: [{ message: { role, content, tool_calls } }] }
      const choices = responseBody.choices as RawObject[] | undefined
      const firstChoice = choices?.[0] as RawObject | undefined
      const message = firstChoice?.message as RawObject | undefined
      if (message) {
        const contentBlocks: RenderBlock[] = []
        const badges: BadgeRenderBlock[] = []

        // 文本内容
        if (typeof message.content === 'string') {
          contentBlocks.push(createTextRenderBlock(message.content))
        }

        // 工具调用
        if (Array.isArray(message.tool_calls)) {
          badges.push(createBadgeBlock('工具调用', 'outline'))
          for (const rawCall of message.tool_calls) {
            const call = rawCall as RawObject
            const fn = call.function as RawObject | undefined
            contentBlocks.push(createToolUseRenderBlock(
              String(fn?.name || '工具调用'),
              this.formatJson(fn?.arguments),
              typeof call.id === 'string' ? call.id : undefined
            ))
          }
        }

        if (contentBlocks.length > 0) {
          blocks.push(createMessageBlock('assistant', contentBlocks, {
            roleLabel: 'Assistant',
            badges: badges.length > 0 ? badges : undefined,
          }))
        }
      }

      return { blocks, isStream: false }
    } catch (e) {
      return createEmptyRenderResult(`渲染失败: ${e}`)
    }
  }

  /**
   * 渲染 OpenAI CLI (Responses API) 响应
   */
  private renderCliResponse(responseBody: RawObject): RenderResult {
    try {
      const blocks: RenderBlock[] = []

      const output = responseBody.output
      if (!Array.isArray(output)) {
        return { blocks, isStream: false }
      }

      for (const rawItem of output) {
        const item = rawItem as RawObject
        if (item?.type === 'message') {
          const contentBlocks: RenderBlock[] = []

          if (Array.isArray(item.content)) {
            for (const rawContent of item.content) {
              const content = rawContent as RawObject
              if (content?.type === 'output_text' && typeof content?.text === 'string') {
                contentBlocks.push(createTextRenderBlock(content.text))
              }
            }
          }

          if (contentBlocks.length > 0) {
            blocks.push(createMessageBlock('assistant', contentBlocks, {
              roleLabel: 'Assistant',
            }))
          }
        }
      }

      return { blocks, isStream: false }
    } catch (e) {
      return createEmptyRenderResult(`CLI 格式渲染失败: ${e}`)
    }
  }

  /**
   * 渲染流式响应
   */
  private renderStreamResponse(chunks: unknown[]): RenderResult {
    if (!chunks || chunks.length === 0) {
      return createEmptyRenderResult('无响应数据')
    }

    try {
      // 先解析流式响应
      const parsed = this.parseStreamResponse(chunks)
      if (parsed.parseError) {
        return createEmptyRenderResult(parsed.parseError)
      }

      const blocks: RenderBlock[] = []

      // 渲染解析后的消息
      for (const msg of parsed.messages) {
        const contentBlocks = this.renderParsedContentBlocks(msg.content)
        if (contentBlocks.length > 0) {
          const badges = this.getBadgesForParsedContent(msg.content)
          blocks.push(createMessageBlock(msg.role, contentBlocks, {
            roleLabel: this.getRoleLabel(msg.role),
            badges: badges.length > 0 ? badges : undefined,
          }))
        }
      }

      return { blocks, isStream: true }
    } catch (e) {
      return createEmptyRenderResult(`渲染失败: ${e}`)
    }
  }

  /**
   * 渲染单条消息
   */
  private renderMessage(msg: RawObject): RenderBlock | null {
    if (!msg || !msg.role) return null

    const role = this.mapRole(String(msg.role))
    const contentBlocks: RenderBlock[] = []
    const badges: BadgeRenderBlock[] = []

    // 文本内容
    if (typeof msg.content === 'string') {
      contentBlocks.push(createTextRenderBlock(msg.content))
    } else if (Array.isArray(msg.content)) {
      // Vision API 格式
      for (const rawPart of msg.content) {
        const part = rawPart as RawObject
        if (part.type === 'text') {
          contentBlocks.push(createTextRenderBlock(String(part.text || '')))
        } else if (part.type === 'image_url') {
          badges.push(createBadgeBlock('图片', 'secondary'))
          const imageUrl = part.image_url as RawObject | undefined
          contentBlocks.push(createImageRenderBlock({
            src: typeof imageUrl?.url === 'string' ? imageUrl.url : undefined,
            alt: '[图片]',
          }))
        }
      }
    }

    // 工具调用（assistant 消息）
    if (Array.isArray(msg.tool_calls)) {
      badges.push(createBadgeBlock('工具调用', 'outline'))
      for (const rawCall of msg.tool_calls) {
        const call = rawCall as RawObject
        const fn = call.function as RawObject | undefined
        contentBlocks.push(createToolUseRenderBlock(
          String(fn?.name || '工具调用'),
          this.formatJson(fn?.arguments),
          typeof call.id === 'string' ? call.id : undefined
        ))
      }
    }

    // 工具结果（tool 消息）
    if (msg.tool_call_id) {
      badges.push(createBadgeBlock('工具结果', 'outline'))
      const content = typeof msg.content === 'string'
        ? msg.content
        : JSON.stringify(msg.content, null, 2)
      contentBlocks.push(createToolResultRenderBlock(content))
    }

    if (contentBlocks.length === 0) return null

    return createMessageBlock(role, contentBlocks, {
      roleLabel: this.getRoleLabel(role),
      badges: badges.length > 0 ? badges : undefined,
    })
  }

  /**
   * 渲染已解析的内容块数组
   */
  private renderParsedContentBlocks(blocks: ContentBlock[]): RenderBlock[] {
    const result: RenderBlock[] = []

    for (const block of blocks) {
      const rendered = this.renderParsedContentBlock(block)
      if (rendered) {
        result.push(rendered)
      }
    }

    return result
  }

  /**
   * 渲染单个已解析的内容块
   */
  private renderParsedContentBlock(block: ContentBlock): RenderBlock | null {
    switch (block.type) {
      case 'text':
        return createTextRenderBlock(block.text)

      case 'tool_use':
        return createToolUseRenderBlock(
          block.toolName || '工具调用',
          this.formatJson(block.input),
          block.toolId
        )

      case 'tool_result': {
        const content = typeof block.content === 'string'
          ? block.content
          : this.formatParsedToolResultContent(block.content)
        return createToolResultRenderBlock(content, block.isError)
      }

      case 'image':
        return createImageRenderBlock({
          src: block.sourceType === 'base64'
            ? `data:${block.mimeType || 'image/png'};base64,${block.data}`
            : block.url,
          mimeType: block.mimeType,
          alt: block.alt || '图片',
        })

      default:
        return null
    }
  }

  /**
   * 获取角色显示标签
   */
  private getRoleLabel(role: MessageRole): string {
    switch (role) {
      case 'user': return 'User'
      case 'assistant': return 'Assistant'
      case 'system': return 'System'
      case 'tool': return 'Tool'
      default: return role
    }
  }

  /**
   * 获取已解析内容的徽章
   */
  private getBadgesForParsedContent(content: ContentBlock[]): BadgeRenderBlock[] {
    const badges: BadgeRenderBlock[] = []
    const types = new Set(content.map(b => b.type))

    if (types.has('tool_use')) {
      badges.push(createBadgeBlock('工具调用', 'outline'))
    }
    if (types.has('tool_result')) {
      badges.push(createBadgeBlock('工具结果', 'outline'))
    }
    if (types.has('image')) {
      badges.push(createBadgeBlock('图片', 'secondary'))
    }

    return badges
  }

  /**
   * 格式化 JSON
   */
  private formatJson(input: unknown): string {
    if (typeof input === 'string') {
      const parsed = parseJsonString(input)
      if (parsed.ok) {
        return JSON.stringify(parsed.value, null, 2)
      }

      const decoded = decodeHtmlEntities(input)
      if (decoded !== input) {
        const parsedDecoded = parseJsonString(decoded)
        if (parsedDecoded.ok) {
          return JSON.stringify(parsedDecoded.value, null, 2)
        }
        return decoded
      }

      return input
    }
    return JSON.stringify(input, null, 2)
  }

  /**
   * 格式化已解析的工具结果内容
   */
  private formatParsedToolResultContent(content: ContentBlock[]): string {
    return content
      .map(block => {
        if (block.type === 'text') return block.text
        if (block.type === 'image') return '[图片]'
        if (block.type === 'error') return `[错误: ${block.message}]`
        return ''
      })
      .filter(Boolean)
      .join('\n')
  }
}

/** 单例实例 */
export const openaiParser = new OpenAIParser()
