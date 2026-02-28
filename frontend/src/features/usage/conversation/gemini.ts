/**
 * Gemini API 格式解析器
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

/**
 * Gemini API 格式解析器
 */
export class GeminiParser implements ApiFormatParser {
  readonly format = 'gemini' as const
  readonly displayName = 'Gemini'

  /**
   * 检测是否为 Gemini 格式
   */
  detect(requestBody: unknown, responseBody: unknown, hint?: string): number {
    // 1. 后端提示优先
    if (hint) {
      const lowerHint = hint.toLowerCase()
      if (lowerHint.includes('gemini')) return 100
      if (lowerHint.includes('claude') || lowerHint.includes('openai')) return 0
    }

    const req = requestBody as RawObject | null | undefined

    // 2. 检查模型名
    const model = (typeof req?.model === 'string' ? req.model : '').toLowerCase()
    if (model.includes('gemini')) return 95

    // 3. Gemini 特有结构: 使用 contents 而非 messages
    if (req?.contents && Array.isArray(req.contents)) {
      return 90
    }

    // 4. 检查响应体特征
    const respBody = (isStreamResponse(responseBody)
      ? (responseBody.chunks?.[0] as RawObject | undefined)
      : responseBody) as RawObject | null | undefined

    if (respBody?.candidates) {
      return 85
    }

    return 0
  }

  /**
   * 解析请求体
   */
  parseRequest(requestBody: unknown): ParsedConversation {
    if (!requestBody) {
      return createEmptyConversation('gemini', '无请求体')
    }

    try {
      const body = requestBody as RawObject
      const result: ParsedConversation = {
        messages: [],
        isStream: false,
        apiFormat: 'gemini',
        model: typeof body.model === 'string' ? body.model : undefined,
      }

      // 提取 system instruction
      const sysInst = (body.system_instruction || body.systemInstruction) as RawObject | undefined
      if (sysInst?.parts && Array.isArray(sysInst.parts)) {
        result.system = (sysInst.parts as RawObject[])
          .filter((p: RawObject) => typeof p.text === 'string')
          .map((p: RawObject) => String(p.text))
          .join('\n')
      }

      // 提取 contents
      if (Array.isArray(body.contents)) {
        for (const content of body.contents) {
          const parsedMsg = this.parseContent(content as RawObject)
          if (parsedMsg) {
            result.messages.push(parsedMsg)
          }
        }
      }

      return result
    } catch (e) {
      return createEmptyConversation('gemini', `解析失败: ${e}`)
    }
  }

  /**
   * 解析响应体
   */
  parseResponse(responseBody: unknown): ParsedConversation {
    if (!responseBody) {
      return createEmptyConversation('gemini', '无响应体')
    }

    try {
      const body = responseBody as RawObject
      const result: ParsedConversation = {
        messages: [],
        isStream: false,
        apiFormat: 'gemini',
      }

      // Gemini 原生响应格式: { candidates: [{ content: { parts: [...] } }] }
      const candidateParts = this.getCandidateParts(body)
      if (candidateParts.length > 0) {
        const contentBlocks = this.parseParts(candidateParts)
        if (contentBlocks.length > 0) {
          result.messages.push(createMessage('assistant', contentBlocks))
          return result
        }
      }

      // 统一响应格式（例如 status/output/output_text）
      const unifiedMessages = this.parseUnifiedOutputMessages(body)
      if (unifiedMessages.length > 0) {
        result.messages.push(...unifiedMessages)
      }

      return result
    } catch (e) {
      return createEmptyConversation('gemini', `解析失败: ${e}`)
    }
  }

