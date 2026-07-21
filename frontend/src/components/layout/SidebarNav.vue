<template>
  <nav class="sidebar-nav w-full px-3">
    <div
      v-for="(group, index) in items"
      :key="index"
      class="space-y-1 mb-5"
    >
      <!-- Section Header -->
      <div
        v-if="group.title"
        class="px-2.5 pb-1 flex items-center gap-2"
        :class="index > 0 ? 'pt-1' : ''"
      >
        <span class="text-[10px] font-medium text-muted-foreground/50 font-mono tabular-nums">{{ String(index + 1).padStart(2, '0') }}</span>
        <span class="text-[10px] font-semibold text-muted-foreground/70 uppercase tracking-[0.1em]">{{ group.title }}</span>
      </div>

      <!-- Links -->
      <div class="space-y-0.5">
        <template
          v-for="item in group.items"
          :key="item.href"
        >
          <RouterLink
            :to="item.href"
            class="group relative flex items-center justify-between px-2.5 py-2 rounded-lg transition-all duration-200"
            :class="[
              isItemActive(item.href)
                ? 'bg-primary/10 text-primary font-medium'
                : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
            ]"
            @pointerenter="schedulePrefetch(item.href)"
            @pointerleave="cancelScheduledPrefetch(item.href)"
            @pointerdown="prefetchNow(item.href)"
            @focus="prefetchNow(item.href)"
            @click="handleNavigate(item.href)"
          >
            <div class="flex items-center gap-2.5">
              <component
                :is="item.icon"
                class="h-4 w-4 transition-colors duration-200"
                :class="isItemActive(item.href) ? 'text-primary' : 'text-muted-foreground/70 group-hover:text-foreground'"
                :stroke-width="isItemActive(item.href) ? 2 : 1.75"
              />
              <span class="text-[13px] tracking-tight">{{ item.name }}</span>
            </div>

            <!-- Active Indicator -->
            <div
              v-if="isItemActive(item.href)"
              class="w-1 h-1 rounded-full bg-primary"
            />
          </RouterLink>
        </template>
      </div>
    </div>
  </nav>
</template>

<script setup lang="ts">
import { onBeforeUnmount, type Component } from 'vue'

export interface NavigationItem {
  name: string
  href: string
  icon: Component
  description?: string
}

export interface NavigationGroup {
  title?: string
  items: NavigationItem[]
}

const props = defineProps<{
  items: NavigationGroup[]
  activePath?: string
  isActive?: (href: string) => boolean
}>()

const emit = defineEmits<{
  (e: 'navigate', href: string): void
  (e: 'prefetch', href: string): void
}>()

const HOVER_PREFETCH_DELAY_MS = 100
let scheduledPrefetchHref: string | null = null
let scheduledPrefetchTimer: ReturnType<typeof setTimeout> | null = null

function cancelScheduledPrefetch(href?: string) {
  if (href && scheduledPrefetchHref !== href) return
  if (scheduledPrefetchTimer) {
    clearTimeout(scheduledPrefetchTimer)
    scheduledPrefetchTimer = null
  }
  scheduledPrefetchHref = null
}

function schedulePrefetch(href: string) {
  cancelScheduledPrefetch()
  scheduledPrefetchHref = href
  scheduledPrefetchTimer = setTimeout(() => {
    scheduledPrefetchTimer = null
    scheduledPrefetchHref = null
    emit('prefetch', href)
  }, HOVER_PREFETCH_DELAY_MS)
}

function prefetchNow(href: string) {
  cancelScheduledPrefetch()
  emit('prefetch', href)
}

onBeforeUnmount(() => cancelScheduledPrefetch())

function isItemActive(href: string) {
  if (props.isActive) {
    return props.isActive(href)
  }
  if (props.activePath) {
    return props.activePath === href || props.activePath.startsWith(`${href}/`)
  }
  return false
}

function handleNavigate(href: string) {
  emit('navigate', href)
}
</script>

<style scoped>
/* Navigation styles handled by Tailwind */
</style>
