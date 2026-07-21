<template>
  <input
    ref="inputRef"
    type="checkbox"
    :class="checkboxClass"
    v-bind="$attrs"
    :checked="isChecked"
    :aria-checked="indeterminate ? 'mixed' : isChecked"
    @change="handleChange"
  >
</template>

<script setup lang="ts">
import { computed, ref, watchEffect } from 'vue'
import { cn } from '@/lib/utils'

interface Props {
  modelValue?: boolean
  checked?: boolean
  indeterminate?: boolean
  class?: string
}

const props = defineProps<Props>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'update:checked': [value: boolean]
}>()

const checkboxClass = computed(() =>
  cn(
    'h-4 w-4 rounded border-border/60 bg-card/80 text-primary shadow-sm focus:ring-2 focus:ring-primary/40 focus:ring-offset-1 accent-primary',
    props.class
  )
)

const isChecked = computed<boolean>(() => {
  if (typeof props.checked === 'boolean') {
    return props.checked
  }
  return props.modelValue ?? false
})

const inputRef = ref<HTMLInputElement | null>(null)

watchEffect(() => {
  if (inputRef.value) {
    inputRef.value.indeterminate = props.indeterminate ?? false
  }
}, { flush: 'post' })

function handleChange(event: Event) {
  const target = event.target as HTMLInputElement
  const value = target.checked
  emit('update:modelValue', value)
  emit('update:checked', value)
}
</script>
