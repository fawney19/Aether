<template>
  <div>
    <div
      v-if="!data || (typeof data === 'object' && Object.keys(data).length === 0)"
      class="text-sm text-muted-foreground"
    >
      {{ emptyMessage }}
    </div>
    <!-- 纯字符串数据（非 JSON 对象） -->
    <Card
      v-else-if="typeof data === 'string'"
      class="bg-muted/30 overflow-hidden"
    >
      <div class="p-4 overflow-x-auto max-h-[500px] overflow-y-auto">
        <pre class="text-xs font-mono whitespace-pre-wrap">{{ data }}</pre>
      </div>
    </Card>
    <!-- 非 JSON 响应（如 HTML 错误页面） -->
    <Card
      v-else-if="hasParseError"
      class="bg-muted/30 overflow-hidden"
    >
      <div class="p-3 bg-amber-50 dark:bg-amber-900/20 border-b border-amber-200 dark:border-amber-800">
        <div class="flex items-start gap-2">
          <span class="text-amber-600 dark:text-amber-400 text-sm font-medium">Warning: 响应解析失败</span>
          <span class="text-xs text-amber-700 dark:text-amber-300">{{ parseErrorMessage }}</span>
        </div>
      </div>
      <div class="p-4 overflow-x-auto max-h-[500px] overflow-y-auto">
        <pre class="text-xs font-mono whitespace-pre-wrap text-muted-foreground">{{ rawResponseContent }}</pre>
      </div>
    </Card>
    <Card
      v-else
      class="bg-muted/30 overflow-hidden"
    >
      <!-- JSON 查看器 -->
      <div
        class="json-viewer"
        :class="{ 'theme-dark': isDark }"
      >
        <div class="json-lines">
          <template
            v-for="line in visibleLines"
            :key="line.displayId"
          >
            <div
              class="json-line"
              :class="{ 'has-fold': line.canFold }"
            >
              <!-- 行号区域（包含折叠按钮） -->
              <div class="line-number-area">
                <span
                  v-if="line.canFold"
                  class="fold-button"
                  @click="toggleFold(line.blockId)"
                >
                  <ChevronRight
                    v-if="collapsedBlocks.has(line.blockId)"
                    class="fold-icon"
                  />
                  <ChevronDown
                    v-else
                    class="fold-icon"
                  />
                </span>
                <span class="line-number">{{ line.displayLineNumber }}</span>
              </div>
              <!-- 内容区域 -->
              <div class="line-content-area">
                <!-- 缩进 -->
                <span
                  class="indent"
                  :style="{ width: `${line.indent * 16}px` }"
                />
                <!-- 内容 -->
                <!-- eslint-disable vue/no-v-html -->
                <span
                  class="line-content"
                  :class="{ 'clickable-collapsed': line.canFold && collapsedBlocks.has(line.blockId) }"
                  @click="line.canFold && collapsedBlocks.has(line.blockId) && toggleFold(line.blockId)"
                  v-html="getDisplayHtml(line)"
                />
                <!-- eslint-enable vue/no-v-html -->
              </div>
            </div>
          </template>
        </div>
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { ChevronRight, ChevronDown } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'

interface JsonLine {
  id: number
  lineNumber: number
  indent: number
  html: string
  canFold: boolean
  blockId: string
  blockEnd?: number
  collapsedInfo?: string
  closingBracket?: string
  trailingComma?: string
}

interface DisplayLine extends JsonLine {
  displayId: string
  displayLineNumber: number
}

/** JSON data can be any serializable value: object, array, string, number, boolean, null */
type JsonValue = Record<string, unknown> | unknown[] | string | number | boolean | null | undefined

const props = defineProps<{
  data: JsonValue
  viewMode: 'formatted' | 'raw' | 'compare'
  expandDepth: number
  isDark: boolean
  emptyMessage: string
}>()

/** Safely cast data to an object for property access in templates */
const dataAsObject = computed(() => {
  if (props.data && typeof props.data === 'object' && !Array.isArray(props.data)) {
    return props.data as Record<string, unknown>
  }
  return null
})

/** Whether the data contains a raw_response with a parse error (non-JSON response) */
const hasParseError = computed(() => {
  const obj = dataAsObject.value
  if (!obj) return false
  const metadata = obj.metadata as Record<string, unknown> | undefined
  return Boolean(obj.raw_response && metadata?.parse_error)
})

/** Parse error message */
const parseErrorMessage = computed(() => {
  const obj = dataAsObject.value
  if (!obj) return ''
  const metadata = obj.metadata as Record<string, unknown> | undefined
  return String(metadata?.parse_error || '')
})

/** Raw response content */
const rawResponseContent = computed(() => {
  const obj = dataAsObject.value
  return obj ? String(obj.raw_response || '') : ''
})

const collapsedBlocks = ref<Set<string>>(new Set())
const lines = ref<JsonLine[]>([])

