<template>
  <Dialog
    :model-value="modelValue"
    title="高级设置"
    description="冷却、健康、成本控制与其他高级参数"
    size="lg"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="space-y-4">
      <!-- Cooldown & Health -->
      <div class="space-y-3">
        <h3 class="text-sm font-medium border-b pb-2">
          冷却与健康
        </h3>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">健康策略</span>
            <p class="text-xs text-muted-foreground">
              按上游错误自动冷却并跳过账号
            </p>
          </div>
          <Switch
            :model-value="form.health_policy_enabled"
            @update:model-value="(v: boolean) => form.health_policy_enabled = v"
          />
        </div>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">主动探测</span>
            <p class="text-xs text-muted-foreground">
              按固定间隔主动刷新 Key 的账号状态与额度
            </p>
          </div>
          <Switch
            :model-value="form.probing_enabled"
            @update:model-value="(v: boolean) => form.probing_enabled = v"
          />
        </div>
        <div
          v-if="form.probing_enabled"
          class="grid grid-cols-2 gap-4"
        >
          <div class="space-y-1.5">
            <Label>
              探测间隔
              <span class="text-xs text-muted-foreground">(分钟)</span>
            </Label>
            <Input
              :model-value="form.probing_interval_minutes ?? ''"
              type="number"
              min="1"
              max="1440"
              placeholder="10"
              @update:model-value="(v) => form.probing_interval_minutes = parseNum(v)"
            />
          </div>
        </div>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">异常自动清除</span>
            <p class="text-xs text-muted-foreground">
              仅在检测到不可恢复的账号异常时自动从号池中移除，不处理纯 Token 失效
            </p>
          </div>
          <Switch
            :model-value="form.auto_remove_banned_keys"
            @update:model-value="(v: boolean) => form.auto_remove_banned_keys = v"
          />
        </div>
        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label>
              429 冷却
              <span class="text-xs text-muted-foreground">(秒)</span>
            </Label>
            <Input
              :model-value="form.rate_limit_cooldown_seconds ?? ''"
              type="number"
              min="10"
              max="3600"
              placeholder="300"
              @update:model-value="(v) => form.rate_limit_cooldown_seconds = parseNum(v)"
            />
          </div>
          <div class="space-y-1.5">
            <Label>
              529 冷却
              <span class="text-xs text-muted-foreground">(秒)</span>
            </Label>
            <Input
              :model-value="form.overload_cooldown_seconds ?? ''"
              type="number"
              min="5"
              max="600"
              placeholder="30"
              @update:model-value="(v) => form.overload_cooldown_seconds = parseNum(v)"
            />
          </div>
        </div>
        <div class="space-y-1.5">
          <Label>
            粘性会话 TTL
            <span class="text-xs text-muted-foreground">(秒)</span>
          </Label>
          <Input
            :model-value="form.sticky_session_ttl_seconds ?? ''"
            type="number"
            min="60"
            max="86400"
            placeholder="3600 (留空禁用)"
            @update:model-value="(v) => form.sticky_session_ttl_seconds = parseNum(v)"
          />
        </div>
      </div>

      <!-- Batch Operations -->
      <div class="space-y-3">
        <h3 class="text-sm font-medium border-b pb-2">
          批量操作
        </h3>
        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label>
              并发数
            </Label>
            <Input
              :model-value="form.batch_concurrency ?? ''"
              type="number"
              min="1"
              max="32"
              placeholder="8"
              @update:model-value="(v) => form.batch_concurrency = parseNum(v)"
            />
            <p class="text-[11px] text-muted-foreground">
              批量刷新 OAuth / 额度等操作的并行请求数
            </p>
          </div>
        </div>
      </div>

      <!-- Claude Code -->
      <div
        v-if="isClaudeCode"
        class="space-y-3"
      >
        <h3 class="text-sm font-medium border-b pb-2">
          Claude Code
        </h3>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">Session ID 伪装</span>
            <p class="text-xs text-muted-foreground">
              固定 metadata.user_id 中 session 片段
            </p>
          </div>
          <Switch
            :model-value="claudeForm.session_id_masking_enabled"
            @update:model-value="(v: boolean) => claudeForm.session_id_masking_enabled = v"
          />
        </div>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">仅限 CLI 客户端</span>
            <p class="text-xs text-muted-foreground">
              仅允许 Claude Code CLI 格式请求
            </p>
          </div>
          <Switch
            :model-value="claudeForm.cli_only_enabled"
            @update:model-value="(v: boolean) => claudeForm.cli_only_enabled = v"
          />
        </div>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">Cache TTL 统一</span>
            <p class="text-xs text-muted-foreground">
              强制所有 cache_control 使用相同 TTL 类型
            </p>
          </div>
          <Switch
            :model-value="claudeForm.cache_ttl_override_enabled"
            @update:model-value="(v: boolean) => claudeForm.cache_ttl_override_enabled = v"
          />
        </div>
        <div
          v-if="claudeForm.cache_ttl_override_enabled"
          class="pl-3"
        >
          <div class="space-y-1.5">
            <Label>TTL 类型</Label>
            <div class="flex gap-0.5 p-0.5 bg-muted/40 rounded-md w-fit">
              <button
                v-for="opt in ['ephemeral']"
                :key="opt"
                type="button"
                class="px-2.5 py-1 text-xs font-medium rounded transition-all"
                :class="[
                  claudeForm.cache_ttl_override_target === opt
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:text-foreground hover:bg-background/50'
                ]"
                @click="claudeForm.cache_ttl_override_target = opt"
              >
                {{ opt }}
              </button>
            </div>
          </div>
        </div>
        <div class="flex items-center justify-between p-3 border rounded-lg bg-muted/50">
          <div class="space-y-0.5">
            <span class="text-sm font-medium">会话数量控制</span>
            <p class="text-xs text-muted-foreground">
              限制单 Key 同时活跃会话数
            </p>
          </div>
          <Switch
            :model-value="claudeForm.session_control_enabled"
            @update:model-value="(v: boolean) => claudeForm.session_control_enabled = v"
          />
        </div>
        <div
          v-if="claudeForm.session_control_enabled"
          class="grid grid-cols-2 gap-4"
        >
          <div class="space-y-1.5">
            <Label>
              最大会话数
            </Label>
            <Input
              :model-value="claudeForm.max_sessions ?? ''"
              type="number"
              min="1"
              max="100"
              placeholder="留空 = 不限"
              @update:model-value="(v) => claudeForm.max_sessions = parseNum(v)"
            />
          </div>
          <div class="space-y-1.5">
            <Label>
              空闲超时
              <span class="text-xs text-muted-foreground">(分钟)</span>
            </Label>
            <Input
              :model-value="claudeForm.session_idle_timeout_minutes ?? ''"
              type="number"
              min="1"
              max="1440"
              placeholder="5"
              @update:model-value="(v) => claudeForm.session_idle_timeout_minutes = parseNum(v) ?? 5"
            />
          </div>
        </div>
      </div>

      <!-- Cost Control -->
      <div class="space-y-3">
        <h3 class="text-sm font-medium border-b pb-2">
          成本控制
        </h3>
        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label>
              成本窗口
              <span class="text-xs text-muted-foreground">(秒)</span>
            </Label>
            <Input
              :model-value="form.cost_window_seconds ?? ''"
              type="number"
              min="3600"
              max="86400"
              placeholder="18000 (5 小时)"
              @update:model-value="(v) => form.cost_window_seconds = parseNum(v)"
            />
          </div>
          <div class="space-y-1.5">
            <Label>
              Key 窗口限额
              <span class="text-xs text-muted-foreground">(tokens)</span>
            </Label>
            <Input
              :model-value="form.cost_limit_per_key_tokens ?? ''"
              type="number"
              min="0"
              placeholder="留空 = 不限"
              @update:model-value="(v) => form.cost_limit_per_key_tokens = parseNum(v)"
            />
          </div>
          <div class="space-y-1.5">
            <Label>
              软阈值
              <span class="text-xs text-muted-foreground">(%)</span>
            </Label>
            <Input
              :model-value="form.cost_soft_threshold_percent ?? ''"
              type="number"
              min="0"
              max="100"
              placeholder="80"
              @update:model-value="(v) => form.cost_soft_threshold_percent = parseNum(v)"
            />
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        :disabled="loading"
        @click="emit('update:modelValue', false)"
      >
        取消
      </Button>
      <Button
        :disabled="loading"
        @click="handleSave"
      >
        {{ loading ? '保存中...' : '保存' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Dialog, Button, Input, Label, Switch } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { updateProvider } from '@/api/endpoints'
import type {
  PoolAdvancedConfig,
  ClaudeCodeAdvancedConfig,
  ProviderWithEndpointsSummary,
} from '@/api/endpoints/types/provider'

const props = defineProps<{
  modelValue: boolean
  providerId: string
  providerType?: string
  currentConfig: PoolAdvancedConfig | null
  currentClaudeConfig?: ClaudeCodeAdvancedConfig | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  saved: [provider: ProviderWithEndpointsSummary]
}>()

const { success, error: showError } = useToast()
const loading = ref(false)

const isClaudeCode = computed(() => {
  return (props.providerType || '').trim().toLowerCase() === 'claude_code'
})

const form = ref({
  sticky_session_ttl_seconds: null as number | null | undefined,
  health_policy_enabled: true,
  rate_limit_cooldown_seconds: null as number | null | undefined,
  overload_cooldown_seconds: null as number | null | undefined,
  cost_window_seconds: null as number | null | undefined,
  cost_limit_per_key_tokens: null as number | null | undefined,
  cost_soft_threshold_percent: null as number | null | undefined,
  batch_concurrency: null as number | null | undefined,
  probing_enabled: false,
  probing_interval_minutes: null as number | null | undefined,
  auto_remove_banned_keys: false,
})

interface ClaudeFormState {
  session_control_enabled: boolean
  max_sessions: number | undefined
  session_idle_timeout_minutes: number
  session_id_masking_enabled: boolean
  cache_ttl_override_enabled: boolean
  cache_ttl_override_target: string
  cli_only_enabled: boolean
}

const claudeForm = ref<ClaudeFormState>({
  session_control_enabled: true,
  max_sessions: undefined,
  session_idle_timeout_minutes: 5,
  session_id_masking_enabled: true,
  cache_ttl_override_enabled: false,
  cache_ttl_override_target: 'ephemeral',
  cli_only_enabled: false,
})

function parseNum(v: string | number): number | undefined {
  if (v === '' || v === null || v === undefined) return undefined
  const n = Number(v)
  return Number.isNaN(n) ? undefined : n
}

watch(() => props.modelValue, (open) => {
  if (!open) return

  const cfg = props.currentConfig
  form.value = {
    sticky_session_ttl_seconds: cfg?.sticky_session_ttl_seconds ?? null,
    health_policy_enabled: cfg?.health_policy_enabled !== false,
    rate_limit_cooldown_seconds: cfg?.rate_limit_cooldown_seconds ?? null,
    overload_cooldown_seconds: cfg?.overload_cooldown_seconds ?? null,
    cost_window_seconds: cfg?.cost_window_seconds ?? null,
    cost_limit_per_key_tokens: cfg?.cost_limit_per_key_tokens ?? null,
    cost_soft_threshold_percent: cfg?.cost_soft_threshold_percent ?? null,
    batch_concurrency: cfg?.batch_concurrency ?? null,
    probing_enabled: cfg?.probing_enabled ?? false,
    probing_interval_minutes: cfg?.probing_interval_minutes ?? null,
    auto_remove_banned_keys: cfg?.auto_remove_banned_keys ?? false,
  }

  const cc = props.currentClaudeConfig
  claudeForm.value = {
    session_control_enabled: cc?.max_sessions !== null,
    max_sessions: cc?.max_sessions ?? undefined,
    session_idle_timeout_minutes: cc?.session_idle_timeout_minutes ?? 5,
    session_id_masking_enabled: cc?.session_id_masking_enabled !== false,
    cache_ttl_override_enabled: cc?.cache_ttl_override_enabled ?? false,
    cache_ttl_override_target: cc?.cache_ttl_override_target ?? 'ephemeral',
    cli_only_enabled: cc?.cli_only_enabled ?? false,
  }
})

async function handleSave() {
  loading.value = true
  try {
    // 合并已有配置（保留 scheduling_presets 等不在此对话框编辑的字段）
    const poolAdvanced: Record<string, unknown> = {
      ...(props.currentConfig ?? {}),
      sticky_session_ttl_seconds: form.value.sticky_session_ttl_seconds ?? undefined,
      cost_window_seconds: form.value.cost_window_seconds ?? undefined,
      cost_limit_per_key_tokens: form.value.cost_limit_per_key_tokens ?? undefined,
      cost_soft_threshold_percent: form.value.cost_soft_threshold_percent ?? undefined,
      rate_limit_cooldown_seconds: form.value.rate_limit_cooldown_seconds ?? undefined,
      overload_cooldown_seconds: form.value.overload_cooldown_seconds ?? undefined,
      health_policy_enabled: form.value.health_policy_enabled,
      batch_concurrency: form.value.batch_concurrency ?? undefined,
      probing_enabled: form.value.probing_enabled,
      probing_interval_minutes: form.value.probing_enabled
        ? (form.value.probing_interval_minutes ?? undefined)
        : undefined,
      auto_remove_banned_keys: form.value.auto_remove_banned_keys,
    }

    const payload: Parameters<typeof updateProvider>[1] = {
      pool_advanced: poolAdvanced as PoolAdvancedConfig,
    }
    if (isClaudeCode.value) {
      const cf = claudeForm.value
      payload.claude_code_advanced = {
        max_sessions: cf.session_control_enabled ? (cf.max_sessions ?? null) : null,
        session_idle_timeout_minutes: cf.session_control_enabled ? cf.session_idle_timeout_minutes : null,
        session_id_masking_enabled: cf.session_id_masking_enabled,
        cache_ttl_override_enabled: cf.cache_ttl_override_enabled,
        cache_ttl_override_target: cf.cache_ttl_override_enabled ? cf.cache_ttl_override_target : undefined,
        cli_only_enabled: cf.cli_only_enabled,
      }
    }
    const updatedProvider = await updateProvider(props.providerId, payload)
    success('高级设置已保存')
    emit('saved', updatedProvider)
    emit('update:modelValue', false)
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    loading.value = false
  }
}
</script>
