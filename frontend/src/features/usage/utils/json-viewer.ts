export const JSON_PAGE_SIZE = 200
export const JSON_SCROLL_CHUNK_SIZE = 50
export const JSON_TEXT_CHUNK_SIZE = 2000
export const RAW_TEXT_CHUNK_SIZE = 16_000

export interface JsonDisplayToken {
  text: string
  type: string
}

export interface JsonDisplayLine {
  id: string
  lineNumber: number
  indent: number
  key?: string
  value?: unknown
  bracket?: string
  closingBracket?: string
  comma: string
  canFold: boolean
  collapsed: boolean
  childCount?: number
  isArray?: boolean
  tokens?: JsonDisplayToken[]
  continuation?: boolean
}

export interface JsonPageOptions {
  page?: number
  pageSize?: number
  expandDepth?: number
  foldOverrides?: ReadonlyMap<string, boolean>
  indexPaths?: boolean
  stringChunkSize?: number
}

type JsonTreeLine = Omit<JsonDisplayLine, 'lineNumber'>

function splitsSurrogatePair(value: string, position: number) {
  const previous = value.charCodeAt(position - 1)
  const next = value.charCodeAt(position)
  return previous >= 0xd800 && previous <= 0xdbff && next >= 0xdc00 && next <= 0xdfff
}

export function getRawTextChunk(value: string, index = 0) {
  const offset = Math.max(0, Math.trunc(index)) * RAW_TEXT_CHUNK_SIZE
  const boundary = Math.min(offset + RAW_TEXT_CHUNK_SIZE, value.length)
  const start = offset + (splitsSurrogatePair(value, offset) ? 1 : 0)
  const end = boundary + (splitsSurrogatePair(value, boundary) ? 1 : 0)
  return { text: value.slice(start, end), hasNext: end < value.length }
}

function* quotedTokens(value: string, type: string, chunkSize: number): Generator<JsonDisplayToken> {
  yield { text: '"', type }
  let offset = 0
  while (offset < value.length) {
    let end = Math.min(offset + chunkSize, value.length)
    if (splitsSurrogatePair(value, end)) end -= 1
    yield { text: JSON.stringify(value.slice(offset, end)).slice(1, -1), type }
    offset = end
  }
  yield { text: '"', type }
}

function* lineTokens(line: JsonTreeLine, chunkSize: number): Generator<JsonDisplayToken> {
  if (line.key !== undefined) {
    yield* quotedTokens(line.key, 'key', chunkSize)
    yield { text: ': ', type: 'punctuation' }
  }
  if (line.bracket) {
    yield { text: line.bracket, type: 'bracket' }
    if (line.collapsed) {
      if (line.childCount) yield { text: '...', type: 'ellipsis' }
      yield { text: line.closingBracket ?? '', type: 'bracket' }
      yield { text: line.comma, type: 'punctuation' }
      if (line.childCount) yield { text: `${line.childCount} ${line.isArray ? 'items' : 'keys'}`, type: 'info' }
    } else if (!line.canFold) {
      yield { text: line.comma, type: 'punctuation' }
    }
  } else {
    if (typeof line.value === 'string') yield* quotedTokens(line.value, 'string', chunkSize)
    else yield { text: String(line.value), type: line.value === null ? 'null' : typeof line.value }
    yield { text: line.comma, type: 'punctuation' }
  }
}

function* splitJsonLine(line: JsonTreeLine, chunkSize: number): Generator<JsonTreeLine> {
  if ((line.key?.length ?? 0) + (typeof line.value === 'string' ? line.value.length : 0) + 10 <= chunkSize) {
    yield line
    return
  }
  let tokens: JsonDisplayToken[] = []
  let length = 0
  let part = 0
  const fragment = (): JsonTreeLine => ({
    ...line,
    id: part === 0 ? line.id : `fragment:${line.id}:${part}`,
    key: undefined,
    value: undefined,
    tokens,
    continuation: part > 0,
    canFold: part === 0 && line.canFold,
  })
  for (const token of lineTokens(line, chunkSize)) {
    let offset = 0
    while (offset < token.text.length) {
      let end = Math.min(offset + chunkSize - length, token.text.length)
      if (splitsSurrogatePair(token.text, end)) end -= 1
      if (end === offset) {
        yield fragment()
        tokens = []
        length = 0
        part += 1
        continue
      }
      tokens.push({ type: token.type, text: token.text.slice(offset, end) })
      length += end - offset
      offset = end
      if (length === chunkSize) {
        yield fragment()
        tokens = []
        length = 0
        part += 1
      }
    }
  }
  if (tokens.length) yield fragment()
}

