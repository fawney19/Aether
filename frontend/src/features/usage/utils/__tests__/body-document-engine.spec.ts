import { describe, expect, it } from 'vitest'
import { gzipSync } from 'node:zlib'
import { BodyDocumentEngine, decodeBody } from '../body-document-engine'
import { JSON_PAGE_SIZE, JSON_TEXT_CHUNK_SIZE } from '../json-viewer'

function bytes(value: string) { return new TextEncoder().encode(value).buffer }
function gzip(value: string) { return Uint8Array.from(gzipSync(value)).buffer }

describe('body document decoding', () => {
  it.each(['gzip', 'json'] as const)('decodes %s off the UI protocol with a byte count', async encoding => {
    const text = JSON.stringify({ text: '你好🙂', count: 0, enabled: false })
    const decoded = await decodeBody(encoding === 'gzip' ? gzip(text) : bytes(text), encoding)
    expect(decoded.value).toEqual(JSON.parse(text))
    expect(decoded.byteLength).toBe(bytes(text).byteLength)
  })

  it('enforces decompressed size while streaming, including exact boundaries', async () => {
    const text = JSON.stringify('x'.repeat(100_000))
    await expect(decodeBody(gzip(text), 'gzip', text.length)).resolves.toHaveProperty('byteLength', text.length)
    await expect(decodeBody(gzip(text), 'gzip', text.length - 1)).rejects.toHaveProperty('code', 'too_large')
    await expect(decodeBody(bytes(text), 'json', text.length - 1)).rejects.toHaveProperty('code', 'too_large')
  })

  it('rejects corrupt gzip, invalid JSON and invalid UTF-8 with safe codes', async () => {
    await expect(decodeBody(bytes('not gzip'), 'gzip')).rejects.toHaveProperty('code', 'decode_failed')
    await expect(decodeBody(gzip('not json'), 'gzip')).rejects.toHaveProperty('code', 'decode_failed')
    await expect(decodeBody(new Uint8Array([34, 255, 34]).buffer, 'json')).rejects.toHaveProperty('code', 'decode_failed')
  })
})

describe('worker-owned body previews', () => {
  it('sends bounded rows and strings, while copy preserves every byte of content', () => {
    const value = { messages: Array.from({ length: 6000 }, () => ({ content: 'x'.repeat(4000) })) }
    const document = new BodyDocumentEngine(value)
    const page = document.json({ expandDepth: 999 })
    expect(page.lines).toHaveLength(JSON_PAGE_SIZE)
    expect(page.hasNext).toBe(true)
    expect(page.lines.some(line => line.continuation)).toBe(true)
    expect(page.lines.every(line => (line.tokens ?? []).reduce((length, token) => length + token.text.length, 0) <= JSON_TEXT_CHUNK_SIZE)).toBe(true)
    const next = document.json({ page: 1, expandDepth: 999 })
    expect(new Set([...page.lines, ...next.lines].map(line => line.id)).size).toBe(JSON_PAGE_SIZE * 2)
    expect(JSON.parse(document.copy())).toEqual(value)
  })

  it('keeps collapsed children unread and identifiers small even with giant keys', () => {
    const hidden = new Proxy({}, { ownKeys: () => { throw new Error('must remain lazy') } })
    const document = new BodyDocumentEngine({ parent: { hidden } })
    expect(document.json({ expandDepth: 0 }).lines).toHaveLength(3)
    const key = 'k'.repeat(100_000)
    const value = 'v'.repeat(10_000)
    const giant = new BodyDocumentEngine({ [key]: value })
    const lines = giant.json().lines
    expect(lines.every(line => line.id.length < 40)).toBe(true)
    expect(lines.filter(line => line.lineNumber === 2).flatMap(line => line.tokens ?? []).map(token => token.text).join('')).toBe(`${JSON.stringify(key)}: ${JSON.stringify(value)}`)
    expect(JSON.parse(giant.copy())).toEqual({ [key]: value })
  })

  it('supports smaller scrolling batches without relaxing the worker transfer limit', () => {
    const value = Array.from({ length: 1000 }, (_value, index) => `message-${index}`)
    const document = new BodyDocumentEngine(value)
    expect(document.json({ pageSize: 50 }).lines).toHaveLength(50)
    expect(document.json({ pageSize: 50, page: 1 }).lines[0].lineNumber).toBe(51)
    expect(document.json({ pageSize: 10_000 }).lines).toHaveLength(JSON_PAGE_SIZE)
    expect(document.json({ pageSize: 50, page: 20 }).hasNext).toBe(false)
    expect(JSON.parse(document.copy())).toEqual(value)
  })

  it('streams all raw text and parse-error response chunks without shipping the full string at once', () => {
    const text = 'x'.repeat(100_000)
    for (const value of [text, { raw_response: text, metadata: { parse_error: 'invalid upstream response' } }]) {
      const document = new BodyDocumentEngine(value)
      expect(document.json().text).toHaveLength(16_000)
      expect(document.json().hasNext).toBe(true)
      const chunks = []
      for (let page = 0; page < 10; page += 1) {
        const chunk = document.json({ page })
        chunks.push(chunk.text)
        if (!chunk.hasNext) break
      }
      expect(chunks.join('')).toBe(text)
      expect(JSON.parse(document.copy())).toEqual(value)
    }
  })

  it('paginates conversation blocks and bounds long text without truncating copy', () => {
    const content = 'message content '.repeat(10_000)
    const document = new BodyDocumentEngine({ model: 'test', messages: Array.from({ length: 40 }, () => ({ role: 'user', content })) })
    const options = { kind: 'request' as const, apiFormat: 'openai:chat' }
    const preview = document.conversationPage(options)
    expect(preview.hasNext).toBe(true)
    expect(preview.truncated).toBe(true)
    expect(JSON.stringify(preview).length).toBeLessThan(70_000)
    expect(document.copy(options)).toContain(content)
    expect(document.conversationPage({ ...options, page: 1 }).result.blocks.length).toBeGreaterThan(0)
  })

  it.each([null, false, 0])('keeps the primitive %s as a valid document', value => {
    const document = new BodyDocumentEngine(value)
    expect(document.json().lines[0].value).toBe(value)
    expect(document.copy()).toBe(JSON.stringify(value))
  })
})
