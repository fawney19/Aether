<template>
  <Dialog
    :model-value="open"
    title="故障转移规则"
    description="配置提供商级别的故障转移规则。默认所有错误都会触发转移，此处可自定义例外。"
    :icon="GitBranch"
    size="lg"
    @update:model-value="handleClose"
  >
    <div class="space-y-5 max-h-[60vh] overflow-y-auto px-0.5 py-0.5 -mx-0.5">
      <!-- 成功转移规则 -->
      <div class="space-y-3">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <h3 class="text-sm font-medium">
              成功转移规则
            </h3>
            <p class="text-xs text-muted-foreground mt-0.5">
              HTTP 200 但响应体匹配正则时，视为失败并触发转移
            </p>
          </div>
          <div class="flex items-center gap-1 shrink-0">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              class="h-7 text-xs px-2"
              :title="successJsonMode ? '切回成功转移表单' : '切到成功转移 JSON'"
              @click="toggleSuccessJsonMode"
            >
              <Code2 class="w-3 h-3 mr-1" />
              {{ successJsonMode ? '表单' : 'JSON' }}
            </Button>
            <Button
              v-if="successJsonMode"
              type="button"
              variant="ghost"
              size="sm"
              class="h-7 px-2 text-xs"
              title="格式化成功转移 JSON"
              @click="formatSuccessJsonDraft"
            >
              <AlignLeft class="w-3 h-3 mr-1" />
              格式化
            </Button>
            <Button
              v-if="!successJsonMode"
              type="button"
              variant="ghost"
              size="sm"
              class="h-7 text-xs px-2"
              @click="addRule('success')"
            >
              <Plus class="w-3 h-3 mr-1" />
              添加
            </Button>
          </div>
        </div>

        <div
          v-if="successJsonMode"
          class="space-y-2"
        >
          <Textarea
            :model-value="successJsonDraft"
            class="min-h-[160px] font-mono text-xs leading-relaxed"
            spellcheck="false"
            placeholder="[{ &quot;pattern&quot;: &quot;relay:.*格式错误&quot; }]"
            @update:model-value="updateSuccessJsonDraft"
          />
          <div
            v-if="successJsonError"
            class="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {{ successJsonError }}
          </div>
          <p class="text-xs text-muted-foreground">
            仅管理成功转移规则；JSON 应为数组，<code class="bg-muted px-1 rounded">pattern</code> 必填。
          </p>
        </div>

        <template v-else>
          <div
            v-if="successPatterns.length === 0"
            class="text-xs text-muted-foreground px-3 py-4 border border-dashed rounded-lg text-center"
          >
            暂无规则
          </div>

          <div
            v-for="(rule, index) in successPatterns"
            :key="'s-' + index"
            class="flex items-center gap-1"
          >
            <Input
              v-model="rule.pattern"
              placeholder="例如: relay:.*格式错误"
              size="sm"
              class="font-mono text-xs flex-1"
            />
            <Button
              variant="ghost"
              size="sm"
              class="shrink-0 h-8 w-8 p-0 text-muted-foreground hover:text-destructive"
              @click="removeRule('success', index)"
            >
              <Trash2 class="w-3.5 h-3.5" />
            </Button>
          </div>
        </template>
      </div>

      <!-- 错误终止规则 -->
      <div class="space-y-3">
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <h3 class="text-sm font-medium">
              错误终止规则
            </h3>
            <p class="text-xs text-muted-foreground mt-0.5">
              HTTP 非 200 且规则命中时，停止转移并直接返回错误。状态码不填则所有错误状态都尝试匹配正则
            </p>
          </div>
          <div class="flex items-center gap-1 shrink-0">
            <Button
              type="button"
              variant="ghost"
              size="sm"
              class="h-7 text-xs px-2"
              :title="errorJsonMode ? '切回错误终止表单' : '切到错误终止 JSON'"
              @click="toggleErrorJsonMode"
            >
              <Code2 class="w-3 h-3 mr-1" />
              {{ errorJsonMode ? '表单' : 'JSON' }}
            </Button>
            <Button
              v-if="errorJsonMode"
              type="button"
              variant="ghost"
              size="sm"
              class="h-7 px-2 text-xs"
              title="格式化错误终止 JSON"
              @click="formatErrorJsonDraft"
            >
              <AlignLeft class="w-3 h-3 mr-1" />
              格式化
            </Button>
            <Button
              v-if="!errorJsonMode"
              type="button"
              variant="ghost"
              size="sm"
              class="h-7 text-xs px-2"
              @click="addRule('error')"
            >
              <Plus class="w-3 h-3 mr-1" />
              添加
            </Button>
          </div>
        </div>

        <div
          v-if="errorJsonMode"
          class="space-y-2"
        >
          <Textarea
            :model-value="errorJsonDraft"
            class="min-h-[180px] font-mono text-xs leading-relaxed"
            spellcheck="false"
            placeholder="[{ &quot;pattern&quot;: &quot;content_policy_violation&quot; }, { &quot;status_codes&quot;: [429, 500, 503], &quot;pattern&quot;: &quot;&quot; }]"
            @update:model-value="updateErrorJsonDraft"
          />
          <div
            v-if="errorJsonError"
            class="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {{ errorJsonError }}
          </div>
          <p class="text-xs text-muted-foreground">
            仅管理错误终止规则；JSON 应为数组，<code class="bg-muted px-1 rounded">status_codes</code> 选填数组，不填则所有错误状态都尝试匹配 <code class="bg-muted px-1 rounded">pattern</code>；两者至少填一个。
          </p>
        </div>

        <template v-else>
          <div
            v-if="errorPatterns.length === 0"
            class="text-xs text-muted-foreground px-3 py-4 border border-dashed rounded-lg text-center"
          >
            暂无规则
          </div>

          <div
            v-for="(rule, index) in errorPatterns"
            :key="'e-' + index"
            class="flex items-center gap-1"
          >
            <Input
              v-model="statusCodeInputs[index]"
              placeholder="状态码 (选填，可多个)"
              size="sm"
              class="font-mono text-xs w-40 shrink-0"
            />
            <Input
              v-model="rule.pattern"
              placeholder="正则内容 (选填)"
              size="sm"
              class="font-mono text-xs flex-1"
            />
            <Button
              variant="ghost"
              size="sm"
              class="shrink-0 h-8 w-8 p-0 text-muted-foreground hover:text-destructive"
              @click="removeRule('error', index)"
            >
              <Trash2 class="w-3.5 h-3.5" />
            </Button>
          </div>
        </template>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="saving"
        @click="handleClose"
      >
        取消
      </Button>
      <Button
        :disabled="saving"
        @click="handleSave"
      >
        {{ saving ? '保存中...' : '保存' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import {
  Dialog,
  Button,
  Input,
  Textarea,
} from '@/components/ui'
import { AlignLeft, Code2, GitBranch, Plus, Trash2 } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { updateProvider, type ProviderWithEndpointsSummary } from '@/api/endpoints'
import { parseApiError } from '@/utils/errorParser'
import type { FailoverRuleItem, FailoverRulesConfig } from '@/api/endpoints/types'

const props = defineProps<{
  open: boolean
  provider: ProviderWithEndpointsSummary | null
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'saved': []
}>()

const { success, error: showError } = useToast()
const saving = ref(false)

const successPatterns = ref<FailoverRuleItem[]>([])
const errorPatterns = ref<FailoverRuleItem[]>([])
const statusCodeInputs = ref<string[]>([])
const successJsonMode = ref(false)
const successJsonDraft = ref('')
const successJsonError = ref<string | null>(null)
const successJsonDirty = ref(false)
const errorJsonMode = ref(false)
const errorJsonDraft = ref('')
const errorJsonError = ref<string | null>(null)
const errorJsonDirty = ref(false)

const TOP_LEVEL_STOP_STATUS_CODE_KEYS = [
  'stop_status_codes',
  'stop_on_status_codes',
  'early_stop_status_codes',
  'non_retryable_status_codes',
] as const

const MANAGED_FAILOVER_RULE_KEYS = [
  'success_failover_patterns',
  'error_stop_patterns',
  ...TOP_LEVEL_STOP_STATUS_CODE_KEYS,
] as const

function isJsonObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}

function uniqueStatusCodes(codes: number[]): number[] {
  return codes.filter((code, index, values) =>
    Number.isInteger(code) && code >= 100 && code <= 599 && values.indexOf(code) === index
  )
}

function collectTopLevelStopStatusCodes(rules: ProviderWithEndpointsSummary['failover_rules']): number[] {
  if (!rules) return []
  return uniqueStatusCodes(TOP_LEVEL_STOP_STATUS_CODE_KEYS.flatMap(key => rules[key] || []))
}

function hasPersistableFailoverValue(value: unknown): boolean {
  if (value === null || value === undefined) return false
  if (Array.isArray(value)) return value.length > 0
  if (typeof value === 'string') return value.trim().length > 0
  if (isJsonObject(value)) return Object.values(value).some(hasPersistableFailoverValue)
  return true
}

function buildPreservedFailoverRules(
  rules: ProviderWithEndpointsSummary['failover_rules'],
): Record<string, unknown> {
  if (!isJsonObject(rules)) return {}
  const preserved: Record<string, unknown> = { ...rules }
  for (const key of MANAGED_FAILOVER_RULE_KEYS) {
    delete preserved[key]
  }
  return preserved
}

function buildNextFailoverRules(
  filteredSuccess: FailoverRuleItem[],
  filteredError: FailoverRuleItem[],
): FailoverRulesConfig | null {
  const nextRules = buildPreservedFailoverRules(props.provider?.failover_rules)
  if (filteredSuccess.length > 0) {
    nextRules.success_failover_patterns = filteredSuccess
  }
  if (filteredError.length > 0) {
    nextRules.error_stop_patterns = filteredError
  }

  return Object.values(nextRules).some(hasPersistableFailoverValue)
    ? nextRules as FailoverRulesConfig
    : null
}

watch(() => [props.open, props.provider], () => {
  if (props.open && props.provider) {
    const rules = props.provider.failover_rules
    successPatterns.value = (rules?.success_failover_patterns || []).map(r => ({
      ...r,
      pattern: r.pattern || '',
    }))
    errorPatterns.value = (rules?.error_stop_patterns || []).map(r => ({
      ...r,
      pattern: r.pattern || '',
    }))
    const topLevelStopStatusCodes = collectTopLevelStopStatusCodes(rules)
    if (topLevelStopStatusCodes.length > 0) {
      errorPatterns.value.push({
        pattern: '',
        description: '',
        status_codes: topLevelStopStatusCodes,
      })
    }
    statusCodeInputs.value = errorPatterns.value.map(r =>
      r.status_codes?.length ? r.status_codes.join(',') : ''
    )
    successJsonMode.value = false
    errorJsonMode.value = false
    refreshSuccessJsonDraft()
    refreshErrorJsonDraft()
  }
}, { immediate: true })

function addRule(type: 'success' | 'error') {
  const rule: FailoverRuleItem = { pattern: '', description: '' }
  if (type === 'success') {
    successPatterns.value.push(rule)
  } else {
    errorPatterns.value.push(rule)
    statusCodeInputs.value.push('')
  }
}

function removeRule(type: 'success' | 'error', index: number) {
  if (type === 'success') {
    successPatterns.value.splice(index, 1)
  } else {
    errorPatterns.value.splice(index, 1)
    statusCodeInputs.value.splice(index, 1)
  }
}

function handleClose() {
  emit('update:open', false)
}

function parseStatusCodes(
  input: string,
  required = false,
): { valid: true; codes: number[] } | { valid: false; reason: string } {
  const trimmed = input.trim()
  if (!trimmed) {
    return required
      ? { valid: false, reason: '状态码不能为空' }
      : { valid: true, codes: [] }
  }
  const parts = trimmed.split(/[,\s]+/)
  const codes: number[] = []
  for (const part of parts) {
    if (!part) continue
    if (!/^\d+$/.test(part)) return { valid: false, reason: `"${part}" 不是有效数字` }
    const n = parseInt(part, 10)
    if (n < 100 || n > 599) return { valid: false, reason: `${n} 不在 100-599 范围内` }
    codes.push(n)
  }
  const uniqueCodes = Array.from(new Set(codes))
  return uniqueCodes.length > 0
    ? { valid: true, codes: uniqueCodes }
    : required
      ? { valid: false, reason: '状态码不能为空' }
      : { valid: true, codes: [] }
}

function validatePattern(pattern: string): string | null {
  if (!pattern.trim()) return '正则表达式不能为空'
  return validateOptionalPattern(pattern)
}

function validateOptionalPattern(pattern: string): string | null {
  if (!pattern.trim()) return null
  try {
    new RegExp(pattern)
    return null
  } catch {
    return `无效的正则表达式: ${pattern}`
  }
}

function buildSuccessJsonRulesFromForm(): FailoverRuleItem[] {
  return successPatterns.value
    .map(rule => ({
      ...rule,
      pattern: rule.pattern.trim(),
      status_codes: rule.status_codes?.length ? uniqueStatusCodes(rule.status_codes) : undefined,
    }))
    .filter(rule => rule.pattern)
}

function buildErrorJsonRulesFromForm(): FailoverRuleItem[] {
  return errorPatterns.value
    .map((rule, index) => {
      const parsed = parseStatusCodes(statusCodeInputs.value[index] || '')
      const statusCodes = parsed.valid
        ? parsed.codes
        : uniqueStatusCodes(rule.status_codes || [])
      return {
        ...rule,
        pattern: rule.pattern.trim(),
        status_codes: statusCodes.length > 0 ? statusCodes : undefined,
      }
    })
    .filter(rule => rule.pattern || (rule.status_codes?.length || 0) > 0)
}

function stringifyRuleArray(rules: FailoverRuleItem[]): string {
  return JSON.stringify(rules, null, 2)
}

function refreshSuccessJsonDraft() {
  successJsonDraft.value = stringifyRuleArray(buildSuccessJsonRulesFromForm())
  successJsonError.value = null
  successJsonDirty.value = false
}

function refreshErrorJsonDraft() {
  errorJsonDraft.value = stringifyRuleArray(buildErrorJsonRulesFromForm())
  errorJsonError.value = null
  errorJsonDirty.value = false
}

function updateSuccessJsonDraft(value: string) {
  successJsonDraft.value = value
  successJsonDirty.value = true
  successJsonError.value = null
}

function updateErrorJsonDraft(value: string) {
  errorJsonDraft.value = value
  errorJsonDirty.value = true
  errorJsonError.value = null
}

function readJsonRuleArray(
  parsed: unknown,
  key: 'success_failover_patterns' | 'error_stop_patterns',
): { root: Record<string, unknown>; value: unknown[]; error: string | null } {
  if (Array.isArray(parsed)) return { root: {}, value: parsed, error: null }
  if (!isJsonObject(parsed)) return { root: {}, value: [], error: '规则 JSON 必须是数组或对象' }
  const root = isJsonObject(parsed.failover_rules) ? parsed.failover_rules : parsed
  const raw = root[key]
  if (raw === undefined || raw === null) return { root, value: [], error: null }
  if (!Array.isArray(raw)) return { root, value: [], error: `${key} 必须是数组或 null` }
  return { root, value: raw, error: null }
}

function parseJsonStatusCodes(
  value: unknown,
  label: string,
  required: boolean,
): { codes: number[]; error: string | null } {
  if (value === undefined || value === null) {
    return required
      ? { codes: [], error: `${label}status_codes 必填` }
      : { codes: [], error: null }
  }
  if (!Array.isArray(value)) return { codes: [], error: `${label}status_codes 必须是数组` }
  const codes: number[] = []
  for (const item of value) {
    if (!Number.isInteger(item)) {
      return { codes: [], error: `${label}status_codes 只能包含整数` }
    }
    const code = item as number
    if (code < 100 || code > 599) {
      return { codes: [], error: `${label}status_codes 只能填写 100-599` }
    }
    codes.push(code)
  }
  const uniqueCodes = uniqueStatusCodes(codes)
  if (required && uniqueCodes.length === 0) {
    return { codes: [], error: `${label}status_codes 必填` }
  }
  return { codes: uniqueCodes, error: null }
}

function parseJsonSuccessRule(rule: unknown, index: number): { rule: FailoverRuleItem | null; error: string | null } {
  const label = `成功转移 JSON 第 ${index + 1} 条：`
  if (!isJsonObject(rule)) return { rule: null, error: `${label}必须是对象` }
  if (typeof rule.pattern !== 'string' || !rule.pattern.trim()) {
    return { rule: null, error: `${label}pattern 必填` }
  }
  const pattern = rule.pattern.trim()
  const patternError = validatePattern(pattern)
  if (patternError) return { rule: null, error: `${label}${patternError}` }
  const status = parseJsonStatusCodes(rule.status_codes, label, false)
  if (status.error) return { rule: null, error: status.error }

  return {
    rule: {
      pattern,
      description: typeof rule.description === 'string' ? rule.description : undefined,
      status_codes: status.codes.length > 0 ? status.codes : undefined,
    },
    error: null,
  }
}

function parseJsonErrorRule(rule: unknown, index: number): { rule: FailoverRuleItem | null; error: string | null } {
  const label = `错误终止 JSON 第 ${index + 1} 条：`
  if (!isJsonObject(rule)) return { rule: null, error: `${label}必须是对象` }
  const status = parseJsonStatusCodes(rule.status_codes, label, false)
  if (status.error) return { rule: null, error: status.error }
  if (rule.pattern !== undefined && rule.pattern !== null && typeof rule.pattern !== 'string') {
    return { rule: null, error: `${label}pattern 必须是字符串` }
  }
  const pattern = typeof rule.pattern === 'string' ? rule.pattern.trim() : ''
  const patternError = validateOptionalPattern(pattern)
  if (patternError) return { rule: null, error: `${label}${patternError}` }
  if (status.codes.length === 0 && !pattern) {
    return { rule: null, error: `${label}status_codes 和 pattern 至少填写一个` }
  }

  return {
    rule: {
      pattern,
      description: typeof rule.description === 'string' ? rule.description : undefined,
      status_codes: status.codes.length > 0 ? status.codes : undefined,
    },
    error: null,
  }
}

function parseSuccessRulesJsonDraft(draft: string): { value: FailoverRuleItem[] | null; error: string | null } {
  const raw = draft.trim()
  if (!raw) return { value: [], error: null }

  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch (error: unknown) {
    return { value: null, error: error instanceof Error ? error.message : 'JSON 格式无效' }
  }

  const rules = readJsonRuleArray(parsed, 'success_failover_patterns')
  if (rules.error) return { value: null, error: rules.error }

  const normalized: FailoverRuleItem[] = []
  for (let i = 0; i < rules.value.length; i++) {
    const parsedRule = parseJsonSuccessRule(rules.value[i], i)
    if (parsedRule.error || !parsedRule.rule) return { value: null, error: parsedRule.error }
    normalized.push(parsedRule.rule)
  }
  return { value: normalized, error: null }
}

function parseErrorRulesJsonDraft(draft: string): { value: FailoverRuleItem[] | null; error: string | null } {
  const raw = draft.trim()
  if (!raw) return { value: [], error: null }

  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch (error: unknown) {
    return { value: null, error: error instanceof Error ? error.message : 'JSON 格式无效' }
  }

  const rules = readJsonRuleArray(parsed, 'error_stop_patterns')
  if (rules.error) return { value: null, error: rules.error }

  const normalized: FailoverRuleItem[] = []
  for (let i = 0; i < rules.value.length; i++) {
    const parsedRule = parseJsonErrorRule(rules.value[i], i)
    if (parsedRule.error || !parsedRule.rule) return { value: null, error: parsedRule.error }
    normalized.push(parsedRule.rule)
  }

  for (const key of TOP_LEVEL_STOP_STATUS_CODE_KEYS) {
    const status = parseJsonStatusCodes(rules.root[key], `${key}：`, false)
    if (status.error) return { value: null, error: status.error }
    if (status.codes.length > 0) {
      normalized.push({
        pattern: '',
        description: '',
        status_codes: status.codes,
      })
    }
  }

  return { value: normalized, error: null }
}

function applySuccessJsonDraft(options: { notify?: boolean; notifyError?: boolean } = {}): boolean {
  const notifyError = options.notifyError !== false
  const parsed = parseSuccessRulesJsonDraft(successJsonDraft.value)
  if (!parsed.value) {
    successJsonError.value = parsed.error
    if (notifyError) showError(parsed.error || '成功转移规则 JSON 无效', '验证失败')
    return false
  }

  successPatterns.value = parsed.value.map(rule => ({ ...rule }))
  successJsonDraft.value = stringifyRuleArray(parsed.value)
  successJsonError.value = null
  successJsonDirty.value = false
  if (options.notify !== false) success('成功转移 JSON 已应用')
  return true
}

function applyErrorJsonDraft(options: { notify?: boolean; notifyError?: boolean } = {}): boolean {
  const notifyError = options.notifyError !== false
  const parsed = parseErrorRulesJsonDraft(errorJsonDraft.value)
  if (!parsed.value) {
    errorJsonError.value = parsed.error
    if (notifyError) showError(parsed.error || '错误终止规则 JSON 无效', '验证失败')
    return false
  }

  errorPatterns.value = parsed.value.map(rule => ({ ...rule }))
  statusCodeInputs.value = errorPatterns.value.map(rule => rule.status_codes?.join(',') || '')
  errorJsonDraft.value = stringifyRuleArray(parsed.value)
  errorJsonError.value = null
  errorJsonDirty.value = false
  if (options.notify !== false) success('错误终止 JSON 已应用')
  return true
}

function toggleSuccessJsonMode() {
  if (successJsonMode.value) {
    if (successJsonDirty.value && !applySuccessJsonDraft({ notify: false })) return
    successJsonMode.value = false
    return
  }
  refreshSuccessJsonDraft()
  successJsonMode.value = true
}

function toggleErrorJsonMode() {
  if (errorJsonMode.value) {
    if (errorJsonDirty.value && !applyErrorJsonDraft({ notify: false })) return
    errorJsonMode.value = false
    return
  }
  refreshErrorJsonDraft()
  errorJsonMode.value = true
}

function formatSuccessJsonDraft() {
  const currentDraft = successJsonDraft.value
  const parsed = parseSuccessRulesJsonDraft(currentDraft)
  if (!parsed.value) {
    successJsonError.value = parsed.error
    return
  }
  const formattedDraft = stringifyRuleArray(parsed.value)
  successJsonDraft.value = formattedDraft
  successJsonError.value = null
  if (formattedDraft !== currentDraft) {
    successJsonDirty.value = true
  }
}

function formatErrorJsonDraft() {
  const currentDraft = errorJsonDraft.value
  const parsed = parseErrorRulesJsonDraft(currentDraft)
  if (!parsed.value) {
    errorJsonError.value = parsed.error
    return
  }
  const formattedDraft = stringifyRuleArray(parsed.value)
  errorJsonDraft.value = formattedDraft
  errorJsonError.value = null
  if (formattedDraft !== currentDraft) {
    errorJsonDirty.value = true
  }
}

async function handleSave() {
  if (!props.provider) return

  if (successJsonMode.value && !applySuccessJsonDraft({ notify: false })) return
  if (errorJsonMode.value && !applyErrorJsonDraft({ notify: false })) return

  // Validate patterns
  for (const rule of successPatterns.value) {
    const err = validatePattern(rule.pattern)
    if (err) {
      showError(err, '验证失败')
      return
    }
  }

  // Parse and validate status codes from raw inputs
  for (let i = 0; i < errorPatterns.value.length; i++) {
    const raw = statusCodeInputs.value[i]?.trim() || ''
    const result = parseStatusCodes(raw)
    if (!result.valid) {
      showError(`状态码格式错误: ${result.reason}，请输入 100-599 之间的整数，多个用逗号分隔`, '验证失败')
      return
    }
    const patternErr = validateOptionalPattern(errorPatterns.value[i].pattern)
    if (patternErr) {
      showError(patternErr, '验证失败')
      return
    }
    const pattern = errorPatterns.value[i].pattern.trim()
    if (result.codes.length === 0 && !pattern) {
      showError(`第 ${i + 1} 条错误终止规则：状态码和正则内容至少填写一个`, '验证失败')
      return
    }
    errorPatterns.value[i].status_codes = result.codes.length > 0 ? result.codes : undefined
  }

  saving.value = true
  try {
    const filteredSuccess = successPatterns.value
      .map(r => ({ ...r, pattern: r.pattern.trim() }))
      .filter(r => r.pattern)
    const filteredError = errorPatterns.value
      .map(r => {
        const statusCodes = r.status_codes?.length ? r.status_codes : undefined
        return {
          ...r,
          pattern: r.pattern.trim(),
          status_codes: statusCodes,
        }
      })
      .filter(r => r.pattern || (r.status_codes?.length || 0) > 0)

    const nextFailoverRules = buildNextFailoverRules(filteredSuccess, filteredError)

    await updateProvider(props.provider.id, {
      failover_rules: nextFailoverRules,
    })

    success('故障转移规则已保存')
    emit('saved')
    handleClose()
  } catch (err: unknown) {
    showError(parseApiError(err, '保存故障转移规则失败'), '保存失败')
  } finally {
    saving.value = false
  }
}
</script>
