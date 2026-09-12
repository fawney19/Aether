import { describe, expect, it, vi } from 'vitest'
import { getJsonPage, getRawTextChunk, JsonPageReader, JSON_PAGE_SIZE, RAW_TEXT_CHUNK_SIZE } from '../json-viewer'

describe('lazy JSON pages', () => {
  it('does not read descendants of collapsed nodes', () => {
    const readContent = vi.fn(() => { throw new Error('collapsed content must not be read') })
    const message = Object.defineProperty({}, 'largeContent', { enumerable: true, get: readContent })
    const result = getJsonPage({ messages: [message] })
    expect(result.lines).toHaveLength(3)
    expect(result.lines[1]).toMatchObject({ id: '$/messages', collapsed: true, childCount: 1 })
    expect(readContent).not.toHaveBeenCalled()
  })

  it('stops traversal after a page and one lookahead even when everything is expanded', () => {
    const values = new Array(100_000).fill('small value')
    const readBeyondPage = vi.fn(() => { throw new Error('off-page data must not be read') })
    Object.defineProperty(values, JSON_PAGE_SIZE + 1, { get: readBeyondPage })
    const result = getJsonPage(values, { expandDepth: 999 })
    expect(result.lines).toHaveLength(JSON_PAGE_SIZE)
    expect(result.hasNext).toBe(true)
    expect(readBeyondPage).not.toHaveBeenCalled()
  })

  it('keeps complete ordered JSON lines across pages and collision-free node ids', () => {
    const data = { nested: [{ value: 1 }, null, false, 0], 'nested:close': 'text', 'a/b': {}, 'a~1b': [] }
    const expected = getJsonPage(data, { expandDepth: 999, pageSize: 1000 }).lines
    const actual = []
    for (let page = 0; page < 20; page += 1) {
      const result = getJsonPage(data, { expandDepth: 999, pageSize: 3, page })
      actual.push(...result.lines)
      expect(result.lines.length).toBeLessThanOrEqual(3)
      if (!result.hasNext) break
    }
    expect(actual).toEqual(expected)
    expect(new Set(actual.map(line => line.id)).size).toBe(actual.length)
  })

  it('expands only explicitly opened descendants and preserves full string data', () => {
    const content = 'large body '.repeat(10_000)
    const data = { messages: [{ content }] }
    const result = getJsonPage(data, { foldOverrides: new Map([['$/messages', false], ['$/messages/0', false]]) })
    expect(result.lines.find(line => line.key === 'content')?.value).toBe(content)
    expect(result.hasNext).toBe(false)
    expect(data.messages[0].content).toBe(content)
  })

  it('reuses the traversal cursor for consecutive chunks and safely reloads evicted chunks', () => {
    const reads = vi.fn()
    const data = new Proxy(new Array(1000).fill('value'), {
      get(target, key, receiver) {
        if (typeof key === 'string' && /^\d+$/.test(key)) reads(key)
        return Reflect.get(target, key, receiver)
      },
    })
    const reader = new JsonPageReader(data, { pageSize: 50, expandDepth: 999 })
    for (let page = 0; page < 20; page += 1) {
      expect(reader.read(page).lines[0].lineNumber).toBe(page * 50 + 1)
    }
    expect(reads).toHaveBeenCalledTimes(1000)
    expect(reader.read(19).lines[0].lineNumber).toBe(951)
    expect(reads).toHaveBeenCalledTimes(1000)
    expect(reader.read(0).lines[0].lineNumber).toBe(1)
    expect(reader.read(20).lines[1]?.lineNumber).toBe(1002)
    expect(reader.read(20).hasNext).toBe(false)
    expect(reader.read(21).lines).toEqual([])
  })

  it.each([false, true])('opens collapsed descendants one layer at a time with indexPaths=%s', indexPaths => {
    const data = { messages: [{ content: { text: 'complete content' } }] }
    const paths = indexPaths ? ['$/0', '$/0/0', '$/0/0/0'] : ['$/messages', '$/messages/0', '$/messages/0/content']
    const foldOverrides = new Map<string, boolean>()
    for (const path of paths) {
      const folded = getJsonPage(data, { expandDepth: 0, foldOverrides, indexPaths })
      expect(folded.lines.find(line => line.id === path)).toMatchObject({ collapsed: true })
      expect(folded.lines.some(line => line.value === 'complete content')).toBe(false)
      foldOverrides.set(path, false)
    }
    const opened = getJsonPage(data, { expandDepth: 0, foldOverrides, indexPaths })
    expect(opened.lines.some(line => line.value === 'complete content')).toBe(true)
  })

  it('does not read unopened descendants after opening their parent', () => {
    const readContent = vi.fn(() => { throw new Error('unopened content must not be read') })
    const message = Object.defineProperty({}, 'content', { enumerable: true, get: readContent })
    const opened = getJsonPage({ messages: [message] }, { expandDepth: 0, foldOverrides: new Map([['$/messages', false]]) })
    expect(opened.lines.find(line => line.id === '$/messages/0')).toMatchObject({ collapsed: true })
    expect(readContent).not.toHaveBeenCalled()
  })

  it('still expands every layer in expand-all mode while honoring explicitly closed children', () => {
    const data = { messages: [{ content: { text: 'complete content' } }] }
    const expanded = getJsonPage(data, { expandDepth: 999 })
    expect(expanded.lines.some(line => line.value === 'complete content')).toBe(true)
    expect(expanded.lines.filter(line => line.canFold).every(line => !line.collapsed)).toBe(true)
    const folded = getJsonPage(data, { expandDepth: 999, foldOverrides: new Map([['$/messages', false], ['$/messages/0/content', true]]) })
    expect(folded.lines.some(line => line.value === 'complete content')).toBe(false)
  })

  it.each(['中文🙂', '"\\\n\r\t', '\u0000\b\f', '\ud800', '<img src=x>'])('streams complete escaped keys and strings for %j', text => {
    const key = `key-${text.repeat(100)}`
    const value = text.repeat(1000)
    const reader = new JsonPageReader({ [key]: value }, { pageSize: 3, stringChunkSize: 31 })
    const lines = []
    for (let page = 0; page < 10_000; page += 1) {
      const chunk = reader.read(page)
      lines.push(...chunk.lines)
      if (!chunk.hasNext) break
    }
    const tokens = lines.filter(line => line.lineNumber === 2).flatMap(line => line.tokens ?? [])
    expect(tokens.map(token => token.text).join('')).toBe(`${JSON.stringify(key)}: ${JSON.stringify(value)}`)
    expect(new Set(lines.map(line => line.id)).size).toBe(lines.length)
    expect(tokens.every(token => token.text.length <= 31 && !/[\ud800-\udbff]$/.test(token.text))).toBe(true)
    expect(lines.filter(line => line.lineNumber === 2 && !line.continuation)).toHaveLength(1)
  })

  it('keeps raw text Unicode pairs complete across automatic chunk boundaries', () => {
    const value = `${'x'.repeat(RAW_TEXT_CHUNK_SIZE - 1)  }🙂${  '中'.repeat(RAW_TEXT_CHUNK_SIZE)  }RAW-END`
    const first = getRawTextChunk(value)
    const second = getRawTextChunk(value, 1)
    const last = getRawTextChunk(value, 2)
    expect(first.text.endsWith('🙂')).toBe(true)
    expect(first.text + second.text + last.text).toBe(value)
    expect(last.hasNext).toBe(false)
  })
})
