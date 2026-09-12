<template>
  <div>
    <div
      v-if="!bodyDocument && (data == null || (typeof data === 'object' && (Array.isArray(data) ? data.length === 0 : Object.keys(data).length === 0)))"
      class="text-sm text-muted-foreground"
    >
      {{ emptyMessage }}
    </div>
    <Card
      v-else
      class="bg-muted/30 overflow-hidden"
    >
      <VirtualBodyContent
        :key="viewRevision"
        ref="viewer"
        class="json-viewer"
        :class="{ 'theme-dark': isDark }"
        :load-chunk="loadChunk"
        @load-error="emit('load-error', $event)"
      >
        <template #default="{ chunk, index }">
          <div
            v-if="chunk.parseError && index === 0"
            class="p-3 bg-amber-50 dark:bg-amber-900/20 border-b border-amber-200 dark:border-amber-800"
          >
            <div class="flex items-start gap-2">
              <span class="text-amber-600 dark:text-amber-400 text-sm font-medium">Warning: 响应解析失败</span>
              <span class="text-xs text-amber-700 dark:text-amber-300">{{ chunk.parseError }}</span>
            </div>
          </div>
          <div
            v-if="chunk.text !== undefined"
            class="px-4"
            :class="{ 'pt-4': index === 0, 'pb-4': !chunk.hasNext }"
          >
            <pre class="text-xs font-mono whitespace-pre-wrap break-all">{{ chunk.text }}</pre>
          </div>
          <div
            v-else
            class="json-lines"
          >
            <template
              v-for="line in chunk.lines"
              :key="line.id"
            >
              <div
                class="json-line"
                :class="{ 'has-fold': line.canFold }"
                :data-json-line="line.lineNumber"
              >
                <!-- 行号区域（包含折叠按钮） -->
                <div class="line-number-area">
                  <button
                    v-if="line.canFold"
                    class="fold-button"
                    type="button"
                    :aria-label="line.collapsed ? '展开节点' : '折叠节点'"
                    :aria-expanded="!line.collapsed"
                    @click="toggleFold(line, index)"
                  >
                    <ChevronRight
                      v-if="line.collapsed"
                      class="fold-icon"
                    />
                    <ChevronDown
                      v-else
                      class="fold-icon"
                    />
                  </button>
                  <span class="line-number">{{ line.continuation ? '' : line.lineNumber }}</span>
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
                    :class="{ 'clickable-collapsed': line.canFold && line.collapsed }"
                    @click="line.canFold && line.collapsed && toggleFold(line, index)"
                    v-html="getDisplayHtml(line)"
                  />
                <!-- eslint-enable vue/no-v-html -->
                </div>
              </div>
            </template>
          </div>
        </template>
      </VirtualBodyContent>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import { ChevronRight, ChevronDown } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import VirtualBodyContent from './VirtualBodyContent.vue'
import { getRawTextChunk, JsonPageReader, JSON_SCROLL_CHUNK_SIZE, JSON_TEXT_CHUNK_SIZE, type JsonDisplayLine } from '../../utils/json-viewer'
import type { BodyDocument } from '../../utils/body-document'
import type { BodyJsonPage } from '../../utils/body-document-protocol'

const props = defineProps<{
  data: unknown
  bodyDocument?: BodyDocument | null
  viewMode: 'formatted' | 'raw' | 'compare'
  expandDepth: number
  isDark: boolean
  emptyMessage: string
}>()

const emit = defineEmits<{ 'load-error': [error: unknown] }>()
const viewer = ref<{ refresh: (index: number, resetTail?: boolean) => void } | null>(null)
const viewRevision = ref(0)
const foldOverrides = ref(new Map<string, boolean>())
let localReader: JsonPageReader | undefined

function loadChunk(index: number): BodyJsonPage | Promise<BodyJsonPage> {
  if (props.bodyDocument) return props.bodyDocument.json({
    page: index,
    pageSize: JSON_SCROLL_CHUNK_SIZE,
    expandDepth: props.expandDepth,
    foldOverrides: new Map(foldOverrides.value),
  })
  const record = props.data && typeof props.data === 'object' ? props.data as Record<string, unknown> : null
  const metadata = record?.metadata as Record<string, unknown> | undefined
  const parseError = record?.raw_response && metadata?.parse_error ? String(metadata.parse_error) : undefined
  const text = typeof props.data === 'string' ? props.data : parseError ? String(record?.raw_response) : undefined
  if (text !== undefined) return { lines: [], ...getRawTextChunk(text, index), parseError }
  localReader ??= new JsonPageReader(props.data, { pageSize: JSON_SCROLL_CHUNK_SIZE, expandDepth: props.expandDepth, foldOverrides: new Map(foldOverrides.value), stringChunkSize: JSON_TEXT_CHUNK_SIZE })
  return localReader.read(index)
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;')
}

function token(value: string, type: string): string {
  return `<span class="token-${type}">${escapeHtml(value)}</span>`
}

function getDisplayHtml(line: JsonDisplayLine): string {
  if (line.tokens) return line.tokens.map(part => part.type === 'info'
    ? `<span class="collapsed-info">${escapeHtml(part.text)}</span>` : token(part.text, part.type)).join('')
  const key = line.key === undefined ? ''
    : token(JSON.stringify(line.key), 'key') + token(': ', 'punctuation')
  if (line.bracket) {
    let content = key + token(line.bracket, 'bracket')
    if (line.collapsed) {
      if (line.childCount) content += token('...', 'ellipsis')
      content += token(line.closingBracket || '', 'bracket') + line.comma
      if (line.childCount) content += `<span class="collapsed-info">${line.childCount} ${line.isArray ? 'items' : 'keys'}</span>`
    } else if (!line.canFold) {
      content += line.comma
    }
    return content
  }
  if (line.value === null) return key + token('null', 'null') + line.comma
  if (typeof line.value === 'string') {
    return key + token(JSON.stringify(line.value), 'string') + line.comma
  }
  const valueType = typeof line.value
  return key + token(String(line.value), valueType === 'number' || valueType === 'boolean' ? valueType : 'string') + line.comma
}

function toggleFold(line: JsonDisplayLine, index: number) {
  const overrides = new Map(foldOverrides.value)
  overrides.set(line.id, !line.collapsed)
  foldOverrides.value = overrides
  localReader = undefined
  viewer.value?.refresh(index, true)
}

watch([() => props.data, () => props.bodyDocument, () => props.expandDepth], () => {
  viewRevision.value += 1
  localReader = undefined
  foldOverrides.value = new Map()
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
