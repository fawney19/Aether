<template>
  <div class="space-y-4 border-t border-border/60 pt-5">
    <div class="flex items-center justify-between gap-2 border-b border-border/60 pb-2">
      <span class="text-sm font-medium">{{ legacyT('组权限') }}</span>
      <span class="flex items-center gap-1 text-[11px] text-muted-foreground">
        {{ legacyT('组权限叠加，Key 可再收窄') }}
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger as-child>
              <button
                type="button"
                class="inline-flex h-10 w-10 items-center justify-center rounded-full text-muted-foreground outline-none transition-colors hover:bg-muted/60 hover:text-primary focus-visible:ring-2 focus-visible:ring-ring"
                :title="helpText"
                :aria-label="legacyT('查看组权限合并规则')"
              >
                <Info class="h-4 w-4" />
              </button>
            </TooltipTrigger>
            <TooltipContent class="max-w-72 text-xs leading-5">
              {{ helpText }}
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
      </span>
    </div>

    <div class="space-y-2">
      <Label class="text-sm font-medium">{{ legacyT('允许的提供商') }}</Label>
      <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
        <div class="flex min-h-10 w-full items-center gap-2 sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.allowed_providers_mode === 'unrestricted'"
            @update:model-value="setProvidersUnrestricted"
          />
          <span class="text-xs text-muted-foreground sm:sr-only">
            {{ legacyT(form.allowed_providers_mode === 'unrestricted' ? '不限制' : '选择提供商') }}
          </span>
        </div>
        <div class="min-w-0 flex-1">
          <MultiSelect
            :model-value="form.allowed_providers"
            :options="providerOptions"
            :search-threshold="0"
            :disabled="form.allowed_providers_mode === 'unrestricted'"
            :placeholder="legacyT(form.allowed_providers_mode === 'unrestricted' ? '不限制所有选项' : '选择提供商')"
            :empty-text="legacyT('暂无选项')"
            @update:model-value="(value) => updateForm({ allowed_providers: value })"
          />
        </div>
      </div>
    </div>

    <!-- 提供商下的具体 Key -->
    <div
      v-if="form.allowed_providers_mode === 'specific' && form.allowed_providers.length > 0"
      class="space-y-3"
    >
      <div class="flex items-center justify-between gap-2">
        <Label class="text-sm font-medium">{{ legacyT('允许的具体 Key（可选）') }}</Label>
        <span class="text-[11px] text-muted-foreground">
          {{ legacyT('不勾选时全部可用') }}
        </span>
      </div>
      <div
        v-for="providerId in form.allowed_providers"
        :key="providerId"
        class="rounded-lg border border-border/70 bg-background/60"
      >
        <div class="flex items-center justify-between gap-2 border-b border-border/50 px-3 py-2">
          <span class="text-xs font-medium">{{ providerDisplayName(providerId) }}</span>
          <span class="text-[11px] text-muted-foreground">
            {{ providerKeyScopeSelectedCount(providerId) > 0 ? legacyT(`已选 ${providerKeyScopeSelectedCount(providerId)} 个 Key`) : legacyT('全部 Key 可用') }}
          </span>
        </div>
        <div v-if="providerKeysLoading[providerId]" class="px-3 py-2 text-xs text-muted-foreground">
          {{ legacyT('加载 Key 中...') }}
        </div>
        <div v-else-if="providerKeysByProvider[providerId]?.length" class="grid gap-1 p-2">
          <label
            v-for="key in providerKeysByProvider[providerId]"
            :key="key.id"
            class="flex cursor-pointer items-center gap-2 rounded px-2 py-1 text-sm hover:bg-muted/50"
          >
            <input
              :checked="selectedProviderKeyIds(providerId).includes(key.id)"
              type="checkbox"
              class="h-3.5 w-3.5 rounded border-gray-300 cursor-pointer"
              @change="toggleProviderKey(providerId, key.id)"
            >
            <span class="min-w-0 flex-1 truncate">{{ key.name || key.id }}</span>
            <span class="text-[11px] text-muted-foreground">{{ key.api_key_masked }}</span>
          </label>
        </div>
        <div v-else class="px-3 py-2 text-xs text-muted-foreground">
          {{ legacyT('该提供商暂无可用 Key') }}
        </div>
      </div>
    </div>

    <div class="space-y-2">
      <Label class="text-sm font-medium">{{ legacyT('允许的端点') }}</Label>
      <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
        <div class="flex min-h-10 w-full items-center gap-2 sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.allowed_api_formats_mode === 'unrestricted'"
            @update:model-value="setApiFormatsUnrestricted"
          />
          <span class="text-xs text-muted-foreground sm:sr-only">
            {{ legacyT(form.allowed_api_formats_mode === 'unrestricted' ? '不限制' : '选择端点') }}
          </span>
        </div>
        <div class="min-w-0 flex-1">
          <MultiSelect
            :model-value="form.allowed_api_formats"
            :options="apiFormatOptions"
            :search-threshold="0"
            :disabled="form.allowed_api_formats_mode === 'unrestricted'"
            :placeholder="legacyT(form.allowed_api_formats_mode === 'unrestricted' ? '不限制所有选项' : '选择端点')"
            :empty-text="legacyT('暂无选项')"
            @update:model-value="(value) => updateForm({ allowed_api_formats: value })"
          />
        </div>
      </div>
    </div>

    <div class="space-y-2">
      <Label class="text-sm font-medium">{{ legacyT('允许的模型') }}</Label>
      <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
        <div class="flex min-h-10 w-full items-center gap-2 sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.allowed_models_mode === 'unrestricted'"
            @update:model-value="setModelsUnrestricted"
          />
          <span class="text-xs text-muted-foreground sm:sr-only">
            {{ legacyT(form.allowed_models_mode === 'unrestricted' ? '不限制' : '选择模型') }}
          </span>
        </div>
        <div class="min-w-0 flex-1">
          <MultiSelect
            :model-value="form.allowed_models"
            :options="modelOptions"
            :search-threshold="0"
            :disabled="form.allowed_models_mode === 'unrestricted'"
            :placeholder="legacyT(form.allowed_models_mode === 'unrestricted' ? '不限制所有选项' : '选择模型')"
            :empty-text="legacyT('暂无选项')"
            @update:model-value="(value) => updateForm({ allowed_models: value })"
          />
        </div>
      </div>
    </div>

    <div class="space-y-2">
      <Label class="text-sm font-medium">{{ legacyT('速率限制 (请求/分钟)') }}</Label>
      <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
        <div class="flex min-h-10 w-full items-center gap-2 sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.rate_limit_mode === 'system'"
            @update:model-value="setSystemRateLimit"
          />
          <span class="text-xs text-muted-foreground sm:sr-only">
            {{ legacyT(form.rate_limit_mode === 'system' ? '系统默认' : '自定义') }}
          </span>
        </div>
        <div class="min-w-0 flex-1">
          <Input
            :model-value="form.rate_limit ?? ''"
            type="number"
            min="0"
            max="10000"
            class="h-10"
            :disabled="form.rate_limit_mode === 'system'"
            :placeholder="legacyT(form.rate_limit_mode === 'system' ? '使用系统默认' : '0 = 不限速')"
            @update:model-value="updateRateLimit"
          />
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { Info } from 'lucide-vue-next'
import {
  Input,
  Label,
  Switch,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui'
import { MultiSelect } from '@/components/common'
import { parseNumberInput } from '@/utils/form'
import { useI18n } from '@/i18n'
import {
  restrictProviderKeyScopeToProviders,
} from '@/features/api-keys/utils/providerKeyScope'
import type { UserGroupFormState, UserSelectOption } from './user-management-types'
import type { EndpointAPIKey } from '@/api/endpoints/keys'

const props = defineProps<{
  form: UserGroupFormState
  providerOptions: UserSelectOption[]
  apiFormatOptions: UserSelectOption[]
  modelOptions: UserSelectOption[]
  providerKeysByProvider: Record<string, EndpointAPIKey[]>
  providerKeysLoading: Record<string, boolean>
  providerDisplayName: (providerId: string) => string
  helpText: string
}>()

const emit = defineEmits<{
  'update:form': [value: UserGroupFormState]
}>()

const { legacyT } = useI18n()

function updateForm(patch: Partial<UserGroupFormState>): void {
  emit('update:form', { ...props.form, ...patch })
}

function setProvidersUnrestricted(value: boolean): void {
  const mode = value ? 'unrestricted' : 'specific'
  const patch: Partial<UserGroupFormState> = { allowed_providers_mode: mode }
  if (value) {
    patch.provider_key_scope = {}
  }
  updateForm(patch)
}

function providerKeyScopeSelectedCount(providerId: string): number {
  return props.form.provider_key_scope[providerId]?.length ?? 0
}

function selectedProviderKeyIds(providerId: string): string[] {
  return props.form.provider_key_scope[providerId] ?? []
}

function toggleProviderKey(providerId: string, keyId: string): void {
  const current = new Set(props.form.provider_key_scope[providerId] ?? [])
  if (current.has(keyId)) {
    current.delete(keyId)
  } else {
    current.add(keyId)
  }
  const nextScope = {
    ...props.form.provider_key_scope,
    [providerId]: [...current].sort(),
  }
  updateForm({
    provider_key_scope: restrictProviderKeyScopeToProviders(
      nextScope,
      props.form.allowed_providers,
    ),
  })
}

function setApiFormatsUnrestricted(value: boolean): void {
  updateForm({ allowed_api_formats_mode: value ? 'unrestricted' : 'specific' })
}

function setModelsUnrestricted(value: boolean): void {
  updateForm({ allowed_models_mode: value ? 'unrestricted' : 'specific' })
}

function setSystemRateLimit(value: boolean): void {
  updateForm({ rate_limit_mode: value ? 'system' : 'custom' })
}

function updateRateLimit(value: string | number): void {
  updateForm({ rate_limit: parseNumberInput(value, { min: 0, max: 10000 }) })
}
</script>