const getTokenHtml = (value: string, type: 'key' | 'string' | 'number' | 'boolean' | 'null' | 'bracket' | 'punctuation' | 'ellipsis'): string => {
  const classMap = {
    key: 'token-key',
    string: 'token-string',
    number: 'token-number',
    boolean: 'token-boolean',
    null: 'token-null',
    bracket: 'token-bracket',
    punctuation: 'token-punctuation',
    ellipsis: 'token-ellipsis',
  }
  return `<span class="${classMap[type]}">${escapeHtml(value)}</span>`
}

const escapeHtml = (str: string): string => {
  return str
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
}

const jsonStringLiteral = (value: string): string => JSON.stringify(value)

const parseJsonToLines = (data: unknown): JsonLine[] => {
  const result: JsonLine[] = []
  let lineNumber = 1
  let blockIdCounter = 0

  const getBlockId = () => `block-${blockIdCounter++}`

  const processValue = (value: unknown, indent: number, isLast: boolean, keyPrefix: string = ''): void => {
    const comma = isLast ? '' : ','

    if (value === null) {
      result.push({
        id: result.length,
        lineNumber: lineNumber++,
        indent,
        html: keyPrefix + getTokenHtml('null', 'null') + comma,
        canFold: false,
        blockId: '',
      })
    } else if (typeof value === 'boolean') {
      result.push({
        id: result.length,
        lineNumber: lineNumber++,
        indent,
        html: keyPrefix + getTokenHtml(String(value), 'boolean') + comma,
        canFold: false,
        blockId: '',
      })
    } else if (typeof value === 'number') {
      result.push({
        id: result.length,
        lineNumber: lineNumber++,
        indent,
        html: keyPrefix + getTokenHtml(String(value), 'number') + comma,
        canFold: false,
        blockId: '',
      })
    } else if (typeof value === 'string') {
      result.push({
        id: result.length,
        lineNumber: lineNumber++,
        indent,
        html: keyPrefix + getTokenHtml(jsonStringLiteral(value), 'string') + comma,
        canFold: false,
        blockId: '',
      })
    } else if (Array.isArray(value)) {
      if (value.length === 0) {
        result.push({
          id: result.length,
          lineNumber: lineNumber++,
          indent,
          html: keyPrefix + getTokenHtml('[]', 'bracket') + comma,
          canFold: false,
          blockId: '',
        })
      } else {
        const blockId = getBlockId()
        const startLine = result.length
        result.push({
          id: result.length,
          lineNumber: lineNumber++,
          indent,
          html: keyPrefix + getTokenHtml('[', 'bracket'),
          canFold: true,
          blockId,
          collapsedInfo: `${value.length} items`,
          closingBracket: ']',
          trailingComma: comma,
        })

        value.forEach((item, i) => {
          processValue(item, indent + 1, i === value.length - 1)
        })

        result.push({
          id: result.length,
          lineNumber: lineNumber++,
          indent,
          html: getTokenHtml(']', 'bracket') + comma,
          canFold: false,
          blockId: '',
        })

        result[startLine].blockEnd = result.length - 1
      }
    } else if (typeof value === 'object') {
      const obj = value as Record<string, unknown>
      const keys = Object.keys(obj)
      if (keys.length === 0) {
        result.push({
          id: result.length,
          lineNumber: lineNumber++,
          indent,
          html: keyPrefix + getTokenHtml('{}', 'bracket') + comma,
          canFold: false,
          blockId: '',
        })
      } else {
        const blockId = getBlockId()
        const startLine = result.length
        result.push({
          id: result.length,
          lineNumber: lineNumber++,
          indent,
          html: keyPrefix + getTokenHtml('{', 'bracket'),
          canFold: true,
          blockId,
          collapsedInfo: `${keys.length} keys`,
          closingBracket: '}',
          trailingComma: comma,
        })

        keys.forEach((key, i) => {
          const keyHtml = getTokenHtml(jsonStringLiteral(key), 'key') + getTokenHtml(': ', 'punctuation')
          processValue(obj[key], indent + 1, i === keys.length - 1, keyHtml)
        })

        result.push({
          id: result.length,
          lineNumber: lineNumber++,
          indent,
          html: getTokenHtml('}', 'bracket') + comma,
          canFold: false,
          blockId: '',
        })

        result[startLine].blockEnd = result.length - 1
      }
    } else {
      result.push({
        id: result.length,
        lineNumber: lineNumber++,
        indent,
        html: keyPrefix + getTokenHtml(String(value), 'string') + comma,
        canFold: false,
        blockId: '',
      })
    }
  }

  processValue(data, 0, true)
  return result
}

