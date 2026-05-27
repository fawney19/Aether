<template>
  <button
    :id="tabId"
    :class="triggerClass"
    :data-state="isActive ? 'active' : 'inactive'"
    :data-value="props.value"
    role="tab"
    :aria-selected="isActive"
    :aria-controls="panelId"
    :tabindex="isActive ? 0 : -1"
    type="button"
    @click="handleClick"
    @keydown="handleKeydown"
  >
    <slot />
  </button>
</template>

<script setup lang="ts">
import { computed, inject, type Ref } from 'vue'
import { cn } from '@/lib/utils'

interface Props {
  value: string
  class?: string
}

const props = defineProps<Props>()

const activeTab = inject<Ref<string>>('activeTab')
const setActiveTab = inject<(value: string) => void>('setActiveTab')
const tabsBaseId = inject<string>('tabsBaseId', 'tabs')

const isActive = computed(() => activeTab?.value === props.value)
const valueId = computed(() => props.value.replace(/[^A-Za-z0-9_-]/g, '-'))
const tabId = computed(() => `${tabsBaseId}-tab-${valueId.value}`)
const panelId = computed(() => `${tabsBaseId}-panel-${valueId.value}`)

const handleClick = () => {
  setActiveTab?.(props.value)
}

const handleKeydown = (event: KeyboardEvent) => {
  const keys = ['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp', 'Home', 'End']
  if (!keys.includes(event.key)) return

  const tabList = (event.currentTarget as HTMLElement | null)?.closest('[role="tablist"]')
  const tabs = Array.from(tabList?.querySelectorAll<HTMLButtonElement>('[role="tab"]:not(:disabled)') ?? [])
  if (tabs.length === 0) return

  const currentIndex = tabs.indexOf(event.currentTarget as HTMLButtonElement)
  if (currentIndex === -1) return

  event.preventDefault()

  let nextIndex = currentIndex
  if (event.key === 'Home') {
    nextIndex = 0
  } else if (event.key === 'End') {
    nextIndex = tabs.length - 1
  } else if (event.key === 'ArrowRight' || event.key === 'ArrowDown') {
    nextIndex = (currentIndex + 1) % tabs.length
  } else if (event.key === 'ArrowLeft' || event.key === 'ArrowUp') {
    nextIndex = (currentIndex - 1 + tabs.length) % tabs.length
  }

  tabs[nextIndex]?.focus()
  tabs[nextIndex]?.click()
}

const triggerClass = computed(() => {
  return cn(
    'relative z-10 inline-flex items-center justify-center whitespace-nowrap rounded-md px-3 py-1.5 text-sm font-medium ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50',
    isActive.value
      ? 'text-foreground font-semibold'
      : 'text-muted-foreground hover:text-foreground',
    props.class
  )
})
</script>
