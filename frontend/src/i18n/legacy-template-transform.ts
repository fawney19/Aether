import { NodeTypes, parse as parseTemplate, type ElementNode, type TemplateChildNode } from '@vue/compiler-dom'
import type { Expression } from '@babel/types'
import { babelParse, MagicString, parse as parseSfc } from 'vue/compiler-sfc'
import type { Plugin } from 'vite'

const cjkPattern = /[\u4e00-\u9fff]/
const helperName = '__aetherLegacyT'
const helperImportName = '__useAetherI18n'
const skipTags = new Set(['script', 'style', 'code', 'pre', 'kbd', 'samp', 'textarea'])
const translatableAttributeNames = new Set([
  'alt', 'aria-label', 'cancel-text', 'client-label', 'confirm-text', 'description',
  'drop-title', 'empty-message', 'empty-text', 'entity-label', 'filter-title', 'label',
  'manual-placeholder', 'message', 'path-hint', 'placeholder', 'provider-label',
  'search-placeholder', 'subtitle', 'title',
])

interface TemplateTransformResult {
  code: string
  changed: boolean
  needsHelper: boolean
}

function toExpressionString(value: string): string {
  return JSON.stringify(value).replace(/</g, '\\u003c').replace(/}/g, '\\u007d')
}

function escapeAttribute(expression: string): string {
  return expression.replace(/&/g, '&amp;').replace(/'/g, '&#39;').replace(/</g, '&lt;')
}

function isTranslatableAttribute(node: ElementNode, name: string): boolean {
  const normalized = name.toLowerCase()
  return translatableAttributeNames.has(normalized)
    || (normalized === 'text' && (node.tag === 'HelpHint' || node.tag === 'help-hint'))
}

function translateExpression(expression: string): string {
  if (!cjkPattern.test(expression)) return expression

  let parsed: Expression
  try {
    const statement = babelParse(`(${expression})`, { plugins: ['typescript'] }).program.body[0]
    if (statement?.type !== 'ExpressionStatement') return expression
    parsed = statement.expression
  } catch {
    // Vue reports invalid expressions with the original source location.
    return expression
  }

  const code = new MagicString(expression)
  const visit = (node: Expression): void => {
    if (node.start == null || node.end == null) return
    if (node.type === 'StringLiteral' && cjkPattern.test(node.value)) {
      code.overwrite(node.start - 1, node.end - 1, `${helperName}(${toExpressionString(node.value)})`)
    } else if (node.type === 'ConditionalExpression') {
      visit(node.consequent)
      visit(node.alternate)
    } else if (node.type === 'LogicalExpression' || (node.type === 'BinaryExpression' && node.operator === '+')) {
      if (node.left.type !== 'PrivateName') visit(node.left)
      visit(node.right)
    } else if (node.type === 'TemplateLiteral') {
      for (const quasi of node.quasis) {
        const value = quasi.value.cooked ?? quasi.value.raw
        if (cjkPattern.test(value) && quasi.start != null && quasi.end != null) {
          code.overwrite(quasi.start - 1, quasi.end - 1, `\${${helperName}(${toExpressionString(value)})}`)
        }
      }
      for (const value of node.expressions) visit(value as Expression)
    } else if (node.type === 'TSAsExpression' || node.type === 'TSSatisfiesExpression' || node.type === 'TSNonNullExpression' || node.type === 'TypeCastExpression' || node.type === 'ParenthesizedExpression') {
      visit(node.expression)
    }
  }

  // Only result positions are display text. Conditions, lookup keys, and call
  // arguments may be application data and must retain their original values.
  visit(parsed)
  return code.toString()
}

function hasVPre(node: ElementNode): boolean {
  // Vue removes v-pre from its AST. Remove the parsed attributes from the
  // opening source before checking the remaining directive, so quoted values
  // such as title="v-pre" cannot accidentally disable translation.
  const openingEnd = node.children[0]?.loc.start.offset ?? node.loc.end.offset
  let cursor = node.loc.start.offset
  let unparsed = ''
  for (const prop of node.props) {
    unparsed += node.loc.source.slice(cursor - node.loc.start.offset, prop.loc.start.offset - node.loc.start.offset)
    cursor = prop.loc.end.offset
  }
  unparsed += node.loc.source.slice(cursor - node.loc.start.offset, openingEnd - node.loc.start.offset)
  return /\sv-pre(?:[\s=>]|$)/.test(unparsed)
}

function shouldSkipElement(node: ElementNode): boolean {
  if (skipTags.has(node.tag.toLowerCase()) || hasVPre(node)) return true

  return node.props.some(prop => {
    if (prop.type !== NodeTypes.ATTRIBUTE) return false
    const name = prop.name.toLowerCase()
    return name === 'data-i18n-skip' || name === 'contenteditable' || name === 'v-pre'
      || (name === 'translate' && prop.value?.content.toLowerCase() === 'no')
  })
}

export function transformLegacyTemplateI18n(template: string): TemplateTransformResult {
  const ast = parseTemplate(template)
  const code = new MagicString(template)
  let needsHelper = false

  const visit = (node: TemplateChildNode): void => {
    if (node.type === NodeTypes.ELEMENT) {
      if (shouldSkipElement(node)) {
        if (hasVPre(node) && !node.props.some(prop => prop.type === NodeTypes.ATTRIBUTE && prop.name === 'data-i18n-skip')) {
          code.appendLeft(node.loc.start.offset + node.tag.length + 1, ' data-i18n-skip')
        }
        return
      }

      for (const prop of node.props) {
        if (prop.type === NodeTypes.ATTRIBUTE) {
          if (!prop.value || !isTranslatableAttribute(node, prop.name) || !cjkPattern.test(prop.value.content)) continue
          const expression = `${helperName}(${toExpressionString(prop.value.content)})`
          code.overwrite(prop.loc.start.offset, prop.loc.end.offset, `:${prop.name}='${escapeAttribute(expression)}'`)
          needsHelper = true
        } else if (prop.name === 'bind' && prop.arg?.type === NodeTypes.SIMPLE_EXPRESSION && prop.arg.isStatic && prop.exp?.type === NodeTypes.SIMPLE_EXPRESSION && isTranslatableAttribute(node, prop.arg.content)) {
          const expression = translateExpression(prop.exp.content)
          if (expression !== prop.exp.content) {
            code.overwrite(prop.loc.start.offset, prop.loc.end.offset, `${prop.rawName ?? `:${prop.arg.content}`}='${escapeAttribute(expression)}'`)
            needsHelper = true
          }
        }
      }

      node.children.forEach(visit)
    } else if (node.type === NodeTypes.TEXT && cjkPattern.test(node.content)) {
      code.overwrite(node.loc.start.offset, node.loc.end.offset, `{{ ${helperName}(${toExpressionString(node.content)}) }}`)
      needsHelper = true
    } else if (node.type === NodeTypes.INTERPOLATION && node.content.type === NodeTypes.SIMPLE_EXPRESSION) {
      const expression = translateExpression(node.content.content)
      if (expression !== node.content.content) {
        code.overwrite(node.content.loc.start.offset, node.content.loc.end.offset, expression)
        needsHelper = true
      }
    }
  }

  ast.children.forEach(visit)
  return { code: code.toString(), changed: code.hasChanged(), needsHelper }
}

export function transformVueSource(source: string): TemplateTransformResult {
  const { descriptor } = parseSfc(source)
  const template = descriptor.template
  if (!template || template.src || (template.lang && template.lang !== 'html')) {
    return { code: source, changed: false, needsHelper: false }
  }

  const transformed = transformLegacyTemplateI18n(template.content)
  if (!transformed.changed) return { code: source, changed: false, needsHelper: false }

  const code = new MagicString(source)
  code.overwrite(template.loc.start.offset, template.loc.end.offset, transformed.code)
  const helperSource = `\nimport { useI18n as ${helperImportName} } from '@/i18n'\nconst { legacyT: ${helperName} } = ${helperImportName}()\n`
  if (transformed.needsHelper && !descriptor.scriptSetup?.content.includes(`legacyT: ${helperName}`)) {
    if (descriptor.scriptSetup) {
      code.appendLeft(descriptor.scriptSetup.loc.start.offset, helperSource)
    } else {
      const language = descriptor.script ? descriptor.script.lang : 'ts'
      const languageAttribute = language ? ` lang="${language}"` : ''
      code.append(`\n<script setup${languageAttribute}>${helperSource}</script>\n`)
    }
  }

  return { code: code.toString(), changed: true, needsHelper: transformed.needsHelper }
}

export function legacyTemplateI18nPlugin(): Plugin {
  return {
    name: 'aether-legacy-template-i18n',
    enforce: 'pre',
    transform(source, id) {
      if (!id.endsWith('.vue')) return null

      const transformed = transformVueSource(source)
      return transformed.changed ? { code: transformed.code, map: null } : null
    },
  }
}
