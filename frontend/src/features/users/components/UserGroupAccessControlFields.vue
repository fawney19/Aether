<template>
  <div class="space-y-4 border-t border-border/60 pt-5">
    <div class="flex flex-wrap items-baseline justify-between gap-x-2 gap-y-1 border-b border-border/60 pb-2">
      <span class="text-sm font-medium">{{ legacyT('组权限') }}</span>
      <span class="flex items-center gap-1 text-[11px] text-muted-foreground">
        {{ legacyT('组权限叠加，Key 可再收窄') }}
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger as-child>
              <button
                type="button"
                class="inline-flex h-4 w-4 items-center justify-center rounded-full border border-border/70 bg-muted/40 text-muted-foreground outline-none transition-colors hover:border-primary/50 hover:text-primary focus-visible:border-primary/60 focus-visible:text-primary"
                :title="helpText"
                :aria-label="legacyT('查看组权限合并规则')"
              >
                <Info class="h-3 w-3" />
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
        <div class="flex w-full items-center sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.allowed_providers_mode === 'unrestricted'"
            @update:model-value="setProvidersUnrestricted"
          />
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

    <div class="space-y-2">
      <Label class="text-sm font-medium">{{ legacyT('允许的端点') }}</Label>
      <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
        <div class="flex w-full items-center sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.allowed_api_formats_mode === 'unrestricted'"
            @update:model-value="setApiFormatsUnrestricted"
          />
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
        <div class="flex w-full items-center sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.allowed_models_mode === 'unrestricted'"
            @update:model-value="setModelsUnrestricted"
          />
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
        <div class="flex w-full items-center sm:w-auto sm:shrink-0">
          <Switch
            :model-value="form.rate_limit_mode === 'system'"
            @update:model-value="setSystemRateLimit"
          />
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
import type { UserGroupFormState, UserSelectOption } from './user-management-types'

const props = defineProps<{
  form: UserGroupFormState
  providerOptions: UserSelectOption[]
  apiFormatOptions: UserSelectOption[]
  modelOptions: UserSelectOption[]
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
  updateForm({ allowed_providers_mode: value ? 'unrestricted' : 'specific' })
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
