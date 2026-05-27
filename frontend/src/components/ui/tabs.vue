<template>
  <div class="tabs-root">
    <slot />
  </div>
</template>

<script setup lang="ts">
import { provide, ref, useId, watch } from 'vue'

interface Props {
  defaultValue?: string
  modelValue?: string
  id?: string
}

const props = defineProps<Props>()
const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

const activeTab = ref(props.modelValue || props.defaultValue || '')
const generatedId = useId()
const tabsBaseId = props.id || `tabs-${generatedId.replace(/[^A-Za-z0-9_-]/g, '-')}`

watch(() => props.modelValue, (newValue) => {
  if (newValue !== undefined) {
    activeTab.value = newValue
  }
})

const setActiveTab = (value: string) => {
  activeTab.value = value
  emit('update:modelValue', value)
}

provide('activeTab', activeTab)
provide('setActiveTab', setActiveTab)
provide('tabsBaseId', tabsBaseId)
</script>
