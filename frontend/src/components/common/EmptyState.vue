<template>
  <div :class="containerClasses">
    <!-- 图标 -->
    <div :class="iconContainerClasses">
      <component
        :is="icon"
        v-if="icon"
        :class="iconClasses"
      />
      <component
        :is="defaultIcon"
        v-else
        :class="iconClasses"
      />
    </div>

    <!-- 标题 -->
    <h3
      v-if="title"
      :class="titleClasses"
    >
      {{ title }}
    </h3>

    <!-- 描述 -->
    <p
      v-if="description"
      :class="descriptionClasses"
    >
      {{ description }}
    </p>

    <!-- 自定义内容插槽 -->
    <div
      v-if="$slots.default"
      class="mt-4"
    >
      <slot />
    </div>

    <!-- 操作按钮 -->
    <div
      v-if="$slots.actions || actionText"
      class="mt-6 flex flex-wrap items-center justify-center gap-3"
    >
      <slot name="actions">
        <Button
          v-if="actionText"
          :variant="actionVariant"
          :size="actionSize"
          @click="handleAction"
        >
          <component
            :is="actionIcon"
            v-if="actionIcon"
            class="mr-2 h-4 w-4"
          />
          {{ actionText }}
        </Button>
      </slot>
    </div>

    <!-- 次要操作 -->
    <div
      v-if="$slots.secondary"
      class="mt-3"
    >
      <slot name="secondary" />
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import Button from '@/components/ui/button.vue'
import {
  FileQuestion,
  Search,
  Inbox,
  AlertCircle,
  PackageOpen,
  Filter
} from 'lucide-vue-next'
import type { Component } from 'vue'

type EmptyStateType = 'default' | 'search' | 'filter' | 'error' | 'empty' | 'notFound'
type ButtonVariant = 'default' | 'outline' | 'secondary' | 'ghost' | 'link' | 'destructive'
type ButtonSize = 'sm' | 'default' | 'lg' | 'icon'

interface Props {
  /** 空状态类型 */
  type?: EmptyStateType
  /** 自定义图标组件 */
  icon?: Component
  /** 标题 */
  title?: string
  /** 描述文本 */
  description?: string
  /** 操作按钮文本 */
  actionText?: string
  /** 操作按钮图标 */
  actionIcon?: Component
  /** 操作按钮变体 */
  actionVariant?: ButtonVariant
  /** 操作按钮大小 */
  actionSize?: ButtonSize
  /** 大小 */
  size?: 'sm' | 'md' | 'lg'
  /** 对齐方式 */
  align?: 'left' | 'center' | 'right'
}

interface Emits {
  (e: 'action'): void
}

const props = withDefaults(defineProps<Props>(), {
  type: 'default',
  icon: undefined,
  title: undefined,
  description: undefined,
  actionText: undefined,
  actionIcon: undefined,
  actionVariant: 'default',
  actionSize: 'default',
  size: 'md',
  align: 'center'
})

const emit = defineEmits<Emits>()

// 根据类型获取默认配置
const typeConfig = computed(() => {
  const configs = {
    default: {
      icon: Inbox,
      title: '暂无数据',
      description: '当前没有可显示的内容'
    },
    search: {
      icon: Search,
      title: '未找到结果',
      description: '尝试使用不同的关键词搜索'
    },
    filter: {
      icon: Filter,
      title: '无匹配结果',
      description: '没有符合当前筛选条件的数据'
    },
    error: {
      icon: AlertCircle,
      title: '加载失败',
      description: '数据加载过程中出现错误'
    },
    empty: {
      icon: PackageOpen,
      title: '这里空空如也',
      description: '还没有任何内容'
    },
    notFound: {
      icon: FileQuestion,
      title: '未找到',
      description: '请求的资源不存在'
    }
  }

  return configs[props.type]
})

// 默认图标
const defaultIcon = computed(() => typeConfig.value.icon)

// 容器样式
const containerClasses = computed(() => {
  const classes = ['empty-state']

  // 大小
  if (props.size === 'sm') {
    classes.push('empty-state-sm', 'py-6')
  } else if (props.size === 'lg') {
    classes.push('empty-state-lg', 'py-16')
  } else {
    classes.push('empty-state-md', 'py-12')
  }

  // 对齐
  if (props.align === 'left') {
    classes.push('text-left')
  } else if (props.align === 'right') {
    classes.push('text-right')
  } else {
    classes.push('text-center')
  }

  return classes.join(' ')
})

// 图标容器样式
const iconContainerClasses = computed(() => {
  const classes = [
    'empty-state-icon-container',
    'rounded-full',
    'inline-flex',
    'items-center',
    'justify-center',
    'mb-4'
  ]

  // 大小和颜色
  if (props.type === 'error') {
    classes.push('bg-red-100', 'dark:bg-red-900/30')
  } else if (props.type === 'search' || props.type === 'filter') {
    classes.push('bg-blue-100', 'dark:bg-blue-900/30')
  } else {
    classes.push('bg-muted')
  }

  // 尺寸
  if (props.size === 'sm') {
    classes.push('w-12', 'h-12')
  } else if (props.size === 'lg') {
    classes.push('w-20', 'h-20')
  } else {
    classes.push('w-16', 'h-16')
  }

  return classes.join(' ')
})

// 图标样式
const iconClasses = computed(() => {
  const classes = []

  // 颜色
  if (props.type === 'error') {
    classes.push('text-red-600', 'dark:text-red-400')
  } else if (props.type === 'search' || props.type === 'filter') {
    classes.push('text-blue-600', 'dark:text-blue-400')
  } else {
    classes.push('text-muted-foreground')
  }

  // 尺寸
  if (props.size === 'sm') {
    classes.push('w-6', 'h-6')
  } else if (props.size === 'lg') {
    classes.push('w-10', 'h-10')
  } else {
    classes.push('w-8', 'h-8')
  }

  return classes.join(' ')
})

// 标题样式
const titleClasses = computed(() => {
  const classes = ['font-semibold', 'text-foreground', 'mb-2']

  if (props.size === 'sm') {
    classes.push('text-base')
  } else if (props.size === 'lg') {
    classes.push('text-2xl')
  } else {
    classes.push('text-lg')
  }

  return classes.join(' ')
})

// 描述样式
const descriptionClasses = computed(() => {
  const classes = ['text-muted-foreground', 'max-w-md']

  if (props.align === 'center') {
    classes.push('mx-auto')
  }

  if (props.size === 'sm') {
    classes.push('text-xs')
  } else if (props.size === 'lg') {
    classes.push('text-base')
  } else {
    classes.push('text-sm')
  }

  return classes.join(' ')
})

// 处理操作
function handleAction() {
  emit('action')
}
</script>

<style scoped>
.empty-state {
  @apply flex flex-col items-center justify-center;
}

.empty-state-icon-container {
  @apply transition-transform duration-200;
}

.empty-state:hover .empty-state-icon-container {
  @apply scale-105;
}
</style>
