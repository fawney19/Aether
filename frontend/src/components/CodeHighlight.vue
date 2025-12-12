<template>
  <div :class="wrapperClass">
    <pre><code
:class="`language-${language}`"
               v-html="highlightedCode"
    /></pre>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import hljs from 'highlight.js/lib/core'
import bash from 'highlight.js/lib/languages/bash'
import json from 'highlight.js/lib/languages/json'
import ini from 'highlight.js/lib/languages/ini'
import javascript from 'highlight.js/lib/languages/javascript'
import { log } from '@/utils/logger'

const props = defineProps<{
  code: string
  language: string
  dense?: boolean
}>()
// 注册需要的语言
hljs.registerLanguage('bash', bash)
hljs.registerLanguage('sh', bash)
hljs.registerLanguage('json', json)
hljs.registerLanguage('toml', ini)
hljs.registerLanguage('ini', ini)
hljs.registerLanguage('javascript', javascript)

const wrapperClass = computed(() =>
  ['code-highlight', props.dense ? 'code-highlight--dense' : '']
    .filter(Boolean)
    .join(' ')
)

// 自定义 bash 高亮增强
function enhanceBashHighlight(html: string, code: string): string {
  // 如果 highlight.js 已经识别了 token，直接返回
  if (html.includes('hljs-')) {
    return html
  }

  // 手动添加高亮
  const escaped = code
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;')

  // 使用占位符保护URL
  const urlPlaceholders: string[] = []
  let result = escaped.replace(/(https?:\/\/[^\s]+)/g, (match) => {
    const index = urlPlaceholders.length
    urlPlaceholders.push(match)
    return `__URL_PLACEHOLDER_${index}__`
  })

  // 高亮命令关键字
  result = result
    .replace(/\b(curl|npm|npx|git|bash|sh|powershell|iex|irm|wget|apt|yum|brew|pip|python|node|docker|kubectl)\b/g, '<span class="hljs-built_in">$1</span>')
    .replace(/(^|\s)(-[a-zA-Z]+)/gm, '$1<span class="hljs-meta">$2</span>')
    .replace(/(\|)/g, '<span class="hljs-keyword">$1</span>')

  // 恢复URL并添加高亮
  result = result.replace(/__URL_PLACEHOLDER_(\d+)__/g, (_, index) => {
    return `<span class="hljs-string">${urlPlaceholders[parseInt(index)]}</span>`
  })

  return result
}