function* walkJsonLines(data: unknown, options: JsonPageOptions): Generator<JsonDisplayLine> {
  const depth = options.expandDepth === 0 || options.expandDepth == null ? 1 : options.expandDepth

  function* walk(value: unknown, path: string, indent: number, comma: string, key?: string): Generator<JsonTreeLine> {
    if (value == null || typeof value !== 'object') {
      yield { id: path, indent, key, value, comma, canFold: false, collapsed: false }
      return
    }

    const isArray = Array.isArray(value)
    const keys = isArray ? [] : Object.keys(value)
    const childCount = isArray ? value.length : keys.length
    const override = options.foldOverrides?.get(path)
    const collapsed = childCount === 0 || (override ?? (indent >= depth))
    const bracket = isArray ? '[' : '{'
    const closingBracket = isArray ? ']' : '}'
    yield { id: path, indent, key, comma, bracket, closingBracket, childCount, isArray, collapsed, canFold: childCount > 0 }
    if (collapsed) return

    for (let index = 0; index < childCount; index += 1) {
      const childKey = isArray ? String(index) : keys[index]
      const childPath = `${path}/${options.indexPaths ? index : childKey.replace(/~/g, '~0').replace(/\//g, '~1')}`
      const childValue = isArray ? value[index] : (value as Record<string, unknown>)[childKey]
      yield* walk(childValue, childPath, indent + 1, index === childCount - 1 ? '' : ',', isArray ? undefined : childKey)
    }
    yield { id: `close:${path}`, indent, bracket: closingBracket, comma, canFold: false, collapsed: false }
  }

  let lineNumber = 0
  for (const line of walk(data, '$', 0, '')) {
    lineNumber += 1
    if (options.stringChunkSize) {
      for (const fragment of splitJsonLine(line, Math.max(2, options.stringChunkSize))) yield { ...fragment, lineNumber }
    } else {
      yield { ...line, lineNumber }
    }
  }
}

export class JsonPageReader {
  private readonly pageSize: number
  private iterator: Generator<JsonDisplayLine>
  private nextLine: IteratorResult<JsonDisplayLine>
  private nextPage = 0
  private readonly cache = new Map<number, { lines: JsonDisplayLine[], hasNext: boolean }>()

  constructor(private readonly data: unknown, private readonly options: JsonPageOptions = {}) {
    this.pageSize = Math.max(1, Math.trunc(options.pageSize ?? JSON_PAGE_SIZE))
    this.iterator = walkJsonLines(data, options)
    this.nextLine = this.iterator.next()
  }

  read(page = 0): { lines: JsonDisplayLine[], hasNext: boolean } {
    const requested = Math.max(0, Math.trunc(page))
    const cached = this.cache.get(requested)
    if (cached) return cached
    if (requested < this.nextPage) {
      this.iterator = walkJsonLines(this.data, this.options)
      this.nextLine = this.iterator.next()
      this.nextPage = 0
    }
    while (this.nextPage <= requested) {
      const lines: JsonDisplayLine[] = []
      while (lines.length < this.pageSize && !this.nextLine.done) {
        lines.push(this.nextLine.value)
        this.nextLine = this.iterator.next()
      }
      const result = { lines, hasNext: !this.nextLine.done }
      this.cache.delete(this.nextPage)
      this.cache.set(this.nextPage, result)
      const oldest = this.cache.keys().next().value
      if (this.cache.size > 6 && oldest !== undefined) this.cache.delete(oldest)
      this.nextPage += 1
      if (this.nextPage > requested) return result
      if (!result.hasNext) return { lines: [], hasNext: false }
    }
    return { lines: [], hasNext: false }
  }
}

export function getJsonPage(data: unknown, options: JsonPageOptions = {}) {
  return new JsonPageReader(data, options).read(options.page)
}
