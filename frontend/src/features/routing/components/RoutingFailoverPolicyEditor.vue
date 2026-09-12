<template>
  <section
    class="space-y-4 rounded-lg border border-border/60 p-4"
    data-testid="routing-failover-policy"
  >
    <div>
      <h3 class="text-sm font-medium">
        故障转移规则
      </h3>
      <p class="mt-1 text-xs leading-relaxed text-muted-foreground">
        作用于当前调度策略的所有提供商。先检查全局规则，再检查提供商自身规则；业务内容输出后不再重放请求。
      </p>
    </div>
    <div class="grid grid-cols-1 gap-4 md:grid-cols-2">
      <label class="space-y-1.5 text-sm">
        <span>全局最大转移次数</span>
        <Input
          :model-value="modelValue.max_transfer_count"
          :disabled="disabled"
          type="number"
          min="0"
          step="1"
          aria-label="全局最大转移次数"
          @update:model-value="updateLimit('max_transfer_count', $event)"
        />
        <span class="block text-xs leading-relaxed text-muted-foreground">0 不限制。首次尝试和粘性同 Key 重试不计入；每次切换候选计 1 次。</span>
      </label>
      <label class="space-y-1.5 text-sm">
        <span>全局最大转移时间（秒）</span>
        <Input
          :model-value="modelValue.max_transfer_timeout_seconds"
          :disabled="disabled"
          type="number"
          min="0"
          step="1"
          aria-label="全局最大转移时间"
          @update:model-value="updateLimit('max_transfer_timeout_seconds', $event)"
        />
        <span class="block text-xs leading-relaxed text-muted-foreground">0 不限制。从首次尝试累计，耗尽后不再启动下一次尝试；不会中断已开始的调用，单次超时仍独立生效。</span>
      </label>
    </div>
    <div
      v-for="section in ruleSections"
      :key="section.key"
      class="space-y-3"
    >
      <div class="flex flex-wrap items-start justify-between gap-3">
        <div class="min-w-0">
          <h4 class="text-sm font-medium">
            {{ section.title }}
          </h4>
          <p class="mt-1 text-xs leading-relaxed text-muted-foreground">
            {{ section.description }}
          </p>
        </div>
        <div class="flex shrink-0 items-center gap-1">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-xs"
            :disabled="disabled"
            :title="jsonMode[section.key] ? `切回${section.title}表单` : `切到${section.title} JSON`"
            :aria-label="jsonMode[section.key] ? `切回${section.title}表单` : `切到${section.title} JSON`"
            @click="toggleJsonMode(section.key)"
          >
            <Code2 class="mr-1 h-3 w-3" />
            {{ jsonMode[section.key] ? '表单' : 'JSON' }}
          </Button>
          <Button
            v-if="jsonMode[section.key]"
            type="button"
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-xs"
            :disabled="disabled"
            :title="`格式化${section.title} JSON`"
            :aria-label="`格式化${section.title} JSON`"
            @click="formatJsonDraft(section.key)"
          >
            <AlignLeft class="mr-1 h-3 w-3" />格式化
          </Button>
          <Button
            v-if="!jsonMode[section.key]"
            type="button"
            variant="ghost"
            size="sm"
            class="h-7 px-2 text-xs"
            :disabled="disabled || modelValue.failover_rules[section.key].length >= MAX_ROUTING_FAILOVER_RULES"
            :aria-label="`添加${section.title}`"
            @click="addRule(section.key)"
          >
            <Plus class="mr-1 h-3 w-3" />添加
          </Button>
        </div>
      </div>
      <div
        v-if="jsonMode[section.key]"
        class="space-y-2"
      >
        <Textarea
          :model-value="jsonDraft[section.key]"
          class="min-h-[160px] font-mono text-xs leading-relaxed"
          :disabled="disabled"
          :aria-label="`${section.title} JSON`"
          spellcheck="false"
          :placeholder="jsonPlaceholder(section.key)"
          @update:model-value="updateJsonDraft(section.key, $event)"
        />
        <div
          v-if="jsonError[section.key]"
          role="alert"
          class="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          {{ jsonError[section.key] }}
        </div>
        <p class="text-xs text-muted-foreground">
          仅管理{{ section.title }}；JSON 应为数组，<code class="rounded bg-muted px-1">pattern</code>{{ section.key === 'success_failover_patterns' ? ' 必填。' : ' 和 status_codes 至少填写一个。' }}
        </p>
      </div>
      <template v-else>
        <p
          v-if="modelValue.failover_rules[section.key].length === 0"
          class="rounded-md border border-dashed p-3 text-xs text-muted-foreground"
        >
          暂无规则
        </p>
        <div
          v-for="(rule, index) in modelValue.failover_rules[section.key]"
          :key="index"
          class="grid min-w-0 grid-cols-[minmax(0,1fr)_2rem] items-start gap-2"
          :class="section.key === 'error_stop_patterns' ? 'sm:grid-cols-[10rem_minmax(0,1fr)_2rem]' : ''"
        >
          <Input
            v-if="section.key === 'error_stop_patterns'"
            :model-value="statusDrafts[index] ?? rule.status_codes.join(', ')"
            :disabled="disabled"
            size="sm"
            class="col-span-2 min-w-0 w-full font-mono text-xs sm:col-span-1"
            :aria-label="`终止规则 ${index + 1} 状态码`"
            placeholder="400, 413（选填）"
            title="状态码用逗号或空格分隔；留空则匹配全部错误状态"
            @update:model-value="updateStatuses(index, String($event))"
          />
          <Input
            :model-value="rule.pattern"
            :disabled="disabled"
            size="sm"
            class="min-w-0 flex-1 font-mono text-xs"
            :aria-label="`${section.title} ${index + 1} 正则`"
            :placeholder="section.key === 'success_failover_patterns' ? '(?i)capacity.*exhausted' : '正则内容（选填）'"
            @update:model-value="updateRule(section.key, index, { pattern: String($event) })"
          />
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-8 w-8 shrink-0 p-0"
            :disabled="disabled"
            :aria-label="`删除${section.title} ${index + 1}`"
            @click="removeRule(section.key, index)"
          >
            <Trash2 class="h-3.5 w-3.5" />
          </Button>
        </div>
      </template>
    </div>
    <p class="text-xs text-muted-foreground">
      正则支持 (?i) 忽略大小写；服务端在保存时校验语法。每组最多 64 条，每条正则最多 4096 字节。
    </p>
    <p
      v-if="validationError"
      role="alert"
      class="text-sm text-destructive"
    >
      {{ validationError }}
    </p>
  </section>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { AlignLeft, Code2, Plus, Trash2 } from 'lucide-vue-next'
