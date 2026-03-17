<template>
  <div
    v-if="masked"
    class="group relative"
  >
    <input
      ref="inputRef"
      :class="inputClass"
      :value="modelValue"
      :type="effectiveType"
      :autocomplete="autocompleteAttr"
      :data-lpignore="shouldDisableAutofill ? 'true' : undefined"
      :data-1p-ignore="shouldDisableAutofill ? 'true' : undefined"
      :data-form-type="shouldDisableAutofill ? 'other' : undefined"
      :data-protonpass-ignore="shouldDisableAutofill ? 'true' : undefined"
      :data-bwignore="shouldDisableAutofill ? 'true' : undefined"
      :data-bitwarden-watching="shouldDisableAutofill ? 'false' : undefined"
      :name="shouldDisableAutofill ? randomName : undefined"
      v-bind="filteredAttrs"
      @input="handleInput"
    >
    <button
      v-if="hasValue"
      type="button"
      class="absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground/20 hover:text-muted-foreground/50 transition-colors"
      tabindex="-1"
      :aria-label="isVisible ? '隐藏内容' : '显示内容'"
      @click="toggleVisibility"
    >
      <EyeOff
        v-if="isVisible"
        class="h-4 w-4"
      />
      <Eye
        v-else
        class="h-4 w-4"
      />
    </button>
  </div>
  <input
    v-else
    ref="inputRef"
    :class="inputClass"
    :value="modelValue"
    :type="effectiveType"
    :autocomplete="autocompleteAttr"
    :data-lpignore="shouldDisableAutofill ? 'true' : undefined"
    :data-1p-ignore="shouldDisableAutofill ? 'true' : undefined"
    :data-form-type="shouldDisableAutofill ? 'other' : undefined"
    :data-protonpass-ignore="shouldDisableAutofill ? 'true' : undefined"
    :data-bwignore="shouldDisableAutofill ? 'true' : undefined"
    :data-bitwarden-watching="shouldDisableAutofill ? 'false' : undefined"
    :name="shouldDisableAutofill ? randomName : undefined"
    v-bind="filteredAttrs"
    @input="handleInput"
  >
</template>

<script setup lang="ts">
import { computed, useAttrs, ref } from 'vue'
import { Eye, EyeOff } from 'lucide-vue-next'
import { cn } from '@/lib/utils'

const props = defineProps<Props>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

interface Props {
  modelValue?: string | number
  class?: string
  autocomplete?: string
  /**
   * 输入框尺寸
   * - 'default': 默认尺寸 (h-11, py-2)
   * - 'sm': 小尺寸 (h-8, py-1)
   */
  size?: 'default' | 'sm'
  /**
   * 遮蔽显示内容（用于 API Key 等敏感信息）
   * 隐藏态使用真正的 password 输入框，显示态切换为 text
   * 同时会显示一个小眼睛按钮用于切换显示/隐藏
   */
  masked?: boolean
  /**
   * 禁用浏览器自动填充
   * - true: 禁用自动填充
   * - false: 允许自动填充（默认）
   */
  disableAutofill?: boolean
}

const attrs = useAttrs()
const inputRef = ref<HTMLInputElement | null>(null)
const isVisible = ref(false)

// 判断是否有值
const hasValue = computed(() => {
  return props.modelValue !== undefined && props.modelValue !== null && props.modelValue !== ''
})

function toggleVisibility() {
  isVisible.value = !isVisible.value
}

// 计算是否应该禁用自动填充
const shouldDisableAutofill = computed(() => {
  // masked 模式默认禁用自动填充
  if (props.masked && props.disableAutofill === undefined) {
    return true
  }
  return props.disableAutofill ?? false
})

const effectiveType = computed(() => {
  const attrType = (attrs.type as string | undefined) ?? 'text'
  if (props.masked) {
    return isVisible.value ? 'text' : 'password'
  }
  return attrType
})

// 过滤掉 type 和 class 属性，因为我们会单独处理
const filteredAttrs = computed(() => {
  const { type: _type, class: _class, ...rest } = attrs
  return rest
})

// 生成一个稳定的随机值（组件实例级别）
const randomSuffix = Math.random().toString(36).substring(2, 8)
const randomName = `field_${randomSuffix}`

const autocompleteAttr = computed(() => {
  // 如果显式设置了 autocomplete 且不禁用自动填充，使用该值
  if (props.autocomplete && !shouldDisableAutofill.value) {
    return props.autocomplete
  }
  // 禁用自动填充时，使用浏览器无法识别的随机值
  if (shouldDisableAutofill.value) {
    return `off-${randomSuffix}`
  }
  return props.autocomplete ?? 'off'
})

// 尺寸相关的样式
const sizeClasses = {
  default: 'h-11 py-2 px-4',
  sm: 'h-8 py-1 px-3'
}

const inputClass = computed(() =>
  cn(
    'flex w-full rounded-xl border border-border/60 bg-muted/50 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40 focus-visible:border-primary/60 text-foreground transition-all',
    sizeClasses[props.size || 'default'],
    props.masked && 'pr-10',
    props.class
  )
)

function handleInput(event: Event) {
  const target = event.target as HTMLInputElement
  emit('update:modelValue', target.value)
}

defineExpose({ inputRef })
</script>
