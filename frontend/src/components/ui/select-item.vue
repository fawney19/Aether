<script setup lang="ts">
import {
  SelectItem as SelectItemPrimitive,
  SelectItemIndicator,
  SelectItemText,
} from 'radix-vue'
import { Check } from 'lucide-vue-next'
import { cn } from '@/lib/utils'
import {
  computed,
  getCurrentInstance,
  inject,
  onBeforeUnmount,
  onMounted,
  useSlots,
  watch,
} from 'vue'
import { SELECT_SEARCH_CONTEXT_KEY } from './select-search-context'

interface Props {
  class?: string
  value: string
  disabled?: boolean
  textValue?: string
}

const props = defineProps<Props>()
const slots = useSlots()
const searchContext = inject(SELECT_SEARCH_CONTEXT_KEY, null)
const instance = getCurrentInstance()
const itemId = instance?.uid
  ? `select-item-${instance.uid}`
  : `select-item-${Math.random().toString(36).slice(2, 10)}`

function extractText(node: unknown): string {
  if (typeof node === 'string' || typeof node === 'number') {
    return String(node)
  }

  if (Array.isArray(node)) {
    return node.map(extractText).join(' ')
  }

  if (node && typeof node === 'object') {
    const vnode = node as { children?: unknown }
    return extractText(vnode.children)
  }

  return ''
}

const normalizedText = computed(() => {
  const slotText = extractText(slots.default?.()).replace(/\s+/g, ' ').trim()
  return (props.textValue ?? slotText ?? props.value).trim()
})

const isHidden = computed(
  () => searchContext?.hiddenValues.value.has(props.value) ?? false,
)
const itemClass = computed(() =>
  cn(
    'relative flex min-w-0 w-full cursor-pointer select-none items-center rounded-lg py-1.5 pl-8 pr-2 text-sm outline-none',
    'data-[highlighted]:bg-accent focus:bg-accent text-foreground',
    'transition-colors data-[disabled]:pointer-events-none data-[disabled]:opacity-50',
    isHidden.value && 'hidden',
    props.class,
  ),
)

onMounted(() => {
  searchContext?.registerItem(itemId, {
    value: props.value,
    text: normalizedText.value,
  })
})

watch(
  () => [props.value, normalizedText.value] as const,
  ([value, text]) => {
    searchContext?.updateItem(itemId, {
      value,
      text,
    })
  },
)

onBeforeUnmount(() => {
  searchContext?.unregisterItem(itemId)
})
</script>

<template>
  <SelectItemPrimitive
    :class="itemClass"
    :value="value"
    :disabled="disabled || isHidden"
    :text-value="normalizedText"
  >
    <span class="absolute left-2 flex h-3.5 w-3.5 items-center justify-center">
      <SelectItemIndicator>
        <Check class="h-4 w-4" />
      </SelectItemIndicator>
    </span>
    <SelectItemText
      as="div"
      class="min-w-0 flex-1 overflow-hidden"
    >
      <span
        class="block w-full min-w-0 truncate"
        :title="normalizedText"
      >
        <slot />
      </span>
    </SelectItemText>
  </SelectItemPrimitive>
</template>
