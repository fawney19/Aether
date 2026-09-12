<template>
  <TooltipProvider :delay-duration="150">
    <nav
      class="sidebar-nav w-full"
      :class="collapsed ? 'px-2' : 'px-3'"
      :data-collapsed="collapsed"
    >
      <div
        v-for="(group, index) in items"
        :key="index"
        :class="[
          collapsed ? 'mb-1 space-y-0' : 'mb-5 space-y-1',
          collapsed && index > 0 ? 'border-t border-[#3d3929]/10 pt-1 dark:border-white/10' : ''
        ]"
        :data-sidebar-group-divider="collapsed && index > 0 ? '' : undefined"
      >
        <!-- Section Header -->
        <div
          v-if="group.title && !collapsed"
          class="flex items-center gap-2 px-2.5 pb-1"
          :class="index > 0 ? 'pt-1' : ''"
        >
          <span class="text-[10px] font-medium text-muted-foreground/50 font-mono tabular-nums">{{ String(index + 1).padStart(2, '0') }}</span>
          <span class="min-w-0 break-words text-[10px] font-semibold leading-4 text-muted-foreground/70 uppercase tracking-normal">{{ group.title }}</span>
        </div>

        <!-- Links -->
        <div class="space-y-0.5">
          <template
            v-for="item in group.items"
            :key="item.href"
          >
            <Tooltip
              :open="collapsed && openTooltipHref === item.href"
              @update:open="handleTooltipOpenChange(item.href, $event)"
            >
              <TooltipTrigger as-child>
                <RouterLink
                  :to="item.href"
                  class="group relative flex min-w-0 items-center gap-2 rounded-lg"
                  :class="[
                    collapsed
                      ? 'h-9 justify-center px-0 transition-colors duration-150'
                      : 'justify-between px-2.5 py-2 transition-all duration-200',
                    isItemActive(item.href)
                      ? 'bg-primary/10 text-primary font-medium'
                      : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                  ]"
                  :aria-label="collapsed ? item.name : undefined"
                  @pointerenter="schedulePrefetch(item.href)"
                  @pointerleave="cancelScheduledPrefetch(item.href)"
                  @pointerdown="prefetchNow(item.href)"
                  @focus="prefetchNow(item.href)"
                  @click="handleNavigate(item.href)"
                >
                  <div class="flex min-w-0 items-center gap-2.5">
                    <component
                      :is="item.icon"
                      class="h-4 w-4 shrink-0 transition-colors duration-200"
                      :class="isItemActive(item.href) ? 'text-primary' : 'text-muted-foreground/70 group-hover:text-foreground'"
                      :stroke-width="isItemActive(item.href) ? 2 : 1.75"
                    />
                    <span
                      v-if="!collapsed"
                      class="min-w-0 break-words text-[13px] leading-5 tracking-normal"
                    >{{ item.name }}</span>
                  </div>

                  <!-- Active Indicator -->
                  <div
                    v-if="isItemActive(item.href)"
                    class="h-1 w-1 shrink-0 rounded-full bg-primary"
                    :class="collapsed ? 'absolute right-1.5' : ''"
                  />
                </RouterLink>
              </TooltipTrigger>
              <TooltipContent
                v-if="collapsed"
                side="right"
                :side-offset="10"
                class="text-xs"
              >
                {{ item.name }}
              </TooltipContent>
            </Tooltip>
          </template>
        </div>
      </div>
    </nav>
  </TooltipProvider>
</template>

<script setup lang="ts">
import { onBeforeUnmount, ref, type Component } from 'vue'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

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
  collapsed?: boolean
}>()

const emit = defineEmits<{
  (e: 'navigate', href: string): void
  (e: 'prefetch', href: string): void
}>()

const HOVER_PREFETCH_DELAY_MS = 100
let scheduledPrefetchHref: string | null = null
let scheduledPrefetchTimer: ReturnType<typeof setTimeout> | null = null
const openTooltipHref = ref<string | null>(null)

function handleTooltipOpenChange(href: string, open: boolean) {
  if (!props.collapsed) {
    openTooltipHref.value = null
    return
  }
  if (open) {
    openTooltipHref.value = href
  } else if (openTooltipHref.value === href) {
    openTooltipHref.value = null
  }
}

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
