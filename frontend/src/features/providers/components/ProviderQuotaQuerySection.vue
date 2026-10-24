<template>
  <div class="space-y-3">
    <div class="flex items-center justify-between gap-2 border-b pb-2">
      <div class="flex items-center gap-1">
        <h3 class="text-sm font-medium">
          {{ legacyT('额度查询') }}
        </h3>
        <HelpHint
          portal
          content-class="max-w-md"
          :label="legacyT('额度查询')"
          :text="quotaQueryHelpText"
        />
      </div>
      <Badge
        variant="secondary"
        class="text-[10px]"
      >
        {{ legacyT('仅自定义类型') }}
      </Badge>
    </div>

    <div class="space-y-1.5">
      <Label>{{ legacyT('查询方式') }}</Label>
      <Select
        :model-value="enabled ? 'custom_api' : 'none'"
        @update:model-value="(v: any) => updateMethod(v as 'none' | 'custom_api')"
      >
        <SelectTrigger>
          <SelectValue :placeholder="legacyT('请选择')" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="none">
            {{ legacyT('不查询') }}
          </SelectItem>
          <SelectItem value="custom_api">
            {{ legacyT('自定义脚本') }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <template v-if="enabled">
      <div class="space-y-1.5">
        <div class="flex items-center justify-between">
          <Label for="quota-query-script">{{ legacyT('查询脚本（Rhai）') }}</Label>
        </div>
        <Textarea
          id="quota-query-script"
          :model-value="scriptText"
          rows="14"
          class="font-mono text-xs leading-5"
          spellcheck="false"
          @update:model-value="updateScript"
        />
        <p
          v-if="scriptError"
          class="text-xs text-destructive"
        >
          {{ scriptError }}
        </p>
        <p class="text-xs text-muted-foreground">
          {{ legacyT('保存时校验脚本语法；测试查询会执行完整请求。') }}
        </p>
      </div>

      <div class="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="outline"
          size="sm"
          :disabled="!providerId || testing"
          @click="runTest"
        >
          <Loader2
            v-if="testing"
            class="mr-1.5 h-3.5 w-3.5 animate-spin"
          />
          {{ testing ? legacyT('查询中...') : legacyT('测试查询') }}
        </Button>
        <span
          v-if="testMessage"
          class="text-xs"
          :class="testOk ? 'text-emerald-600' : 'text-destructive'"
        >
          {{ testMessage }}
        </span>
      </div>

      <div
        v-if="testResult"
        class="space-y-2 rounded-lg border bg-muted/30 p-3 text-xs"
      >
        <p class="font-medium">
          {{ legacyT('查询结果') }}
        </p>
        <div
          v-if="testFields.length"
          class="flex flex-wrap gap-x-4 gap-y-1"
        >
          <span
            v-for="field in testFields"
            :key="field.label"
            class="tabular-nums"
          >
            <span class="text-muted-foreground">{{ field.label }}：</span>{{ field.value }}
          </span>
        </div>
        <!-- 与密钥管理中的实际展示完全一致（共用同一投影与渲染组件） -->
        <template v-if="testQuotaDisplay">
          <ProviderQuotaSectionHeader
            :title="legacyT('账号配额')"
            :updated-text="legacyT('刚刚更新')"
          />
          <CodexQuotaUsageGrid
            :display="testQuotaDisplay"
            :primary-label="testPrimaryLabel"
            show-secondary
            :tick="countdownTick"
          />
        </template>
        <p
          v-else-if="testPeriodsPresent"
          class="text-muted-foreground"
        >
          {{ legacyT('periods 已提取，但缺少可计算的 used/limit，实际展示中不会渲染对应行。') }}
        </p>
      </div>
    </template>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Badge,
  Button,
  Label,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Textarea,
} from '@/components/ui'
import { Loader2 } from 'lucide-vue-next'
import HelpHint from '@/components/common/HelpHint.vue'
import { useI18n } from '@/i18n'
import { useCountdownTimer } from '@/composables/useCountdownTimer'
import CodexQuotaUsageGrid from './CodexQuotaUsageGrid.vue'
import ProviderQuotaSectionHeader from './ProviderQuotaSectionHeader.vue'
import {
  buildCustomQuotaSnapshotFromExtractorData,
  getCodexQuotaDisplayFromSnapshot,
  getCustomQuotaPrimaryLimitLabel,
} from '@/utils/codexQuotaWindow'
import { queryQuota, type ActionResultResponse } from '@/api/providerOps'
import type { QuotaQueryConfig } from '@/api/endpoints'
import type { ProviderCustomQuota } from '@/api/providerFinance'

const props = defineProps<{
  modelValue?: QuotaQueryConfig | null
  providerId?: string
}>()

const emit = defineEmits<{
  'update:modelValue': [value: QuotaQueryConfig | null]
}>()

const { legacyT } = useI18n()

