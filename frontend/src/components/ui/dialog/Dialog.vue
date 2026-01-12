<template>
  <Teleport to="body">
    <div
      v-if="isOpen"
      class="fixed inset-0 overflow-y-auto pointer-events-none"
      :style="{ zIndex: containerZIndex }"
    >
      <!-- 背景遮罩 -->
      <Transition
        enter-active-class="duration-200 ease-out"
        enter-from-class="opacity-0"
        enter-to-class="opacity-100"
        leave-active-class="duration-200 ease-in"
        leave-from-class="opacity-100"
        leave-to-class="opacity-0"
      >
        <div
          v-if="isOpen"
          class="fixed inset-0 bg-black/40 backdrop-blur-sm transition-opacity pointer-events-auto"
          :style="{ zIndex: backdropZIndex }"
          @click="handleBackdropClick"
        />
      </Transition>

      <div class="relative flex min-h-full items-end justify-center px-3 py-4 text-center sm:items-center sm:p-0 pointer-events-none">
        <!-- 对话框内容 -->
        <Transition
          enter-active-class="duration-300 ease-out"
          enter-from-class="opacity-0 translate-y-4 sm:translate-y-0 sm:scale-95"
          enter-to-class="opacity-100 translate-y-0 sm:scale-100"
          leave-active-class="duration-200 ease-in"
          leave-from-class="opacity-100 translate-y-0 sm:scale-100"
          leave-to-class="opacity-0 translate-y-4 sm:translate-y-0 sm:scale-95"
        >
          <div
            v-if="isOpen"
            class="relative transform rounded-lg bg-background text-left shadow-2xl transition-all w-full sm:my-8 sm:w-full border border-border pointer-events-auto"
            :style="{ zIndex: contentZIndex }"
            :class="maxWidthClass"
            @click.stop
          >
            <!-- Header 区域：优先使用 slot，否则使用 title prop -->
            <slot name="header">
              <div
                v-if="title"
                class="border-b border-border px-6 py-4"
              >
                <div class="flex items-center gap-3">
                  <div
                    v-if="icon"
                    class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0"
                    :class="iconClass"
                  >
                    <component
                      :is="icon"
                      class="h-5 w-5 text-primary"
                    />
                  </div>
                  <div class="flex-1 min-w-0">
                    <h3 class="text-lg font-semibold text-foreground leading-tight">
                      {{ title }}
                    </h3>
                    <p
                      v-if="description"
                      class="text-xs text-muted-foreground"
                    >
                      {{ description }}
                    </p>
                  </div>
                </div>
              </div>
            </slot>

            <!-- 内容区域：可选添加 padding -->
            <div :class="noPadding ? '' : 'px-6 py-3'">
              <slot />
            </div>

            <!-- Footer 区域：如果有 footer 插槽，自动添加样式 -->
            <div
              v-if="slots.footer"
              class="border-t border-border px-6 py-4 bg-muted/10 flex flex-row-reverse gap-3"
            >
              <slot name="footer" />
            </div>
          </div>
        </Transition>
      </div>
    </div>
  </Teleport>
</template>

<script setup lang="ts">
import { computed, useSlots, type Component } from 'vue'
import { useEscapeKey } from '@/composables/useEscapeKey'

// Props 定义
const props = defineProps<{
  open?: boolean
  modelValue?: boolean
  size?: 'sm' | 'md' | 'lg' | 'xl' | '2xl' | '3xl' | '4xl' | '5xl' | '6xl' | '7xl'
  maxWidth?: 'sm' | 'md' | 'lg' | 'xl' | '2xl' | '3xl' | '4xl' | '5xl' | '6xl' | '7xl'
  title?: string
  description?: string
  icon?: Component // Lucide icon component
  iconClass?: string // Custom icon color class
  zIndex?: number // Custom z-index for nested dialogs (default: 60)
  noPadding?: boolean // Disable default content padding
  persistent?: boolean // Prevent closing on backdrop click
}>()

// Emits 定义
const emit = defineEmits<{
  'update:open': [value: boolean]
  'update:modelValue': [value: boolean]
}>()

// 获取 slots 以便在模板中使用
const slots = useSlots()

// 统一处理 open 状态
const isOpen = computed(() => {
  if (props.modelValue === true) {
    return true
  }
  if (props.open === true) {
    return true
  }
  return false
})

// 统一处理关闭事件
function handleClose() {
  if (props.open !== undefined) {
    emit('update:open', false)
  }
  if (props.modelValue !== undefined) {
    emit('update:modelValue', false)
  }
}

// 处理背景点击
function handleBackdropClick() {
  if (!props.persistent) {
    handleClose()
  }
}

const maxWidthClass = computed(() => {
  const sizeValue = props.maxWidth || props.size || 'md'
  const sizes = {
    sm: 'sm:max-w-sm',
    md: 'sm:max-w-md',
    lg: 'sm:max-w-lg',
    xl: 'sm:max-w-xl',
    '2xl': 'sm:max-w-2xl',
    '3xl': 'sm:max-w-3xl',
    '4xl': 'sm:max-w-4xl',
    '5xl': 'sm:max-w-5xl',
    '6xl': 'sm:max-w-6xl',
    '7xl': 'sm:max-w-7xl'
  }
  return sizes[sizeValue]
})

// Z-index computed values for nested dialogs support
const containerZIndex = computed(() => props.zIndex || 60)
const backdropZIndex = computed(() => props.zIndex || 60)
const contentZIndex = computed(() => (props.zIndex || 60) + 10)

// 添加 ESC 键监听
useEscapeKey(() => {
  if (isOpen.value && !props.persistent) {
    handleClose()
    return true  // 阻止其他监听器（如父级抽屉的 ESC 监听器）
  }
  return false
}, {
  disableOnInput: true,
  once: false
})
</script>