import { Button, Input, Textarea } from '@/components/ui'
import {
  MAX_ROUTING_FAILOVER_PATTERN_BYTES,
  MAX_ROUTING_FAILOVER_RULES,
  validateRoutingFailoverPolicy,
  type RoutingFailoverPolicy,
  type RoutingFailoverRule,
  type RoutingFailoverRules,
} from '../utils/routingFailover'

const props = defineProps<{ modelValue: RoutingFailoverPolicy, disabled?: boolean }>()
const emit = defineEmits<{
  'update:modelValue': [value: RoutingFailoverPolicy]
  'pending-change': [value: boolean]
}>()
type RuleSection = 'success_failover_patterns' | 'error_stop_patterns'
const jsonMode = reactive<Record<RuleSection, boolean>>({
  success_failover_patterns: false,
  error_stop_patterns: false,
})
const jsonDraft = reactive<Record<RuleSection, string>>({
  success_failover_patterns: '',
  error_stop_patterns: '',
})
const jsonError = reactive<Record<RuleSection, string | null>>({
  success_failover_patterns: null,
  error_stop_patterns: null,
})
const jsonDirty = reactive<Record<RuleSection, boolean>>({
  success_failover_patterns: false,
  error_stop_patterns: false,
})
const statusDrafts = ref<Record<number, string>>({})
const ruleSections: Array<{ key: RuleSection, title: string, description: string }> = [
  { key: 'success_failover_patterns', title: '成功转移规则', description: 'HTTP 200 的响应体或流式输出前的缓冲内容命中正则时，放弃当前候选并继续转移；不是对所有 200 都重试。' },
  { key: 'error_stop_patterns', title: '错误终止规则', description: '状态码与正则同时满足时立即终止。可只填状态码，或只填正则匹配全部 400–599 错误；对流内错误使用解析后的错误状态。' },
]
const validationError = computed(() => {
  const rules = errorRulesFromForm()
  return typeof rules === 'string' ? rules : validateRoutingFailoverPolicy(props.modelValue)
})
watch(
  () => jsonDirty.success_failover_patterns || jsonDirty.error_stop_patterns || Object.keys(statusDrafts.value).length > 0,
  value => emit('pending-change', value),
  { immediate: true },
)

