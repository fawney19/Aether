<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, useAttrs, useSlots } from 'vue'
import { ArrowDown, ArrowUp, ArrowUpDown, ListFilter } from 'lucide-vue-next'

import { cn } from '@/lib/utils'
import TableHead from './table-head.vue'

type SortDirection = 'asc' | 'desc'

const props = withDefaults(defineProps<{
  class?: string
  columnKey?: string
  sortable?: boolean
  activeKey?: string | null
  direction?: SortDirection
  defaultDirection?: SortDirection
  align?: 'left' | 'center' | 'right'
  title?: string
  filterActive?: boolean
  filterTitle?: string
  filterContentClass?: string
}>(), {
  columnKey: undefined,
  sortable: true,
  activeKey: null,
  direction: 'asc',
  defaultDirection: 'asc',
  align: 'left',
  title: undefined,
  filterActive: false,
  filterTitle: '筛选',
  filterContentClass: undefined,
})

const emit = defineEmits<{
  sort: [payload: { key: string, direction: SortDirection }]
}>()

defineOptions({
  inheritAttrs: false,
})

const attrs = useAttrs()
const slots = useSlots()
const rootRef = ref<HTMLElement | null>(null)
const filterTriggerRef = ref<HTMLButtonElement | null>(null)
const filterPanelRef = ref<HTMLElement | null>(null)
const filterOpen = ref(false)
const filterPanelStyle = ref<Record<string, string>>({})
const canSort = computed(() => props.sortable && Boolean(props.columnKey))
const hasFilter = computed(() => Boolean(slots.filter))
const isActive = computed(() => props.activeKey === props.columnKey)
const nextDirection = computed<SortDirection>(() => {
  if (!isActive.value) return props.defaultDirection
  return props.direction === 'asc' ? 'desc' : 'asc'
})
const icon = computed(() => {
  if (!isActive.value) return ArrowUpDown
  return props.direction === 'asc' ? ArrowUp : ArrowDown
})
const ariaSort = computed(() => {
  if (!canSort.value) return undefined
  if (!isActive.value) return 'none'
  return props.direction === 'asc' ? 'ascending' : 'descending'
})
const wrapperClass = computed(() => cn(
  'relative flex w-full items-center gap-1.5',
  props.align === 'center' && 'justify-center',
  props.align === 'right' && 'justify-end',
))
const labelClass = computed(() => cn(
  'inline-flex min-w-0 items-center gap-1.5 text-xs font-semibold text-muted-foreground',
  props.align === 'center' && 'justify-center',
  props.align === 'right' && 'justify-end',
))
const buttonClass = computed(() => cn(
  labelClass.value,
  'rounded-sm transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
))
const iconClass = computed(() => cn(
  'h-3.5 w-3.5 shrink-0 transition-colors',
  isActive.value ? 'text-foreground' : 'text-muted-foreground/60',
))
const filterButtonClass = computed(() => cn(
  'inline-flex h-5 w-5 shrink-0 items-center justify-center rounded-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2',
  props.filterActive
    ? 'bg-primary/10 text-primary hover:bg-primary/15'
    : 'text-muted-foreground/60 hover:bg-muted/50 hover:text-foreground',
))
const filterPanelClass = computed(() => cn(
  'fixed z-[1000] w-64 rounded-md border bg-popover p-3 text-popover-foreground shadow-md outline-none',
  props.filterContentClass,
))

function handleSort() {
  if (!canSort.value || !props.columnKey) return
  emit('sort', {
    key: props.columnKey,
    direction: nextDirection.value,
  })
}

function updateFilterPosition() {
  if (!filterOpen.value || !filterTriggerRef.value) return

  const rect = filterTriggerRef.value.getBoundingClientRect()
  const panelWidth = filterPanelRef.value?.offsetWidth ?? 256
  const viewportPadding = 8
  let left = rect.left

  if (props.align === 'center') {
    left = rect.left + rect.width / 2 - panelWidth / 2
  } else if (props.align === 'right') {
    left = rect.right - panelWidth
  }

  left = Math.max(viewportPadding, Math.min(left, window.innerWidth - panelWidth - viewportPadding))

  filterPanelStyle.value = {
    left: `${Math.round(left)}px`,
    top: `${Math.round(rect.bottom + 8)}px`,
  }
}

async function openFilter() {
  filterOpen.value = true
  await nextTick()
  updateFilterPosition()
}

function toggleFilter() {
  if (filterOpen.value) {
    closeFilter()
  } else {
    void openFilter()
  }
}

function closeFilter() {
  filterOpen.value = false
}

function handleDocumentPointerDown(event: PointerEvent) {
  if (!filterOpen.value) return
  const target = event.target
  if (target instanceof Node && rootRef.value?.contains(target)) return
  if (target instanceof Node && filterPanelRef.value?.contains(target)) return
  closeFilter()
}

function handleDocumentKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    closeFilter()
  }
}

onMounted(() => {
  document.addEventListener('pointerdown', handleDocumentPointerDown)
  document.addEventListener('keydown', handleDocumentKeydown)
  document.addEventListener('scroll', updateFilterPosition, true)
  window.addEventListener('resize', updateFilterPosition)
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', handleDocumentPointerDown)
  document.removeEventListener('keydown', handleDocumentKeydown)
  document.removeEventListener('scroll', updateFilterPosition, true)
  window.removeEventListener('resize', updateFilterPosition)
})
</script>

<template>
  <TableHead
    v-bind="attrs"
    :class="cn(props.class, (canSort || hasFilter) && 'select-none')"
    :aria-sort="ariaSort"
  >
    <div
      ref="rootRef"
      :class="wrapperClass"
    >
      <template v-if="hasFilter">
        <button
          ref="filterTriggerRef"
          type="button"
          :class="filterButtonClass"
          :title="filterTitle"
          :aria-pressed="filterActive"
          @click.stop="toggleFilter"
        >
          <ListFilter class="h-3.5 w-3.5" />
        </button>
        <Teleport to="body">
          <div
            v-if="filterOpen"
            ref="filterPanelRef"
            :class="filterPanelClass"
            :style="filterPanelStyle"
            @click.stop
          >
            <slot
              name="filter"
              :close="closeFilter"
            />
          </div>
        </Teleport>
      </template>
      <button
        v-if="canSort"
        type="button"
        :class="buttonClass"
        :title="title || '排序'"
        @click="handleSort"
      >
        <slot />
        <component
          :is="icon"
          :class="iconClass"
        />
      </button>
      <span
        v-else
        :class="labelClass"
      >
        <slot />
      </span>
    </div>
  </TableHead>
</template>
