<template>
  <div class="space-y-2.5">
    <div class="flex items-center justify-between gap-3">
      <Label class="text-sm font-medium">{{ legacyT('选择批量动作') }}</Label>
      <span class="text-[11px] text-muted-foreground">{{ legacyT('只会提交当前动作对应的字段') }}</span>
    </div>
    <div class="grid gap-2 md:grid-cols-4">
      <button
        v-for="action in actions"
        :key="action.value"
        type="button"
        :class="actionCardClass(action.value)"
        @click="$emit('update:modelValue', action.value)"
      >
        <span class="flex items-center gap-2">
          <span :class="actionIconClass(action.value)">
            <component
              :is="action.icon"
              class="h-4 w-4"
            />
          </span>
          <span class="font-medium text-foreground">{{ legacyT(action.label) }}</span>
        </span>
        <span class="mt-1 block text-[11px] leading-relaxed text-muted-foreground">
          {{ legacyT(action.description) }}
        </span>
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { Label } from '@/components/ui'
import { cn } from '@/lib/utils'
import { useI18n } from '@/i18n'
import type { UserBatchAction } from '@/api/users'
import type { UserBatchActionOption } from './user-management-config'

const props = defineProps<{
  modelValue: UserBatchAction
  actions: UserBatchActionOption[]
}>()

defineEmits<{
  'update:modelValue': [value: UserBatchAction]
}>()

const { legacyT } = useI18n()

function actionCardClass(action: UserBatchAction): string {
  return cn(
    'rounded-xl border p-3 text-left transition-all hover:-translate-y-0.5 hover:border-primary/35 hover:bg-primary/5 hover:shadow-sm focus:outline-none focus:ring-2 focus:ring-primary/30',
    props.modelValue === action
      ? 'border-primary/60 bg-primary/10 shadow-sm ring-1 ring-primary/20'
      : 'border-border/70 bg-background',
  )
}

function actionIconClass(action: UserBatchAction): string {
  return cn(
    'flex h-7 w-7 items-center justify-center rounded-lg transition-colors',
    props.modelValue === action
      ? 'bg-primary text-primary-foreground'
      : 'bg-muted text-muted-foreground',
  )
}
</script>