function updateLimit(field: 'max_transfer_count' | 'max_transfer_timeout_seconds', value: string | number) {
  if (props.disabled) return
  emit('update:modelValue', { ...props.modelValue, [field]: Number(value) })
}

function updateRules(patch: Partial<RoutingFailoverRules>) {
  if (props.disabled) return
  emit('update:modelValue', { ...props.modelValue, failover_rules: { ...props.modelValue.failover_rules, ...patch } })
}

function jsonPlaceholder(section: RuleSection): string {
  return section === 'success_failover_patterns'
    ? '[{ "pattern": "(?i)capacity.*exhausted" }]'
    : '[{ "status_codes": [400, 413], "pattern": "invalid.*parameter" }]'
}

function stringifyRules(rules: RoutingFailoverRule[]): string {
  return JSON.stringify(rules, null, 2)
}

function refreshJsonDraft(section: RuleSection, rules = props.modelValue.failover_rules[section]) {
  jsonDraft[section] = stringifyRules(rules)
  jsonError[section] = null
  jsonDirty[section] = false
}

function updateJsonDraft(section: RuleSection, value: string) {
  if (props.disabled) return
  jsonDraft[section] = value
  jsonDirty[section] = true
  jsonError[section] = null
}

function parseJsonStatusCodes(value: unknown, section: RuleSection, index: number): number[] | string {
  if (value === undefined || value === null) return []
  if (!Array.isArray(value)) return `${section} 第 ${index + 1} 条的 status_codes 必须是数组`
  const statusCodes: number[] = []
  for (const status of value) {
    if (!Number.isInteger(status)) return `${section} 第 ${index + 1} 条的 status_codes 只能包含整数`
    if (section === 'success_failover_patterns' ? status !== 200 : status < 400 || status > 599) {
      return `${section} 第 ${index + 1} 条的 status_codes 包含无效状态码`
    }
    if (!statusCodes.includes(status)) statusCodes.push(status)
  }
  return statusCodes
}

function parseJsonRules(section: RuleSection, draft: string): RoutingFailoverRule[] | string {
  let parsed: unknown
  try {
    parsed = JSON.parse(draft.trim() || '[]')
  } catch (error) {
    return error instanceof Error ? error.message : 'JSON 格式无效'
  }

  let entries: unknown = parsed
  if (!Array.isArray(parsed) && parsed !== null && typeof parsed === 'object') {
    const root = parsed as Record<string, unknown>
    const nested = root.failover_rules
    const source = nested !== null && typeof nested === 'object' && !Array.isArray(nested)
      ? nested as Record<string, unknown>
      : root
    entries = source[section]
  }
  if (!Array.isArray(entries)) return `${section} JSON 必须是数组`
  if (entries.length > MAX_ROUTING_FAILOVER_RULES) return `${section} 最多 ${MAX_ROUTING_FAILOVER_RULES} 条`

  const rules: RoutingFailoverRule[] = []
  for (const [index, entry] of entries.entries()) {
    if (entry === null || typeof entry !== 'object' || Array.isArray(entry)) {
      return `${section} 第 ${index + 1} 条必须是对象`
    }
    const rawRule = entry as Record<string, unknown>
    if (rawRule.pattern !== undefined && typeof rawRule.pattern !== 'string') {
      return `${section} 第 ${index + 1} 条的 pattern 必须是字符串`
    }
    const pattern = typeof rawRule.pattern === 'string' ? rawRule.pattern.trim() : ''
    if (new TextEncoder().encode(pattern).length > MAX_ROUTING_FAILOVER_PATTERN_BYTES) {
      return `${section} 第 ${index + 1} 条正则过长`
    }
    const statusCodes = parseJsonStatusCodes(rawRule.status_codes, section, index)
    if (typeof statusCodes === 'string') return statusCodes
    if (!pattern && (section === 'success_failover_patterns' || statusCodes.length === 0)) {
      return `${section} 第 ${index + 1} 条需要${section === 'success_failover_patterns' ? '正则表达式' : '状态码或正则表达式'}`
    }
    rules.push({ pattern, status_codes: statusCodes })
  }
  return rules
}

