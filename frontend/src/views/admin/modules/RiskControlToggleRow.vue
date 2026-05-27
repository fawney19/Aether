<template>
  <div class="flex items-center justify-between rounded-lg border border-border bg-muted/25 px-4 py-3">
    <span class="text-sm font-medium text-foreground">
      {{ label }}
    </span>
    <Switch
      :model-value="value"
      @update:model-value="emit('update:value', $event)"
    />
  </div>
</template>

<script setup lang="ts">
import { Switch } from '@/components/ui'

defineProps<{
  label: string
  value: boolean
}>()

const emit = defineEmits<{
  'update:value': [value: boolean]
}>()
</script>