// 自定义 dotenv 高亮
function highlightDotenv(code: string): string {
  const escaped = code
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#039;')

  return escaped
    // 注释行
    .replace(/^(#.*)$/gm, '<span class="hljs-comment">$1</span>')
    // 环境变量 KEY=VALUE
    .replace(/^([A-Z_][A-Z0-9_]*)(=)(.*)$/gm, (_, key, eq, value) => {
      return `<span class="hljs-attr">${key}</span>${eq}<span class="hljs-string">${value}</span>`
    })
}

const highlightedCode = computed(() => {
  const lang = props.language.trim().toLowerCase()
  const code = props.code ?? ''

  let result: string
  try {
    if (lang === 'bash' || lang === 'sh') {
      const highlighted = hljs.highlight(code, { language: 'bash' }).value
      result = enhanceBashHighlight(highlighted, code)
    } else if (lang === 'dotenv' || lang === 'env') {
      result = highlightDotenv(code)
    } else {
      const language = hljs.getLanguage(lang) ? lang : 'plaintext'
      result = hljs.highlight(code, { language }).value
    }
  } catch (e) {
    log.error('Highlight error:', e)
    result = code
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
  }

  // Highlight placeholders that need user modification (e.g., your-api-key, latest-model-name)
  result = highlightPlaceholders(result)

  return result
})

// Highlight placeholder values that users need to modify
function highlightPlaceholders(html: string): string {
  // Match common placeholder patterns (already HTML-escaped)
  const placeholderPatterns = [
    /your-api-key/gi,
    /latest-model-name/gi,
    /your-[a-z-]+/gi,
  ]

  for (const pattern of placeholderPatterns) {
    html = html.replace(pattern, (match) => {
      // Avoid double-wrapping if already in a span
      return `<span class="hljs-placeholder">${match}</span>`
    })
  }

  return html
}
</script>

<style scoped>
.code-highlight {
  width: 100%;
}

.code-highlight pre {
  margin: 0;
  padding: 0.9rem 1.1rem;
  border-radius: 0.85rem;
  border: 1px solid var(--color-border);
  background-color: var(--color-code-background);
  font-family: var(--font-mono, 'Cascadia Code', monospace);
  font-size: 0.875rem;
  line-height: 1.6;
  color: var(--color-code-text);
  overflow-x: auto;
  transition: background-color 0.2s ease, border-color 0.2s ease, color 0.2s ease;
  letter-spacing: 0.01em;
}

.code-highlight code {
  font-family: inherit;
  font-size: inherit;
  font-weight: 400;
}

.code-highlight--dense pre {
  padding: 0.6rem 0.9rem;
  font-size: 0.9375rem;
  border-radius: 0.75rem;
  line-height: 1.5;
  /* Dense mode: transparent background for embedding in panels */
  background-color: transparent;
  border: 1px solid var(--color-border);
}

/* Highlight.js 浅色主题 */
.code-highlight :deep(.hljs-string) {
  color: #24292e;
  font-weight: 450;
}

.code-highlight :deep(.hljs-attr),
.code-highlight :deep(.hljs-attribute) {
  color: #c96442;
  font-weight: 500;
}

.code-highlight :deep(.hljs-keyword),
.code-highlight :deep(.hljs-selector-tag),
.code-highlight :deep(.hljs-built_in) {
  color: #0066cc;
  font-weight: 500;
}

.code-highlight :deep(.hljs-comment) {
  color: #6a737d;
  font-style: italic;
  opacity: 0.8;
}

.code-highlight :deep(.hljs-function),
.code-highlight :deep(.hljs-title) {
  color: #6f42c1;
  font-weight: 500;
}

.code-highlight :deep(.hljs-number) {
  color: #005cc5;
}

.code-highlight :deep(.hljs-literal),
.code-highlight :deep(.language-json .hljs-literal),
.code-highlight :deep(.language-json .hljs-literal .hljs-keyword) {
  color: #24292e;
  font-weight: normal;
}

.code-highlight :deep(.hljs-variable),
.code-highlight :deep(.hljs-property) {
  color: #cc785c;
}

.code-highlight :deep(.hljs-punctuation) {
  color: #24292e;
  opacity: 0.7;
}

/* Placeholder values that need user modification */
.code-highlight :deep(.hljs-placeholder) {
  color: #d73a49;
  font-weight: 500;
  font-style: italic;
}

/* Bash 命令样式 - 浅色模式 */
.code-highlight :deep(.hljs-built_in),
.code-highlight :deep(.hljs-name),
.code-highlight :deep(.language-bash .hljs-built_in),
.code-highlight :deep(.language-bash .hljs-name) {
  color: #d73a49;
  font-weight: 500;
}

.code-highlight :deep(.hljs-meta),
.code-highlight :deep(.language-bash .hljs-meta) {
  color: #d73a49;
  font-weight: 500;
}

.code-highlight :deep(.hljs-params),
.code-highlight :deep(.language-bash .hljs-params) {
  color: #0366d6;
  font-weight: 500;
}

.code-highlight :deep(.language-bash .hljs-keyword),
.code-highlight :deep(.language-bash .hljs-literal) {
  color: #0366d6;
  font-weight: 500;
}

.code-highlight :deep(.language-bash) {
  color: #24292e;
}

.code-highlight :deep(.language-bash .hljs-string) {
  color: #24292e;
  font-weight: 400;
}

.code-highlight :deep(pre code.language-bash) {
  color: #24292e;
}

.code-highlight :deep(pre code.language-bash .hljs-subst) {
  color: #24292e;
}

/* Highlight.js 深色主题 */
.dark .code-highlight :deep(.hljs-string),
body[theme-mode='dark'] .code-highlight :deep(.hljs-string) {
  color: #f1ead8;
  font-weight: 450;
}

.dark .code-highlight :deep(.hljs-attr),
.dark .code-highlight :deep(.hljs-attribute),
body[theme-mode='dark'] .code-highlight :deep(.hljs-attr),
body[theme-mode='dark'] .code-highlight :deep(.hljs-attribute) {
  color: #d4a27f;
  font-weight: 500;
}

.dark .code-highlight :deep(.hljs-keyword),
.dark .code-highlight :deep(.hljs-selector-tag),
.dark .code-highlight :deep(.hljs-built_in),
body[theme-mode='dark'] .code-highlight :deep(.hljs-keyword),
body[theme-mode='dark'] .code-highlight :deep(.hljs-selector-tag),
body[theme-mode='dark'] .code-highlight :deep(.hljs-built_in) {
  color: #569cd6;
  font-weight: 500;
}

.dark .code-highlight :deep(.hljs-comment),
body[theme-mode='dark'] .code-highlight :deep(.hljs-comment) {
  color: #6a9955;
  font-style: italic;
  opacity: 0.85;
}

.dark .code-highlight :deep(.hljs-function),
.dark .code-highlight :deep(.hljs-title),
body[theme-mode='dark'] .code-highlight :deep(.hljs-function),
body[theme-mode='dark'] .code-highlight :deep(.hljs-title) {
  color: #dcdcaa;
  font-weight: 500;
}

.dark .code-highlight :deep(.hljs-number),
body[theme-mode='dark'] .code-highlight :deep(.hljs-number) {
  color: #b5cea8;
}

.dark .code-highlight :deep(.hljs-literal),
.dark .code-highlight :deep(.language-json .hljs-literal),
.dark .code-highlight :deep(.language-json .hljs-literal .hljs-keyword),
body[theme-mode='dark'] .code-highlight :deep(.hljs-literal),
body[theme-mode='dark'] .code-highlight :deep(.language-json .hljs-literal),
body[theme-mode='dark'] .code-highlight :deep(.language-json .hljs-literal .hljs-keyword) {
  color: #e1e4e8;
  font-weight: normal;
}

.dark .code-highlight :deep(.hljs-variable),
.dark .code-highlight :deep(.hljs-property),
body[theme-mode='dark'] .code-highlight :deep(.hljs-variable),
body[theme-mode='dark'] .code-highlight :deep(.hljs-property) {
  color: #9cdcfe;
}

.dark .code-highlight :deep(.hljs-punctuation),
body[theme-mode='dark'] .code-highlight :deep(.hljs-punctuation) {
  color: #d4d4d4;
  opacity: 0.7;
}

/* Placeholder values - dark mode */
.dark .code-highlight :deep(.hljs-placeholder),
body[theme-mode='dark'] .code-highlight :deep(.hljs-placeholder) {
  color: #f97583;
  font-weight: 500;
  font-style: italic;
}

/* Bash 深色主题 */
.dark .code-highlight :deep(.hljs-built_in),
.dark .code-highlight :deep(.hljs-name),
.dark .code-highlight :deep(.language-bash .hljs-built_in),
.dark .code-highlight :deep(.language-bash .hljs-name),
body[theme-mode='dark'] .code-highlight :deep(.hljs-built_in),
body[theme-mode='dark'] .code-highlight :deep(.hljs-name),
body[theme-mode='dark'] .code-highlight :deep(.language-bash .hljs-built_in),
body[theme-mode='dark'] .code-highlight :deep(.language-bash .hljs-name) {
  color: #e67764;
  font-weight: 500;
}

.dark .code-highlight :deep(.hljs-meta),
.dark .code-highlight :deep(.language-bash .hljs-meta),
body[theme-mode='dark'] .code-highlight :deep(.hljs-meta),
body[theme-mode='dark'] .code-highlight :deep(.language-bash .hljs-meta) {
  color: #e67764;
}

.dark .code-highlight :deep(.hljs-params),
.dark .code-highlight :deep(.language-bash .hljs-params),
body[theme-mode='dark'] .code-highlight :deep(.hljs-params),
body[theme-mode='dark'] .code-highlight :deep(.language-bash .hljs-params) {
  color: #6fa9e6;
  font-weight: 500;
}

.dark .code-highlight :deep(.language-bash .hljs-keyword),
.dark .code-highlight :deep(.language-bash .hljs-literal),
body[theme-mode='dark'] .code-highlight :deep(.language-bash .hljs-keyword),
body[theme-mode='dark'] .code-highlight :deep(.language-bash .hljs-literal) {
  color: #6fa9e6;
  font-weight: 500;
}

.dark .code-highlight :deep(.language-bash),
body[theme-mode='dark'] .code-highlight :deep(.language-bash) {
  color: #e1e4e8;
}

.dark .code-highlight :deep(.language-bash .hljs-string),
body[theme-mode='dark'] .code-highlight :deep(.language-bash .hljs-string) {
  color: #e1e4e8;
  font-weight: 400;
}
</style>
