<template>
  <div class="space-y-3 rounded-lg border p-3">
    <div class="grid gap-3 md:grid-cols-2">
      <div class="space-y-2 rounded-md border bg-muted/10 p-3">
        <label class="flex items-center gap-2 text-sm font-medium">
          <Checkbox
            :checked="state.maxRetries.include"
            :disabled="disabled"
            @update:checked="patchMaxRetries({ include: $event === true })"
          />
          <span>最大重试次数</span>
        </label>
        <Input
          :model-value="state.maxRetries.value ?? ''"
          type="number"
          min="0"
          max="999"
          step="1"
          placeholder="2"
          :disabled="disabled || !state.maxRetries.include"
          @update:model-value="patchMaxRetries({ value: parseNumberInput($event, { min: 0, max: 999 }) })"
        />
      </div>

      <div class="space-y-2 rounded-md border bg-muted/10 p-3">
        <label class="flex items-center gap-2 text-sm font-medium">
          <Checkbox
            :checked="state.streamFirstByteTimeout.include"
            :disabled="disabled"
            @update:checked="patchStreamFirstByteTimeout({ include: $event === true })"
          />
          <span>流式首字节超时 <span class="text-xs text-muted-foreground">(秒)</span></span>
        </label>
        <Input
          :model-value="state.streamFirstByteTimeout.value ?? ''"
          type="number"
          min="1"
          max="300"
          step="1"
          placeholder="使用全局"
          :disabled="disabled || !state.streamFirstByteTimeout.include"
          @update:model-value="patchStreamFirstByteTimeout({ value: parseNumberInput($event, { min: 1, max: 300 }) })"
        />
      </div>

      <div class="space-y-2 rounded-md border bg-muted/10 p-3">
        <label class="flex items-center gap-2 text-sm font-medium">
          <Checkbox
            :checked="state.requestTimeout.include"
            :disabled="disabled"
            @update:checked="patchRequestTimeout({ include: $event === true })"
          />
          <span>非流式请求超时 <span class="text-xs text-muted-foreground">(秒)</span></span>
        </label>
        <Input
          :model-value="state.requestTimeout.value ?? ''"
          type="number"
          min="1"
          max="600"
          step="1"
          placeholder="使用全局"
          :disabled="disabled || !state.requestTimeout.include"
          @update:model-value="patchRequestTimeout({ value: parseNumberInput($event, { min: 1, max: 600 }) })"
        />
      </div>

      <div class="space-y-2 rounded-md border bg-muted/10 p-3 md:col-span-2">
        <label class="flex items-center gap-2 text-sm font-medium">
          <Checkbox
            :checked="state.keepPriorityOnConversion.include"
            :disabled="disabled"
            @update:checked="patchKeepPriorityOnConversion({ include: $event === true })"
          />
          <span>格式转换保持优先级</span>
        </label>
        <div
          class="flex items-center justify-between"
          :class="state.keepPriorityOnConversion.include ? '' : 'pointer-events-none opacity-45'"
        >
          <span class="text-xs text-muted-foreground">跨格式请求时保持原优先级排名</span>
          <Switch
            :model-value="state.keepPriorityOnConversion.value"
            @update:model-value="patchKeepPriorityOnConversion({ value: $event })"
          />
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import {
  Checkbox,
  Input,
  Switch,
} from '@/components/ui'
import { parseNumberInput } from '@/utils/form'
import type { ProviderBasicBatchState } from '@/features/providers/utils/batchEdit'

const props = defineProps<{
  state: ProviderBasicBatchState
  disabled: boolean
}>()

const emit = defineEmits<{
  'update:state': [state: ProviderBasicBatchState]
}>()

function patchState(patch: Partial<ProviderBasicBatchState>): void {
  emit('update:state', { ...props.state, ...patch })
}

function patchMaxRetries(patch: Partial<ProviderBasicBatchState['maxRetries']>): void {
  patchState({ maxRetries: { ...props.state.maxRetries, ...patch } })
}

function patchStreamFirstByteTimeout(patch: Partial<ProviderBasicBatchState['streamFirstByteTimeout']>): void {
  patchState({ streamFirstByteTimeout: { ...props.state.streamFirstByteTimeout, ...patch } })
}

function patchRequestTimeout(patch: Partial<ProviderBasicBatchState['requestTimeout']>): void {
  patchState({ requestTimeout: { ...props.state.requestTimeout, ...patch } })
}

function patchKeepPriorityOnConversion(patch: Partial<ProviderBasicBatchState['keepPriorityOnConversion']>): void {
  patchState({ keepPriorityOnConversion: { ...props.state.keepPriorityOnConversion, ...patch } })
}
</script>