const visibleLines = computed((): DisplayLine[] => {
  const result: DisplayLine[] = []
  const hiddenRanges: Array<{ start: number; end: number }> = []

  for (const line of lines.value) {
    if (line.canFold && collapsedBlocks.value.has(line.blockId) && line.blockEnd !== undefined) {
      hiddenRanges.push({ start: line.id + 1, end: line.blockEnd })
    }
  }

  const isHidden = (id: number): boolean => {
    return hiddenRanges.some(range => id >= range.start && id <= range.end)
  }

  let displayLineNumber = 1
  for (const line of lines.value) {
    if (!isHidden(line.id)) {
      result.push({
        ...line,
        displayId: `display-${line.id}`,
        displayLineNumber: displayLineNumber++,
      })
    }
  }

  return result
})

const getDisplayHtml = (line: DisplayLine): string => {
  if (line.canFold && collapsedBlocks.value.has(line.blockId)) {
    const closingBracket = getTokenHtml(line.closingBracket || '}', 'bracket')
    const ellipsis = getTokenHtml('...', 'ellipsis')
    const comma = line.trailingComma || ''
    return `${line.html}${ellipsis}${closingBracket}${comma}<span class="collapsed-info">${line.collapsedInfo}</span>`
  }
  return line.html
}

const toggleFold = (blockId: string) => {
  const newSet = new Set(collapsedBlocks.value)
  if (newSet.has(blockId)) {
    newSet.delete(blockId)
  } else {
    newSet.add(blockId)
  }
  collapsedBlocks.value = newSet
}

const initCollapsedState = () => {
  const newSet = new Set<string>()

  // 默认展开第一层（indent = 0），折叠更深层（indent >= 1）
  // expandDepth = 999 表示全部展开
  const depth = props.expandDepth === 0 ? 1 : props.expandDepth
  if (depth < 999) {
    for (const line of lines.value) {
      if (line.canFold && line.indent >= depth) {
        newSet.add(line.blockId)
      }
    }
  }

  collapsedBlocks.value = newSet
}

watch(() => props.data, () => {
  if (props.data) {
    lines.value = parseJsonToLines(props.data)
    initCollapsedState()
  } else {
    lines.value = []
  }
}, { immediate: true })

watch(() => props.expandDepth, () => {
  initCollapsedState()
})
</script>

<style scoped>
.json-viewer {
  max-height: 500px;
  overflow: auto;
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
  font-size: 13px;
  line-height: 20px;
}

.json-lines {
  padding: 4px 0;
}

.json-line {
  display: flex;
  min-height: 20px;
}

.json-line:hover {
  background: hsl(var(--muted) / 0.4);
}

.line-number-area {
  flex-shrink: 0;
  width: 48px;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  padding-right: 8px;
  background: hsl(var(--muted) / 0.2);
  border-right: 1px solid hsl(var(--border));
  user-select: none;
}

.fold-button {
  width: 16px;
  height: 16px;
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  color: hsl(var(--muted-foreground) / 0.6);
  margin-right: 2px;
  border-radius: 2px;
}

.fold-button:hover {
  color: hsl(var(--foreground));
  background: hsl(var(--muted) / 0.8);
}

.fold-icon {
  width: 14px;
  height: 14px;
}

.line-number {
  color: hsl(var(--muted-foreground) / 0.5);
  min-width: 20px;
  text-align: right;
}

.line-content-area {
  flex: 1;
  display: flex;
  padding-left: 12px;
  padding-right: 12px;
}

.indent {
  flex-shrink: 0;
}

.line-content {
  white-space: pre-wrap;
  word-break: break-all;
}

.line-content.clickable-collapsed {
  cursor: pointer;
}

.line-content.clickable-collapsed:hover :deep(.token-ellipsis) {
  background: hsl(var(--primary) / 0.2);
  border-radius: 2px;
}

/* Token 颜色 - 亮色主题 */
:deep(.token-key) {
  color: #0451a5;
}

:deep(.token-string) {
  color: #a31515;
}

:deep(.token-number) {
  color: #098658;
}

:deep(.token-boolean) {
  color: #0000ff;
}

:deep(.token-null) {
  color: #0000ff;
}

:deep(.token-bracket) {
  color: #000000;
}

:deep(.token-punctuation) {
  color: #000000;
}

:deep(.token-ellipsis) {
  color: #0451a5;
  padding: 0 2px;
}

:deep(.collapsed-info) {
  color: hsl(var(--muted-foreground));
  font-style: italic;
  margin-left: 8px;
  font-size: 12px;
}

/* Token 颜色 - 暗色主题 */
.theme-dark :deep(.token-key) {
  color: #9cdcfe;
}

.theme-dark :deep(.token-string) {
  color: #ce9178;
}

.theme-dark :deep(.token-number) {
  color: #b5cea8;
}

.theme-dark :deep(.token-boolean) {
  color: #569cd6;
}

.theme-dark :deep(.token-null) {
  color: #569cd6;
}

.theme-dark :deep(.token-bracket) {
  color: #d4d4d4;
}

.theme-dark :deep(.token-punctuation) {
  color: #d4d4d4;
}

.theme-dark :deep(.token-ellipsis) {
  color: #9cdcfe;
}

.theme-dark .line-number-area {
  background: hsl(var(--muted) / 0.3);
}
</style>
