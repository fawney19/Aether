<template>
  <SelectPortal :disabled="shouldDisablePortal">
    <SelectContentPrimitive
      v-bind="$attrs"
      :class="contentClass"
      :position="position"
      :side="side"
      :side-offset="sideOffset"
      :align="align"
      :align-offset="alignOffset"
    >
      <SelectViewport :class="viewportClass">
        <div
          v-if="showSearchInput"
          class="sticky top-0 z-10 bg-card/95 px-1 pt-1 pb-2 backdrop-blur supports-[backdrop-filter]:bg-card/85"
        >
          <div class="relative">
            <Search
              class="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
            />
            <Input
              ref="searchInputRef"
              v-model="searchQuery"
              :placeholder="searchPlaceholder"
              class="h-9 rounded-xl border-border/60 bg-background/80 pl-9 pr-3 text-sm"
              @keydown.stop
            />
          </div>
        </div>

        <slot />

        <div
          v-if="showEmptyState"
          class="px-3 py-2 text-sm text-muted-foreground"
        >
          未找到匹配项
        </div>
      </SelectViewport>
    </SelectContentPrimitive>
  </SelectPortal>
</template>

<script setup lang="ts">
import {
  SelectContent as SelectContentPrimitive,
  SelectPortal,
  SelectViewport,
} from 'radix-vue'
import { Search } from 'lucide-vue-next'
import {
  computed,
  inject,
  nextTick,
  onMounted,
  onUnmounted,
  provide,
  ref,
  watch,
} from 'vue'
import Input from './input.vue'
import { cn } from '@/lib/utils'
import { DIALOG_CONTEXT_KEY } from './dialog/context'
import {
  SELECT_SEARCH_CONTEXT_KEY,
  type RegisteredSelectItem,
} from './select-search-context'
import { matchesSearchQuery, preloadPinyin } from '@/utils/search'

interface Props {
  class?: string
  position?: 'item-aligned' | 'popper'
  side?: 'top' | 'right' | 'bottom' | 'left'
  sideOffset?: number
  align?: 'start' | 'center' | 'end'
  alignOffset?: number
  disablePortal?: boolean
  searchable?: boolean
  searchThreshold?: number
  searchPlaceholder?: string
}

const props = withDefaults(defineProps<Props>(), {
  class: undefined,
  position: 'popper',
  side: undefined,
  sideOffset: 4,
  align: undefined,
  alignOffset: undefined,
  disablePortal: false,
  searchable: true,
  searchThreshold: 8,
  searchPlaceholder: '输入关键词搜索...',
})

const isInsideDialog = inject(DIALOG_CONTEXT_KEY, false)
const shouldDisablePortal = computed(
  () => props.disablePortal || isInsideDialog,
)
const searchQuery = ref('')
const searchInputRef = ref<InstanceType<typeof Input> | null>(null)
const registeredItems = ref<Record<string, RegisteredSelectItem>>({})

const hiddenValues = computed(() => {
  const query = searchQuery.value.trim()
  if (!query) return new Set<string>()

  const hidden = new Set<string>()
  for (const item of Object.values(registeredItems.value)) {
    if (!matchesSearchQuery(query, item.value, item.text)) {
      hidden.add(item.value)
    }
  }
  return hidden
})

provide(SELECT_SEARCH_CONTEXT_KEY, {
  searchQuery,
  hiddenValues,
  registerItem(id, item) {
    registeredItems.value = {
      ...registeredItems.value,
      [id]: item,
    }
  },
  updateItem(id, item) {
    const currentItem = registeredItems.value[id]
    if (!currentItem) {
      return
    }

    registeredItems.value = {
      ...registeredItems.value,
      [id]: {
        ...currentItem,
        ...item,
      },
    }
  },
  unregisterItem(id) {
    if (!(id in registeredItems.value)) {
      return
    }

    const nextItems = { ...registeredItems.value }
    delete nextItems[id]
    registeredItems.value = nextItems
  },
})

const itemCount = computed(() => Object.keys(registeredItems.value).length)
const showSearchInput = computed(
  () => props.searchable && itemCount.value >= props.searchThreshold,
)
const showEmptyState = computed(
  () =>
    showSearchInput.value &&
    searchQuery.value.trim().length > 0 &&
    hiddenValues.value.size === itemCount.value,
)

onMounted(() => {
  if (props.searchable) {
    preloadPinyin()
  }
})

watch(showSearchInput, async (visible) => {
  if (!visible) {
    searchQuery.value = ''
    return
  }

  await nextTick()
  searchInputRef.value?.inputRef?.focus()
})

const contentClass = computed(() =>
  cn(
    'z-[200] max-h-96 min-w-[max(8rem,var(--radix-select-trigger-width))] overflow-hidden rounded-2xl border border-border bg-card text-foreground shadow-2xl backdrop-blur-xl pointer-events-auto',
    'data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0 data-[state=closed]:zoom-out-95 data-[state=open]:zoom-in-95',
    'data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2',
    props.class,
  ),
)

const viewportClass = 'p-1 max-h-[var(--radix-select-content-available-height)]'

onUnmounted(() => {
  searchQuery.value = ''
})
</script>