function applyJsonDraft(section: RuleSection): boolean {
  const parsed = parseJsonRules(section, jsonDraft[section])
  if (typeof parsed === 'string') {
    jsonError[section] = parsed
    return false
  }
  updateRules({ [section]: parsed })
  jsonDraft[section] = stringifyRules(parsed)
  jsonError[section] = null
  jsonDirty[section] = false
  return true
}

function toggleJsonMode(section: RuleSection) {
  if (props.disabled) return
  if (jsonMode[section]) {
    if (jsonDirty[section] && !applyJsonDraft(section)) return
    jsonMode[section] = false
    return
  }
  const rules = section === 'error_stop_patterns' ? errorRulesFromForm() : props.modelValue.failover_rules[section]
  if (typeof rules === 'string') return
  refreshJsonDraft(section, rules)
  if (section === 'error_stop_patterns') statusDrafts.value = {}
  jsonMode[section] = true
}

function formatJsonDraft(section: RuleSection) {
  if (props.disabled) return
  const parsed = parseJsonRules(section, jsonDraft[section])
  if (typeof parsed === 'string') {
    jsonError[section] = parsed
    return
  }
  const formatted = stringifyRules(parsed)
  if (formatted !== jsonDraft[section]) jsonDirty[section] = true
  jsonDraft[section] = formatted
  jsonError[section] = null
}

function addRule(section: RuleSection) {
  updateRules({ [section]: [...props.modelValue.failover_rules[section], { pattern: '', status_codes: [] }] })
}

function updateRule(section: RuleSection, index: number, patch: Partial<RoutingFailoverRule>) {
  updateRules({ [section]: props.modelValue.failover_rules[section].map((rule, position) => position === index ? { ...rule, ...patch } : rule) })
}

function updateStatuses(index: number, value: string | number) {
  if (props.disabled) return
  statusDrafts.value[index] = String(value)
  const codes = parseStatusInput(String(value), index)
  if (typeof codes !== 'string') updateRule('error_stop_patterns', index, { status_codes: codes })
}

function parseStatusInput(value: string, index: number): number[] | string {
  const parts = value.trim().split(/[,，\s]+/).filter(Boolean)
  if (parts.some(part => !/^\d{3}$/.test(part) || Number(part) < 400 || Number(part) > 599)) {
    return `错误终止规则第 ${index + 1} 条状态码必须为 400–599，多个状态码用逗号或空格分隔`
  }
  return [...new Set(parts.map(Number))]
}

function errorRulesFromForm(): RoutingFailoverRule[] | string {
  const rules = props.modelValue.failover_rules.error_stop_patterns.map(rule => ({ ...rule, status_codes: [...rule.status_codes] }))
  for (const [rawIndex, value] of Object.entries(statusDrafts.value)) {
    const index = Number(rawIndex)
    if (!rules[index]) continue
    const codes = parseStatusInput(value, index)
    if (typeof codes === 'string') return codes
    rules[index].status_codes = codes
  }
  return rules
}

function removeRule(section: RuleSection, index: number) {
  if (props.disabled) return
  if (section === 'error_stop_patterns') {
    const nextDrafts: Record<number, string> = {}
    for (const [rawIndex, value] of Object.entries(statusDrafts.value)) {
      const position = Number(rawIndex)
      if (position !== index) nextDrafts[position > index ? position - 1 : position] = value
    }
    statusDrafts.value = nextDrafts
  }
  updateRules({ [section]: props.modelValue.failover_rules[section].filter((_, position) => position !== index) })
}

function commitJsonDrafts(): boolean {
  if (props.disabled) return false
  const formErrors = errorRulesFromForm()
  if (typeof formErrors === 'string') return false
  const nextRules = { ...props.modelValue.failover_rules, error_stop_patterns: formErrors }
  for (const section of ruleSections.map(item => item.key)) {
    if (!jsonMode[section] || !jsonDirty[section]) continue
    const parsed = parseJsonRules(section, jsonDraft[section])
    if (typeof parsed === 'string') {
      jsonError[section] = parsed
      return false
    }
    nextRules[section] = parsed
  }
  if (validateRoutingFailoverPolicy({ ...props.modelValue, failover_rules: nextRules })) return false
  updateRules(nextRules)
  for (const { key: section } of ruleSections) {
    jsonDraft[section] = stringifyRules(nextRules[section])
    jsonError[section] = null
    jsonDirty[section] = false
  }
  statusDrafts.value = {}
  return true
}

defineExpose({ commitJsonDrafts })
</script>
