import type { Plugin } from 'vite'

const cjkPattern = /[\u4e00-\u9fff]/
const helperName = '__aetherLegacyT'
const helperImportName = '__useAetherI18n'

const skipTags = new Set(['script', 'style', 'code', 'pre', 'kbd', 'samp', 'textarea'])
const voidTags = new Set([
  'area',
  'base',
  'br',
  'col',
  'embed',
  'hr',
  'img',
  'input',
  'link',
  'meta',
  'param',
  'source',
  'track',
  'wbr',
])

const translatableAttributeNames = new Set([
  'alt',
  'aria-label',
  'cancel-text',
  'client-label',
  'confirm-text',
  'description',
  'drop-title',
  'empty-message',
  'empty-text',
  'entity-label',
  'filter-title',
  'label',
  'manual-placeholder',
  'message',
  'path-hint',
  'placeholder',
  'provider-label',
  'search-placeholder',
  'subtitle',
  'title',
])

interface TemplateTransformResult {
  code: string
  changed: boolean
  needsHelper: boolean
}

interface TagInfo {
  closing: boolean
  name: string
  selfClosing: boolean
  skipSubtree: boolean
}

interface TagStackEntry {
  name: string
  skip: boolean
}

function toExpressionString(value: string): string {
  return JSON.stringify(value).replace(/'/g, "\\'")
}

function wrapExpression(expression: string): string {
  const trimmed = expression.trim()
  if (!trimmed || trimmed.includes(helperName)) {
    return expression
  }

  return `${helperName}(${trimmed})`
}

function renderTranslatedText(value: string): string {
  return `{{ ${helperName}(${toExpressionString(value)}) }}`
}

function findTagEnd(source: string, start: number): number {
  let quote: string | null = null

  for (let index = start; index < source.length; index++) {
    const char = source[index]

    if (quote) {
      if (char === quote) {
        quote = null
      }
      continue
    }

    if (char === '"' || char === "'") {
      quote = char
      continue
    }

    if (char === '>') {
      return index
    }
  }

  return -1
}

function looksLikeTagStart(source: string, index: number): boolean {
  const next = source[index + 1]
  return !!next && /[A-Za-z!/]/.test(next)
}

function parseTagInfo(tag: string): TagInfo | null {
  if (tag.startsWith('<!--') || tag.startsWith('<!') || tag.startsWith('<?')) {
    return null
  }

  const match = tag.match(/^<\s*(\/)?\s*([A-Za-z][A-Za-z0-9:._-]*)/)
  if (!match) {
    return null
  }

  const name = match[2].toLowerCase()
  const closing = !!match[1]
  const selfClosing = closing ? false : /\/\s*>$/.test(tag) || voidTags.has(name)
  const skipSubtree = !closing && (skipTags.has(name) || /\sv-pre(?:[\s=>]|$)/.test(tag))

  return {
    closing,
    name,
    selfClosing,
    skipSubtree,
  }
}

function isTranslatableAttribute(attributeName: string): boolean {
  const normalized = attributeName
    .replace(/^:/, '')
    .replace(/^v-bind:/, '')
    .split('.')[0]
    .toLowerCase()

  return translatableAttributeNames.has(normalized)
}

function isBoundAttribute(attributeName: string): boolean {
  return attributeName.startsWith(':') || attributeName.startsWith('v-bind:')
}

function transformTagAttributes(tag: string): TemplateTransformResult {
  let changed = false
  let needsHelper = false
  const attributePattern = /(\s)([:@]?[A-Za-z_][\w:.-]*)(\s*=\s*)(["'])([\s\S]*?)\4/g

  const code = tag.replace(
    attributePattern,
    (fullMatch, prefix: string, attributeName: string, equals: string, quote: string, value: string) => {
      if (!isTranslatableAttribute(attributeName) || attributeName.startsWith('@')) {
        return fullMatch
      }

      if (isBoundAttribute(attributeName)) {
        const wrapped = wrapExpression(value)
        if (wrapped === value) {
          return fullMatch
        }

        changed = true
        needsHelper = true
        return `${prefix}${attributeName}${equals}${quote}${wrapped}${quote}`
      }

      if (!cjkPattern.test(value)) {
        return fullMatch
      }

      changed = true
      needsHelper = true
      return `${prefix}:${attributeName}='${helperName}(${toExpressionString(value)})'`
    },
  )

  return { code, changed, needsHelper }
}

function transformTextSegment(segment: string): TemplateTransformResult {
  if (!segment) {
    return { code: segment, changed: false, needsHelper: false }
  }

  let changed = false
  let needsHelper = false
  let cursor = 0
  let code = ''
  const interpolationPattern = /\{\{([\s\S]*?)\}\}/g
  let match: RegExpExecArray | null

  while ((match = interpolationPattern.exec(segment))) {
    const staticText = segment.slice(cursor, match.index)
    if (cjkPattern.test(staticText)) {
      code += renderTranslatedText(staticText)
      changed = true
      needsHelper = true
    } else {
      code += staticText
    }

    const expression = match[1]
    const wrapped = wrapExpression(expression)
    code += `{{ ${wrapped} }}`
    if (wrapped !== expression) {
      changed = true
      needsHelper = true
    }

    cursor = match.index + match[0].length
  }

  const tail = segment.slice(cursor)
  if (cjkPattern.test(tail)) {
    code += renderTranslatedText(tail)
    changed = true
    needsHelper = true
  } else {
    code += tail
  }

  return changed ? { code, changed, needsHelper } : { code: segment, changed: false, needsHelper: false }
}

function closeTag(stack: TagStackEntry[], tagName: string): void {
  const index = stack.findLastIndex(entry => entry.name === tagName)
  if (index >= 0) {
    stack.splice(index)
  }
}

function isInsideSkippedTag(stack: TagStackEntry[]): boolean {
  return stack.some(entry => entry.skip)
}

export function transformLegacyTemplateI18n(template: string): TemplateTransformResult {
  let code = ''
  let changed = false
  let needsHelper = false
  let cursor = 0
  const stack: TagStackEntry[] = []

  while (cursor < template.length) {
    if (template.startsWith('<!--', cursor)) {
      const end = template.indexOf('-->', cursor + 4)
      const nextCursor = end >= 0 ? end + 3 : template.length
      code += template.slice(cursor, nextCursor)
      cursor = nextCursor
      continue
    }

    if (template[cursor] !== '<' || !looksLikeTagStart(template, cursor)) {
      const nextTag = template.indexOf('<', cursor + 1)
      const nextCursor = nextTag >= 0 ? nextTag : template.length
      const segment = template.slice(cursor, nextCursor)

      if (isInsideSkippedTag(stack)) {
        code += segment
      } else {
        const transformed = transformTextSegment(segment)
        code += transformed.code
        changed = changed || transformed.changed
        needsHelper = needsHelper || transformed.needsHelper
      }

      cursor = nextCursor
      continue
    }

    const tagEnd = findTagEnd(template, cursor)
    if (tagEnd < 0) {
      code += template.slice(cursor)
      break
    }

    const rawTag = template.slice(cursor, tagEnd + 1)
    const info = parseTagInfo(rawTag)
    const shouldTransformAttributes = !!info && !info.closing && !isInsideSkippedTag(stack) && !info.skipSubtree

    if (shouldTransformAttributes) {
      const transformed = transformTagAttributes(rawTag)
      code += transformed.code
      changed = changed || transformed.changed
      needsHelper = needsHelper || transformed.needsHelper
    } else {
      code += rawTag
    }

    if (info) {
      if (info.closing) {
        closeTag(stack, info.name)
      } else if (!info.selfClosing) {
        stack.push({ name: info.name, skip: info.skipSubtree })
      }
    }

    cursor = tagEnd + 1
  }

  return { code, changed, needsHelper }
}

function injectScriptSetupHelper(source: string): string {
  if (source.includes(`legacyT: ${helperName}`)) {
    return source
  }

  const helperSource = `\nimport { useI18n as ${helperImportName} } from '@/i18n'\nconst { legacyT: ${helperName} } = ${helperImportName}()\n`
  const scriptSetupMatch = source.match(/<script\s+setup(?:\s[^>]*)?>/)

  if (scriptSetupMatch?.index !== undefined) {
    const insertAt = scriptSetupMatch.index + scriptSetupMatch[0].length
    return `${source.slice(0, insertAt)}${helperSource}${source.slice(insertAt)}`
  }

  return `${source}\n<script setup lang="ts">${helperSource}</script>\n`
}

function transformVueSource(source: string): TemplateTransformResult {
  const templateMatch = source.match(/<template(?:\s[^>]*)?>([\s\S]*?)<\/template>/)
  if (!templateMatch || templateMatch.index === undefined) {
    return { code: source, changed: false, needsHelper: false }
  }

  const templateContent = templateMatch[1]
  const transformed = transformLegacyTemplateI18n(templateContent)
  if (!transformed.changed) {
    return { code: source, changed: false, needsHelper: false }
  }

  const templateStart = templateMatch.index + templateMatch[0].indexOf(templateContent)
  const templateEnd = templateStart + templateContent.length
  const nextSource = `${source.slice(0, templateStart)}${transformed.code}${source.slice(templateEnd)}`
  const code = transformed.needsHelper ? injectScriptSetupHelper(nextSource) : nextSource

  return {
    code,
    changed: true,
    needsHelper: transformed.needsHelper,
  }
}

export function legacyTemplateI18nPlugin(): Plugin {
  return {
    name: 'aether-legacy-template-i18n',
    enforce: 'pre',
    transform(source, id) {
      const filename = id.split('?')[0]
      if (!filename.endsWith('.vue')) {
        return null
      }

      const transformed = transformVueSource(source)
      if (!transformed.changed) {
        return null
      }

      return {
        code: transformed.code,
        map: null,
      }
    },
  }
}
