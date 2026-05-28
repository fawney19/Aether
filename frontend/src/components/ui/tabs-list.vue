<template>
  <div
    ref="listRef"
    role="tablist"
    :class="listClass"
  >
    <!-- 滑动指示器 - 放在按钮前面 -->
    <div
      aria-hidden="true"
      class="tabs-indicator"
      :style="indicatorStyle"
    />
    <slot />
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted, nextTick, inject, type Ref } from 'vue'
import { cn } from '@/lib/utils'

interface Props {
  class?: string
}

const props = defineProps<Props>()

const listRef = ref<HTMLElement | null>(null)
const indicatorStyle = ref<Record<string, string>>({
  transform: 'translate(0, 0)',
  width: '0px',
  height: '0px',
  opacity: '0',
  transition: 'none'
})

// 标记是否已完成首次定位（首次无动画）
const hasInitialized = ref(false)
// 记录当前激活的 tab 索引，用于计算相对位置
const activeIndex = ref(-1)

const activeTab = inject<Ref<string>>('activeTab')

// 检查是否有 grid 类（由外部传入）
const hasGridClass = computed(() => {
  return props.class?.includes('grid')
})

const listClass = computed(() => {
  return cn(
    'tabs-list',
    // 如果外部传入了 grid 类，就不使用默认的 inline-flex
    !hasGridClass.value && 'inline-flex',
    props.class
  )
})

// 更新指示器位置
const updateIndicator = () => {
  if (!listRef.value) return

  const buttons = Array.from(
    listRef.value.querySelectorAll<HTMLButtonElement>('button[data-value]')
  )

  // 确保所有 button 都已渲染且有 data-value
  if (buttons.length === 0) return

  const newIndex = buttons.findIndex(
    (button) => button.dataset.value === activeTab?.value
  )

  if (newIndex === -1) {
    indicatorStyle.value = {
      transform: 'translate(0, 0)',
      width: '0px',
      height: '0px',
      opacity: '0',
      transition: 'none'
    }
    return
  }

  const activeButton = buttons[newIndex]
  const listRect = listRef.value.getBoundingClientRect()
  const buttonRect = activeButton.getBoundingClientRect()

  // 确保按钮已渲染
  if (buttonRect.width === 0) return

  // 计算相对位置：使用真实 DOM 坐标，避免 grid gap / padding / 换行被漏算导致指示器偏移
  const offsetLeft = Math.max(0, buttonRect.left - listRect.left)
  const offsetTop = Math.max(0, buttonRect.top - listRect.top)

  // 判断是否需要动画：
  // 1. 首次初始化不需要动画
  // 2. 索引变化（用户切换 tab）需要动画
  const isTabChange = hasInitialized.value && activeIndex.value !== newIndex && activeIndex.value !== -1

  // 更新状态
  activeIndex.value = newIndex
  if (!hasInitialized.value) {
    hasInitialized.value = true
  }

  indicatorStyle.value = {
    transform: `translate(${offsetLeft}px, ${offsetTop}px)`,
    width: `${buttonRect.width}px`,
    height: `${buttonRect.height}px`,
    opacity: '1',
    transition: isTabChange
      ? 'transform 0.2s cubic-bezier(0.4, 0, 0.2, 1), width 0.2s cubic-bezier(0.4, 0, 0.2, 1), height 0.2s cubic-bezier(0.4, 0, 0.2, 1)'
      : 'none'
  }
}

let rafId: number | null = null

const scheduleIndicatorUpdate = () => {
  if (rafId !== null) {
    cancelAnimationFrame(rafId)
  }
  rafId = requestAnimationFrame(() => {
    updateIndicator()
    rafId = null
  })
}

// 监听 activeTab 变化
watch(
  () => activeTab?.value,
  () => {
    nextTick(() => {
      scheduleIndicatorUpdate()
    })
  },
  { immediate: true, flush: 'post' }
)

// DOM 初始化/重新挂载时重新计算
watch(
  () => listRef.value,
  (el) => {
    if (el) {
      nextTick(() => {
        scheduleIndicatorUpdate()
      })
    }
  }
)

onMounted(() => {
  // 重置状态
  hasInitialized.value = false
  activeIndex.value = -1
  // 立即尝试更新
  nextTick(() => {
    scheduleIndicatorUpdate()
  })
  window.addEventListener('resize', scheduleIndicatorUpdate)
})

onUnmounted(() => {
  if (rafId !== null) {
    cancelAnimationFrame(rafId)
  }
  window.removeEventListener('resize', scheduleIndicatorUpdate)
})
</script>

<style scoped>
.tabs-list {
  position: relative;
  min-height: 2.5rem;
  align-items: center;
  justify-content: center;
  border-radius: 0.5rem;
  background-color: hsl(var(--muted) / 0.3);
  padding: 0.25rem;
  color: hsl(var(--muted-foreground));
  border: 1px solid hsl(var(--border) / 0.6);
}

.tabs-indicator {
  position: absolute;
  z-index: 0;
  top: 0;
  left: 0;
  border-radius: 0.375rem;
  background: linear-gradient(
    180deg,
    hsl(var(--background)),
    hsl(var(--background) / 0.95)
  );
  border: 1px solid hsl(var(--border));
  box-shadow:
    0 1px 3px 0 rgb(0 0 0 / 0.1),
    0 1px 2px -1px rgb(0 0 0 / 0.1);
  pointer-events: none;
}

@media (prefers-color-scheme: dark) {
  .tabs-indicator {
    background: linear-gradient(
      180deg,
      hsl(var(--accent)),
      hsl(var(--accent) / 0.95)
    );
    border-color: hsl(var(--border) / 0.8);
  }
}
</style>
