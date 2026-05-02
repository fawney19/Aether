<script setup lang="ts">
import { Check } from 'lucide-vue-next'

export interface TableFilterMenuOption {
  value: string
  label: string
  disabled?: boolean
}

defineProps<{
  modelValue: string
  options: TableFilterMenuOption[]
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
  select: [value: string]
}>()

function selectOption(value: string) {
  emit('update:modelValue', value)
  emit('select', value)
}
</script>

<template>
  <div>
    <button
      v-for="option in options"
      :key="option.value"
      type="button"
      class="relative flex w-full cursor-pointer select-none items-center rounded-lg py-1.5 pl-8 pr-2 text-left text-sm text-foreground outline-none transition-colors hover:bg-accent focus:bg-accent disabled:pointer-events-none disabled:opacity-50"
      :disabled="option.disabled"
      @click="selectOption(option.value)"
    >
      <Check
        class="absolute left-2 h-4 w-4"
        :class="modelValue === option.value ? 'opacity-100' : 'opacity-0'"
      />
      <span>{{ option.label }}</span>
    </button>
  </div>
</template>
