<template>
  <div class="relative">
    <button
      type="button"
      :class="
        cn(
          'flex h-10 w-full items-center justify-between rounded-lg border bg-background px-3 text-left transition-colors',
          disabled ? 'cursor-not-allowed opacity-60 hover:bg-background' : 'hover:bg-muted/50',
          triggerClass,
        )
      "
      :disabled="disabled"
      @click="isOpen = !isOpen"
    >
      <span
        :class="modelValue.length ? 'text-foreground' : 'text-muted-foreground'"
        class="truncate text-sm"
      >
        {{ displayText }}
        <span
          v-if="invalidItems.length"
          class="text-destructive"
        >({{ invalidItems.length }} 个已失效)</span>
      </span>
      <ChevronDown
        class="h-4 w-4 shrink-0 text-muted-foreground transition-transform"
        :class="isOpen ? 'rotate-180' : ''"
      />
    </button>
    <div
      v-if="isOpen"
      class="fixed inset-0 z-[80]"
      @click.stop="isOpen = false"
    />
    <div
      v-if="isOpen"
      class="absolute z-[90] w-full mt-1 rounded-lg border bg-popover shadow-lg"
      :style="dropdownMinWidth ? { minWidth: dropdownMinWidth } : undefined"
    >
      <div
        v-if="showSearch"
        class="sticky top-0 z-10 border-b bg-popover/95 p-1 backdrop-blur supports-[backdrop-filter]:bg-popover/85"
      >
        <div class="relative">
          <Search
            class="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            v-model="searchQuery"
            :placeholder="searchPlaceholder"
            class="h-8 rounded-md border-border/60 bg-popover pl-8 text-xs"
            @keydown.stop
          />
        </div>
      </div>

      <div class="max-h-48 overflow-y-auto">
        <div
          v-if="hasOptions"
          class="sticky top-0 z-10 flex cursor-pointer items-center gap-2 border-b bg-popover/95 px-3 py-2 backdrop-blur hover:bg-muted/50 supports-[backdrop-filter]:bg-popover/85"
          @click="toggleAll"
        >
          <input
            type="checkbox"
            :checked="isAllSelected"
            :indeterminate="isPartiallySelected"
            aria-label="全选"
            class="h-4 w-4 shrink-0 cursor-pointer rounded border-gray-300"
            @click.stop
            @change="toggleAll"
          >
          <span class="min-w-0 truncate text-sm">全选</span>
          <span class="ml-auto shrink-0 text-xs text-muted-foreground">
            {{ selectedOptionCount }}/{{ options.length }}
          </span>
        </div>

        <div
          v-for="item in filteredInvalidItems"
          :key="'invalid-' + item"
          class="flex cursor-pointer items-center gap-2 bg-destructive/5 px-3 py-2 hover:bg-muted/50"
          @click="remove(item)"
        >
          <input
            type="checkbox"
            :checked="true"
            class="h-4 w-4 shrink-0 cursor-pointer rounded border-gray-300"
            @click.stop
            @change="remove(item)"
          >
          <span class="min-w-0 truncate text-sm text-destructive">{{ item }}</span>
          <span class="shrink-0 text-xs text-destructive/70">(已失效)</span>
        </div>

        <div
          v-for="item in filteredOptions"
          :key="item.value"
          class="flex cursor-pointer items-center gap-2 px-3 py-2 hover:bg-muted/50"
          @click="toggle(item.value)"
        >
          <input
            type="checkbox"
            :checked="modelValue.includes(item.value)"
            class="h-4 w-4 shrink-0 cursor-pointer rounded border-gray-300"
            @click.stop
            @change="toggle(item.value)"
          >
          <span class="min-w-0 truncate text-sm">{{ item.label }}</span>
        </div>
        <div
          v-if="filteredOptions.length === 0 && filteredInvalidItems.length === 0"
          class="px-3 py-2 text-sm text-muted-foreground"
        >
          {{ searchQuery.trim() ? noResultsText : emptyText }}
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ChevronDown, Search } from 'lucide-vue-next'
import { Input } from '@/components/ui'
import { cn } from '@/lib/utils'
import { matchesSearchQuery } from '@/utils/search'

export interface MultiSelectOption {
  value: string
  label: string
}

const props = withDefaults(
  defineProps<{
    modelValue: string[]
    options: MultiSelectOption[]
    placeholder?: string
    emptyText?: string
    noResultsText?: string
    triggerClass?: string
    dropdownMinWidth?: string
    disabled?: boolean
    searchable?: boolean
    searchThreshold?: number
    searchPlaceholder?: string
  }>(),
  {
    placeholder: '请选择',
    emptyText: '暂无选项',
    noResultsText: '未找到匹配项',
    triggerClass: '',
    dropdownMinWidth: undefined,
    disabled: false,
    searchable: true,
    searchThreshold: 8,
    searchPlaceholder: '输入关键词搜索...',
  },
)

const emit = defineEmits<{
  'update:modelValue': [value: string[]]
}>()

const isOpen = ref(false)
const searchQuery = ref('')

const validValues = computed(() => new Set(props.options.map(o => o.value)))

const invalidItems = computed(() =>
  props.modelValue.filter(v => !validValues.value.has(v)),
)

const totalCount = computed(() => props.options.length + invalidItems.value.length)

const hasOptions = computed(() => props.options.length > 0)

const selectedOptionCount = computed(
  () => new Set(props.modelValue.filter(v => validValues.value.has(v))).size,
)

const isAllSelected = computed(
  () => hasOptions.value && selectedOptionCount.value === props.options.length,
)

const isPartiallySelected = computed(
  () => selectedOptionCount.value > 0 && !isAllSelected.value,
)

const showSearch = computed(
  () => props.searchable && totalCount.value >= props.searchThreshold,
)

const filteredInvalidItems = computed(() => {
  if (!showSearch.value || !searchQuery.value.trim()) {
    return invalidItems.value
  }
  return invalidItems.value.filter((item) =>
    matchesSearchQuery(searchQuery.value, item),
  )
})

const filteredOptions = computed(() => {
  if (!showSearch.value || !searchQuery.value.trim()) {
    return props.options
  }

  return props.options.filter((item) =>
    matchesSearchQuery(searchQuery.value, item.label, item.value),
  )
})

const displayText = computed(() => {
  if (props.modelValue.length === 0) return props.placeholder
  if (props.modelValue.length <= 2) {
    return props.modelValue
      .map((v) => props.options.find((o) => o.value === v)?.label ?? v)
      .join(', ')
  }
  return `已选择 ${props.modelValue.length} 项`
})

watch(isOpen, (open) => {
  if (!open) {
    searchQuery.value = ''
  }
})

function toggle(value: string) {
  const newValue = [...props.modelValue]
  const index = newValue.indexOf(value)
  if (index === -1) {
    newValue.push(value)
  } else {
    newValue.splice(index, 1)
  }
  emit('update:modelValue', newValue)
}

function remove(value: string) {
  emit('update:modelValue', props.modelValue.filter(v => v !== value))
}

function toggleAll() {
  const invalidValues = props.modelValue.filter(v => !validValues.value.has(v))

  if (isAllSelected.value) {
    emit('update:modelValue', invalidValues)
    return
  }

  emit('update:modelValue', [
    ...invalidValues,
    ...props.options.map(option => option.value),
  ])
}
</script>
