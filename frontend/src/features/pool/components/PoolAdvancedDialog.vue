<template>
  <Dialog
    :model-value="modelValue"
    title="高级设置"
    description="冷却、健康、成本控制与其他高级参数"
    size="3xl"
    @update:model-value="emit('update:modelValue', $event)"
  >
    <div class="max-h-[calc(100dvh-13rem)] space-y-5 overflow-y-auto overscroll-contain pr-1 sm:max-h-[min(72vh,42rem)] sm:space-y-6 sm:pr-2">
      <section class="space-y-4 rounded-2xl border border-border/60 bg-card/70 p-4 sm:p-5">
        <div class="space-y-1">
          <div class="flex flex-wrap items-center gap-2">
            <h3 class="text-sm font-semibold">
              冷却与健康
            </h3>
            <span class="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
              核心策略
            </span>
          </div>
          <p class="text-xs leading-5 text-muted-foreground">
            控制自动冷却、主动探测、异常清理和全局调度优先级。
          </p>
        </div>

        <div class="grid gap-3 lg:grid-cols-3">
          <div
            v-for="item in healthToggleCards"
            :key="item.key"
            class="flex flex-col gap-3 rounded-xl border border-border/60 bg-muted/30 p-4 sm:flex-row sm:items-start sm:justify-between lg:items-center"
          >
            <div class="min-w-0 flex-1 space-y-1">
              <div class="flex items-center gap-1.5">
                <span class="text-sm font-medium">{{ item.label }}</span>
                <TooltipProvider
                  :delay-duration="100"
                >
                  <Tooltip>
                    <TooltipTrigger as-child>
                      <button
                        type="button"
                        :title="item.description"
                        :aria-label="`${item.label} 说明`"
                        class="hidden lg:inline-flex items-center justify-center rounded-sm p-0.5 text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
                      >
                        <CircleHelp class="h-3.5 w-3.5" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent
                      side="top"
                      :side-offset="8"
                      class="max-w-xs px-3 py-2 text-xs leading-5"
                    >
                      {{ item.description }}
                    </TooltipContent>
                  </Tooltip>
                </TooltipProvider>
              </div>
              <p class="text-xs leading-5 text-muted-foreground lg:hidden">
                {{ item.description }}
              </p>
            </div>
            <Switch
              :model-value="getHealthToggleValue(item.key)"
              class="shrink-0"
              @update:model-value="(v: boolean) => updateHealthToggleValue(item.key, v)"
            />
          </div>
        </div>

        <div
          v-if="form.probing_enabled"
          class="rounded-xl border border-dashed border-primary/25 bg-primary/5 p-4"
        >
          <div class="grid gap-3 sm:grid-cols-2">
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
        </div>

        <div
          class="grid gap-3 sm:grid-cols-2"
          :class="cooldownFieldLayout.desktopColumnsClass"
        >
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
          <div class="space-y-1.5">
            <Label>
              全局优先级
            </Label>
            <Input
              :model-value="form.global_priority ?? ''"
              type="number"
              min="0"
              max="999999"
              placeholder="留空回退 provider_priority"
              @update:model-value="(v) => form.global_priority = parseNum(v)"
            />
          </div>
        </div>
      </section>

      <div :class="secondarySectionLayout.wrapperClass">
        <section class="space-y-4 rounded-2xl border border-border/60 bg-card/70 p-4 sm:p-5">
          <div class="space-y-1">
            <div class="flex flex-wrap items-center gap-2">
              <h3 class="text-sm font-semibold">
                成本控制
              </h3>
              <span class="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
                额度保护
              </span>
            </div>
            <p class="text-xs leading-5 text-muted-foreground">
              控制窗口期、Key 限额与软阈值，防止个别账号短时间内过度消耗。
            </p>
          </div>

          <div
            class="grid gap-3 sm:grid-cols-2"
            :class="costFieldLayout.desktopColumnsClass"
          >
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
        </section>

        <section class="space-y-4 rounded-2xl border border-border/60 bg-card/70 p-4 sm:p-5">
          <div class="space-y-1">
            <div class="flex flex-wrap items-center gap-2">
              <h3 class="text-sm font-semibold">
                批量操作
              </h3>
              <span class="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
                任务效率
              </span>
            </div>
            <p class="text-xs leading-5 text-muted-foreground">
              控制刷新 OAuth、主动探测和批量额度处理时的并行请求数。
            </p>
          </div>

          <div class="rounded-xl bg-muted/30 p-4">
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
              <p class="text-[11px] leading-5 text-muted-foreground">
                为空时沿用默认值；数值越大，批量操作越快，但会增加瞬时请求压力。
              </p>
            </div>
          </div>
        </section>
      </div>

      <section
        v-if="isClaudeCode"
        class="space-y-4 rounded-2xl border border-border/60 bg-card/70 p-4 sm:p-5"
      >
        <div class="space-y-1">
          <div class="flex flex-wrap items-center gap-2">
            <h3 class="text-sm font-semibold">
              Claude Code
            </h3>
            <span class="rounded-full bg-muted px-2 py-0.5 text-[11px] text-muted-foreground">
              请求约束
            </span>
          </div>
          <p class="text-xs leading-5 text-muted-foreground">
            管理 CLI 请求限制、会话控制和 metadata / cache 相关的兼容行为。
          </p>
        </div>

        <div class="grid gap-3 lg:grid-cols-2">
          <div class="flex flex-col gap-3 rounded-xl border border-border/60 bg-muted/30 p-4 sm:flex-row sm:items-start sm:justify-between">
            <div class="space-y-1">
              <span class="text-sm font-medium">Session ID 伪装</span>
              <p class="text-xs leading-5 text-muted-foreground">
                固定 metadata.user_id 中 session 片段。
              </p>
            </div>
            <Switch
              :model-value="claudeForm.session_id_masking_enabled"
              class="shrink-0"
              @update:model-value="(v: boolean) => claudeForm.session_id_masking_enabled = v"
            />
          </div>

          <div class="flex flex-col gap-3 rounded-xl border border-border/60 bg-muted/30 p-4 sm:flex-row sm:items-start sm:justify-between">
            <div class="space-y-1">
              <span class="text-sm font-medium">仅限 CLI 客户端</span>
              <p class="text-xs leading-5 text-muted-foreground">
                仅允许 Claude Code CLI 格式请求。
              </p>
            </div>
            <Switch
              :model-value="claudeForm.cli_only_enabled"
              class="shrink-0"
              @update:model-value="(v: boolean) => claudeForm.cli_only_enabled = v"
            />
          </div>

          <div class="flex flex-col gap-3 rounded-xl border border-border/60 bg-muted/30 p-4 sm:flex-row sm:items-start sm:justify-between">
            <div class="space-y-1">
              <span class="text-sm font-medium">Cache TTL 统一</span>
              <p class="text-xs leading-5 text-muted-foreground">
                强制所有 cache_control 使用同一种 TTL 类型。
              </p>
            </div>
            <Switch
              :model-value="claudeForm.cache_ttl_override_enabled"
              class="shrink-0"
              @update:model-value="(v: boolean) => claudeForm.cache_ttl_override_enabled = v"
            />
          </div>

          <div class="flex flex-col gap-3 rounded-xl border border-border/60 bg-muted/30 p-4 sm:flex-row sm:items-start sm:justify-between">
            <div class="space-y-1">
              <span class="text-sm font-medium">会话数量控制</span>
              <p class="text-xs leading-5 text-muted-foreground">
                限制单 Key 同时活跃会话数，降低长期占用风险。
              </p>
            </div>
            <Switch
              :model-value="claudeForm.session_control_enabled"
              class="shrink-0"
              @update:model-value="(v: boolean) => claudeForm.session_control_enabled = v"
            />
          </div>
        </div>

        <div
          v-if="claudeForm.cache_ttl_override_enabled"
          class="rounded-xl border border-dashed border-primary/25 bg-primary/5 p-4"
        >
          <div class="space-y-1.5">
            <Label>TTL 类型</Label>
            <div class="flex w-fit gap-0.5 rounded-md bg-muted/40 p-0.5">
              <button
                v-for="opt in ['ephemeral']"
                :key="opt"
                type="button"
                class="rounded px-2.5 py-1 text-xs font-medium transition-all"
                :class="[
                  claudeForm.cache_ttl_override_target === opt
                    ? 'bg-primary text-primary-foreground shadow-sm'
                    : 'text-muted-foreground hover:bg-background/50 hover:text-foreground'
                ]"
                @click="claudeForm.cache_ttl_override_target = opt"
              >
                {{ opt }}
              </button>
            </div>
          </div>
        </div>

        <div
          v-if="claudeForm.session_control_enabled"
          class="rounded-xl border border-dashed border-primary/25 bg-primary/5 p-4"
        >
          <div class="grid gap-3 sm:grid-cols-2">
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
      </section>
    </div>

    <template #footer>
      <Button
        variant="outline"
        class="min-w-[96px] flex-1 sm:flex-none"
        :disabled="loading"
        @click="emit('update:modelValue', false)"
      >
        取消
      </Button>
      <Button
        class="min-w-[96px] flex-1 sm:flex-none"
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
import { CircleHelp } from 'lucide-vue-next'
import { Dialog, Button, Input, Label, Switch, Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { updateProvider } from '@/api/endpoints'
import {
  buildPoolCooldownFieldLayout,
  buildPoolCostFieldLayout,
  buildPoolHealthToggleCards,
  buildPoolSecondarySectionLayout,
  type PoolHealthToggleKey,
} from '@/features/pool/utils/poolAdvancedDialog'
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

const healthToggleCards = buildPoolHealthToggleCards()
const cooldownFieldLayout = buildPoolCooldownFieldLayout()
const costFieldLayout = buildPoolCostFieldLayout()
const secondarySectionLayout = buildPoolSecondarySectionLayout()

const form = ref({
  global_priority: null as number | null | undefined,
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

function getHealthToggleValue(key: PoolHealthToggleKey): boolean {
  switch (key) {
    case 'health_policy_enabled':
      return form.value.health_policy_enabled
    case 'probing_enabled':
      return form.value.probing_enabled
    case 'auto_remove_banned_keys':
      return form.value.auto_remove_banned_keys
  }
}

function updateHealthToggleValue(key: PoolHealthToggleKey, value: boolean): void {
  switch (key) {
    case 'health_policy_enabled':
      form.value.health_policy_enabled = value
      return
    case 'probing_enabled':
      form.value.probing_enabled = value
      return
    case 'auto_remove_banned_keys':
      form.value.auto_remove_banned_keys = value
  }
}

watch(() => props.modelValue, (open) => {
  if (!open) return

  const cfg = props.currentConfig
  form.value = {
    global_priority: cfg?.global_priority ?? null,
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
      global_priority: form.value.global_priority ?? undefined,
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
