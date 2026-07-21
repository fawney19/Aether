<template>
  <div class="min-w-0 space-y-1.5">
    <label class="flex items-center gap-2 text-xs font-medium">
      <Checkbox
        :model-value="modelValue"
        @update:model-value="value => emit('update:modelValue', value)"
      />
      <span>{{ label }}</span>
    </label>
    <div :class="!modelValue ? 'pointer-events-none opacity-45' : ''">
      <slot />
    </div>
  </div>
</template>

<script setup lang="ts">
import { Checkbox } from '@/components/ui'

defineProps<{
  modelValue: boolean
  label: string
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()
</script>