// Keep `{{apiKey}}` out of the template so Vue does not parse it as
// interpolation.
const quotaQueryHelpText = computed(() =>
  legacyT('用一段 Rhai 脚本自定义额度查询：request 定义请求（完整 url / method / headers / body），extractor 把响应 JSON 映射为展示字段。仅自定义类型提供商可用。\n\n占位符：{{apiKey}} / {{api_key}} 替换为扩展操作凭据，未配置凭据时自动使用该提供商密钥池中首个可用密钥。url、headers、body 均支持占位符。脚本不存储任何密钥。\n\nmethod 支持 GET / POST / PUT / PATCH；body（可选）为 JSON 对象或 JSON 字符串，自动以 application/json 发送，例如 GraphQL：body: #{ query: "quota { remaining }" }。\n\nextractor 返回字段（均可选）：isValid（套餐是否有效）、invalidMessage（失效原因）、remaining（剩余额度）、unit（单位，如 USD）、planName（套餐名称）、total（总额度）、used（已用额度）、extra（自由展示文本）、periods（5h / week / month 用量窗口数组，每项为 { key, used, limit, reset }，reset 支持 epoch 秒/毫秒与 RFC3339）。\n\n写法提示：闭包体内需要多行逻辑时，用 let out = #{ ... }; out 返回结果；数组内嵌 map 时 [ 与 #{ 之间保留空格或换行。'),
)

const DEFAULT_SCRIPT = `#{
    request: #{
        url: "https://api.example.com/user/balance",
        method: "GET",
        headers: #{
            "Authorization": "Bearer {{apiKey}}"
        }
    },
    extractor: |response| #{
        remaining: response.balance,
        unit: "USD",
        periods: [
            #{ key: "5h", used: response.usage.five_hours.used, limit: response.usage.five_hours.limit },
            #{ key: "week", used: response.usage.week.used, limit: response.usage.week.limit },
            #{ key: "month", used: response.usage.month.used, limit: response.usage.month.limit }
        ]
    }
}`

const enabled = computed(() => !!props.modelValue)

const scriptText = computed(() => {
  const script = props.modelValue?.script
  return script && script.trim() ? script : DEFAULT_SCRIPT
})

const scriptError = ref('')

function updateMethod(next: 'none' | 'custom_api') {
  scriptError.value = ''
  emit('update:modelValue', next === 'custom_api' ? { script: DEFAULT_SCRIPT } : null)
}

function updateScript(value: string | number) {
  const text = String(value)
  const trimmed = text.trim()
  if (!trimmed) {
    scriptError.value = legacyT('脚本不能为空')
    return
  }
  if (trimmed.length > 16 * 1024) {
    scriptError.value = legacyT('脚本长度不能超过 16KB')
    return
  }
  scriptError.value = ''
  emit('update:modelValue', { script: trimmed })
}

// ===== 测试查询 =====
const testing = ref(false)
const testOk = ref(false)
const testMessage = ref('')
const testResult = ref<ProviderCustomQuota | null>(null)

const FIELD_LABELS: { key: keyof ProviderCustomQuota; label: string }[] = [
  { key: 'plan_name', label: '套餐' },
  { key: 'remaining', label: '剩余' },
  { key: 'total', label: '总额' },
  { key: 'used', label: '已用' },
  { key: 'unit', label: '单位' },
  { key: 'extra', label: '备注' },
]

// The test result renders through the same snapshot projection and grid
// component as the persisted key snapshots in 密钥管理.
const testQuotaDisplay = computed(() => {
  if (!testResult.value) return null
  const snapshot = buildCustomQuotaSnapshotFromExtractorData(testResult.value)
  return snapshot ? getCodexQuotaDisplayFromSnapshot(snapshot) : null
})

const testPrimaryLabel = computed(() => {
  const windows = buildCustomQuotaSnapshotFromExtractorData(testResult.value)?.windows
  return getCustomQuotaPrimaryLimitLabel(windows) || legacyT('周限额')
})

const testPeriodsPresent = computed(() => (testResult.value?.periods?.length ?? 0) > 0)

const { tick: countdownTick, start: startCountdown, stop: stopCountdown } = useCountdownTimer()
watch(testResult, (value) => {
  if (value) startCountdown()
  else stopCountdown()
})

/** Follow the extractor's unit: percent renders inline, known currencies get
 * a symbol, anything else is appended as a suffix. */
function formatWithUnit(value: number | null | undefined, unit: string | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(Number(value))) {
    return '-'
  }
  const normalized = unit?.trim()
  if (!normalized || normalized.toUpperCase() === 'USD') {
    return `$${Number(value).toFixed(2)}`
  }
  if (normalized === '%') {
    return `${Number(value)}%`
  }
  return `${Number(value)} ${normalized}`
}

const testFields = computed(() => {
  const result = testResult.value
  if (!result) {
    return []
  }
  const fields: { label: string; value: string }[] = []
  if (result.is_valid === false) {
    fields.push({
      label: legacyT('状态'),
      value: result.invalid_message || legacyT('套餐已失效'),
    })
  }
  for (const field of FIELD_LABELS) {
    const value = result[field.key]
    if (value === null || value === undefined || value === '') {
      continue
    }
    const text =
      field.key === 'remaining' || field.key === 'total' || field.key === 'used'
        ? formatWithUnit(value as number, result.unit)
        : String(value)
    fields.push({ label: legacyT(field.label), value: text })
  }
  return fields
})

async function runTest() {
  if (!props.providerId || testing.value) {
    return
  }
  testing.value = true
  testOk.value = false
  testMessage.value = ''
  testResult.value = null
  try {
    const result: ActionResultResponse = await queryQuota(props.providerId, {
      config: { script: scriptText.value },
    })
    if (result.status === 'success') {
      testResult.value = (result.data as ProviderCustomQuota | null) ?? null
      testOk.value = true
      testMessage.value = legacyT('查询成功')
    } else {
      testOk.value = false
      testMessage.value = result.message || result.status
    }
  } catch (error) {
    testOk.value = false
    testMessage.value = error instanceof Error ? error.message : legacyT('查询失败')
  } finally {
    testing.value = false
  }
}
</script>