  /**
   * 解析流式响应
   */
  parseStreamResponse(chunks: unknown[]): ParsedConversation {
    if (!chunks || chunks.length === 0) {
      return createEmptyConversation('gemini', '无响应数据')
    }

    try {
      const result: ParsedConversation = {
        messages: [],
        isStream: true,
        apiFormat: 'gemini',
      }

      const textParts: string[] = []
      const toolCalls: { name: string; args: Record<string, unknown> }[] = []

      for (const rawChunk of chunks) {
        const chunk = rawChunk as RawObject
        const candidates = chunk.candidates as RawObject[] | undefined
        const firstCandidate = candidates?.[0] as RawObject | undefined
        const candidateContent = firstCandidate?.content as RawObject | undefined
        const parts = candidateContent?.parts as RawObject[] | undefined
        if (parts) {
          for (const part of parts) {
            if (typeof part.text === 'string') {
              textParts.push(part.text)
            } else if (part.functionCall) {
              const fc = part.functionCall as RawObject
              toolCalls.push({
                name: String(fc.name || ''),
                args: (fc.args as Record<string, unknown>) || {},
              })
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
      for (const call of toolCalls) {
        contentBlocks.push(createToolUseBlock(
          '', // Gemini 没有 tool_use_id
          call.name || '',
          call.args || {}
        ))
      }

      if (contentBlocks.length > 0) {
        result.messages.push(createMessage('assistant', contentBlocks))
      }

      return result
    } catch (e) {
      return createEmptyConversation('gemini', `解析失败: ${e}`)
    }
  }

  /**
   * 解析 content 对象
   */
  private parseContent(content: RawObject): ParsedMessage | null {
    if (!content) return null

    const role = this.mapRole(typeof content.role === 'string' ? content.role : undefined)
    const parts = Array.isArray(content.parts) ? content.parts as RawObject[] : []
    const contentBlocks = this.parseParts(parts)

    if (contentBlocks.length === 0) return null

    return createMessage(role, contentBlocks)
  }

  /**
   * 解析 parts 数组
   */
  private parseParts(parts: RawObject[]): ContentBlock[] {
    const result: ContentBlock[] = []

    for (const part of parts) {
      const block = this.parsePart(part)
      if (block) {
        result.push(block)
      }
    }

    return result
  }

  /**
   * 解析单个 part
   */
  private parsePart(part: RawObject): ContentBlock | null {
    if (!part) return null

    // 文本
    if (part.text !== undefined) {
      return createTextBlock(String(part.text))
    }

    // 内联数据（图片等）
    if (part.inlineData) {
      const inlineData = part.inlineData as RawObject
      return createImageBlock('base64', {
        data: typeof inlineData.data === 'string' ? inlineData.data : undefined,
        mimeType: typeof inlineData.mimeType === 'string' ? inlineData.mimeType : undefined,
      })
    }

    // 函数调用
    if (part.functionCall) {
      const fc = part.functionCall as RawObject
      return createToolUseBlock(
        '',
        String(fc.name || ''),
        (fc.args as Record<string, unknown>) || {}
      )
    }

    // 函数响应
    if (part.functionResponse) {
      const fr = part.functionResponse as RawObject
      return createToolResultBlock(
        '', // Gemini 用 name 关联
        JSON.stringify(fr.response, null, 2)
      )
    }

    return null
  }

  /**
   * 映射角色
   */
  private mapRole(role: string | undefined): MessageRole {
    switch (role) {
      case 'user':
        return 'user'
      case 'model':
        return 'assistant'
      default:
        return 'user'
    }
  }

  // ============================================================
  // 渲染方法
  // ============================================================

  /**
   * 渲染请求体
   */
  renderRequest(requestBody: unknown): RenderResult {
    if (!requestBody) {
      return createEmptyRenderResult('无请求体')
    }

    try {
      const body = requestBody as RawObject
      const blocks: RenderBlock[] = []

      // 渲染 system instruction
      const sysInst = (body.system_instruction || body.systemInstruction) as RawObject | undefined
      if (sysInst?.parts && Array.isArray(sysInst.parts)) {
        const systemText = (sysInst.parts as RawObject[])
          .filter((p: RawObject) => typeof p.text === 'string')
          .map((p: RawObject) => String(p.text))
          .join('\n')
        if (systemText) {
          blocks.push(createMessageBlock('system', [
            createTextRenderBlock(systemText),
          ], { roleLabel: 'System' }))
        }
      }

      // 渲染 contents
      if (Array.isArray(body.contents)) {
        for (const content of body.contents) {
          const msgBlock = this.renderContent(content as RawObject)
          if (msgBlock) {
            blocks.push(msgBlock)
          }
        }
      }

      return { blocks, isStream: false }
    } catch (e) {
      return createEmptyRenderResult(`渲染失败: ${e}`)
    }
  }

  /**
   * 渲染响应体
   */
  renderResponse(responseBody: unknown): RenderResult {
    if (!responseBody) {
      return createEmptyRenderResult('无响应体')
    }

    // 检查是否为流式响应
    if (isStreamResponse(responseBody)) {
      return this.renderStreamResponse(responseBody.chunks || [])
    }

    try {
      const body = responseBody as RawObject
      const blocks: RenderBlock[] = []

      // Gemini 原生响应格式: { candidates: [{ content: { parts: [...] } }] }
      const candidateParts = this.getCandidateParts(body)
      if (candidateParts.length > 0) {
        const parts = candidateParts
        const contentBlocks = this.renderParts(parts)
        if (contentBlocks.length > 0) {
          const badges = this.getBadgesForParts(parts)
          blocks.push(createMessageBlock('assistant', contentBlocks, {
            roleLabel: 'Assistant',
            badges: badges.length > 0 ? badges : undefined,
          }))
          return { blocks, isStream: false }
        }
      }

      // 统一响应格式（例如 status/output/output_text）
      blocks.push(...this.renderUnifiedOutputMessages(body))

      return { blocks, isStream: false }
    } catch (e) {
      return createEmptyRenderResult(`渲染失败: ${e}`)
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
   * 渲染 content 对象
   */
  private renderContent(content: RawObject): RenderBlock | null {
    if (!content) return null

    const role = this.mapRole(typeof content.role === 'string' ? content.role : undefined)
    const parts = Array.isArray(content.parts) ? content.parts as RawObject[] : []
    const contentBlocks = this.renderParts(parts)

    if (contentBlocks.length === 0) return null

    const badges = this.getBadgesForParts(parts)

    return createMessageBlock(role, contentBlocks, {
      roleLabel: this.getRoleLabel(role),
      badges: badges.length > 0 ? badges : undefined,
    })
  }

  /**
   * 渲染 parts 数组
   */
  private renderParts(parts: RawObject[]): RenderBlock[] {
    const result: RenderBlock[] = []

    for (const part of parts) {
      const rendered = this.renderPart(part)
      if (rendered) {
        result.push(rendered)
      }
    }

    return result
  }

  /**
   * 渲染单个 part
   */
  private renderPart(part: RawObject): RenderBlock | null {
    if (!part) return null

    // 文本
    if (part.text !== undefined) {
      return createTextRenderBlock(String(part.text))
    }

    // 内联数据（图片等）
    if (part.inlineData) {
      const inlineData = part.inlineData as RawObject
      return createImageRenderBlock({
        src: `data:${inlineData.mimeType || 'image/png'};base64,${inlineData.data}`,
        mimeType: typeof inlineData.mimeType === 'string' ? inlineData.mimeType : undefined,
      })
    }

    // 函数调用
    if (part.functionCall) {
      const fc = part.functionCall as RawObject
      return createToolUseRenderBlock(
        String(fc.name || '函数调用'),
        this.formatJson(fc.args)
      )
    }

    // 函数响应
    if (part.functionResponse) {
      const fr = part.functionResponse as RawObject
      return createToolResultRenderBlock(
        this.formatJson(fr.response)
      )
    }

    return null
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
          block.toolName || '函数调用',
          this.formatJson(block.input)
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
      case 'assistant': return 'Model'
      case 'system': return 'System'
      case 'tool': return 'Tool'
      default: return role
    }
  }

  /**
   * 获取 parts 的徽章
   */
  private getBadgesForParts(parts: RawObject[]): BadgeRenderBlock[] {
    const badges: BadgeRenderBlock[] = []
    const hasImage = parts.some((p: RawObject) => p.inlineData)
    const hasToolCall = parts.some((p: RawObject) => p.functionCall)
    const hasToolResult = parts.some((p: RawObject) => p.functionResponse)

    if (hasToolCall) {
      badges.push(createBadgeBlock('函数调用', 'outline'))
    }
    if (hasToolResult) {
      badges.push(createBadgeBlock('函数结果', 'outline'))
    }
    if (hasImage) {
      badges.push(createBadgeBlock('图片', 'secondary'))
    }

    return badges
  }

  /**
   * 提取 Gemini candidates 中的 parts
   */
  private getCandidateParts(body: RawObject): RawObject[] {
    const candidates = body.candidates as RawObject[] | undefined
    const candidate = candidates?.[0] as RawObject | undefined
    const candidateContent = candidate?.content as RawObject | undefined
    if (Array.isArray(candidateContent?.parts)) {
      return candidateContent.parts as RawObject[]
    }
    return []
  }

  /**
   * 解析统一响应格式中的 output 消息
   */
  private parseUnifiedOutputMessages(body: RawObject): ParsedMessage[] {
    const output = body.output
    if (!Array.isArray(output)) {
      return []
    }

    const messages: ParsedMessage[] = []
    for (const rawItem of output) {
      const item = rawItem as RawObject
      if (item?.type !== 'message') {
        continue
      }

      const texts = this.extractUnifiedOutputTexts(item.content)
      if (texts.length === 0) {
        continue
      }

      messages.push(createMessage(
        this.mapUnifiedOutputRole(item.role),
        texts.map(text => createTextBlock(text))
      ))
    }

    return messages
  }

  /**
   * 渲染统一响应格式中的 output 消息
   */
  private renderUnifiedOutputMessages(body: RawObject): RenderBlock[] {
    const output = body.output
    if (!Array.isArray(output)) {
      return []
    }

    const blocks: RenderBlock[] = []
    for (const rawItem of output) {
      const item = rawItem as RawObject
      if (item?.type !== 'message') {
        continue
      }

      const texts = this.extractUnifiedOutputTexts(item.content)
      if (texts.length === 0) {
        continue
      }

      const role = this.mapUnifiedOutputRole(item.role)
      blocks.push(createMessageBlock(
        role,
        texts.map(text => createTextRenderBlock(text)),
        { roleLabel: this.getRoleLabel(role) }
      ))
    }

    return blocks
  }

  /**
   * 提取统一响应格式的文本内容
   */
  private extractUnifiedOutputTexts(content: unknown): string[] {
    if (typeof content === 'string') {
      return content ? [content] : []
    }

    if (!Array.isArray(content)) {
      return []
    }

    const texts: string[] = []
    for (const rawPart of content) {
      const part = rawPart as RawObject
      if (
        (part.type === 'output_text' || part.type === 'text' || part.type === 'input_text') &&
        typeof part.text === 'string'
      ) {
        texts.push(part.text)
      }
    }
    return texts
  }

  /**
   * 映射统一 output 结构的角色
   */
  private mapUnifiedOutputRole(role: unknown): MessageRole {
    if (role === 'assistant' || role === 'model') return 'assistant'
    if (role === 'user') return 'user'
    if (role === 'system') return 'system'
    if (role === 'tool') return 'tool'
    return 'assistant'
  }

  /**
   * 获取已解析内容的徽章
   */
  private getBadgesForParsedContent(content: ContentBlock[]): BadgeRenderBlock[] {
    const badges: BadgeRenderBlock[] = []
    const types = new Set(content.map(b => b.type))

    if (types.has('tool_use')) {
      badges.push(createBadgeBlock('函数调用', 'outline'))
    }
    if (types.has('tool_result')) {
      badges.push(createBadgeBlock('函数结果', 'outline'))
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
      try {
        const parsed = JSON.parse(input) as unknown
        return JSON.stringify(parsed, null, 2)
      } catch {
        return input
      }
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
export const geminiParser = new GeminiParser()
