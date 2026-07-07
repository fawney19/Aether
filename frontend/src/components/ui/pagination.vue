<template>
  <div class="flex flex-col sm:flex-row gap-3 sm:gap-4 border-t border-border/60 px-4 sm:px-6 py-3 sm:py-4 bg-muted/20">
    <!-- 左侧：记录范围和每页数量 -->
    <div class="flex items-center justify-between sm:justify-start gap-3 text-sm text-muted-foreground">
      <span class="font-medium whitespace-nowrap">
        {{ rangeSummary }}
      </span>
      <Select
        v-if="showPageSizeSelector"
        :model-value="String(pageSize)"
        @update:model-value="handlePageSizeChange"
      >
        <SelectTrigger class="w-[120px] h-8 sm:h-9 border-border/60 text-xs sm:text-sm">
          <span class="flex-1 text-center">
            <SelectValue />
          </span>
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="size in pageSizeOptions"
            :key="size"
            :value="String(size)"
          >
            {{ pageSizeLabel(size) }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <!-- 右侧：分页按钮 -->
    <div class="flex flex-wrap items-center justify-center gap-1.5 sm:gap-2 sm:ml-auto">
      <!-- 页码按钮（智能省略） -->
      <template
        v-for="page in pageNumbers"
        :key="page"
      >
        <Button
          v-if="typeof page === 'number'"
          :variant="page === current ? 'default' : 'outline'"
          size="sm"
          class="h-9 min-w-[36px] px-2"
          :class="page === current ? 'shadow-sm' : ''"
          @click="handlePageChange(page)"
        >
          {{ page }}
        </Button>
        <span
          v-else
          class="px-2 text-muted-foreground select-none"
        >{{ page }}</span>
      </template>

      <!-- 页码跳转 -->
      <div
        v-if="totalPages > 7"
        class="flex items-center gap-1.5 ml-2 text-sm text-muted-foreground"
      >
        <span class="hidden sm:inline">{{ jumpToLabel }}</span>
        <input
          v-model="jumpPageInput"
          type="text"
          inputmode="numeric"
          pattern="[0-9]*"
          class="w-12 h-9 px-2 text-center text-sm border border-border/60 rounded-md bg-background focus:outline-none focus:ring-2 focus:ring-primary/40 focus:border-primary/60"
          @keydown.enter="handleJumpPage"
          @blur="handleJumpPage"
          @input="filterNumericInput"
        >
        <span class="hidden sm:inline">{{ pageLabel }}</span>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, onMounted } from 'vue'
import { Button, Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui'
import { useI18n } from '@/i18n'

interface Props {
  current: number
  total: number
  pageSize?: number
  pageSizeOptions?: number[]
  showPageSizeSelector?: boolean
  /** 缓存键名，设置后会将 pageSize 缓存到 localStorage 并在组件挂载时自动恢复 */
  cacheKey?: string
}

interface Emits {
  (e: 'update:current', value: number): void
  (e: 'update:pageSize', value: number): void
}

const props = withDefaults(defineProps<Props>(), {
  pageSize: 20,
  pageSizeOptions: () => [10, 20, 50, 100],
  showPageSizeSelector: true,
  cacheKey: undefined
})

const emit = defineEmits<Emits>()

const jumpPageInput = ref('')
const locale = useI18n().locale

const totalPages = computed(() => Math.ceil(props.total / props.pageSize))

const recordRange = computed(() => {
  const start = (props.current - 1) * props.pageSize + 1
  const end = Math.min(props.current * props.pageSize, props.total)
  return { start, end }
})

const rangeSummary = computed(() => {
  if (locale.value === 'en-US') {
    return `Showing ${recordRange.value.start}-${recordRange.value.end} of ${props.total} items`
  }
  return `显示 ${recordRange.value.start}-${recordRange.value.end} 条，共 ${props.total} 条`
})

const jumpToLabel = computed(() => locale.value === 'en-US' ? 'Go to' : '跳至')
const pageLabel = computed(() => locale.value === 'en-US' ? 'page' : '页')

function pageSizeLabel(size: number): string {
  return locale.value === 'en-US' ? `${size} / page` : `${size} 条/页`
}

const pageNumbers = computed(() => {
  const pages: (number | string)[] = []
  const total = totalPages.value
  const current = props.current

  if (total <= 7) {
    // 总页数 <= 7，全部显示
    for (let i = 1; i <= total; i++) {
      pages.push(i)
    }
  } else {
    // 总页数 > 7，智能省略
    if (current <= 3) {
      // 当前页在前 3 页：[1, 2, 3, 4, 5, ..., total]
      for (let i = 1; i <= 5; i++) pages.push(i)
      pages.push('...')
      pages.push(total)
    } else if (current >= total - 2) {
      // 当前页在后 3 页：[1, ..., total-4, total-3, total-2, total-1, total]
      pages.push(1)
      pages.push('...')
      for (let i = total - 4; i <= total; i++) pages.push(i)
    } else {
      // 当前页在中间：[1, ..., current-1, current, current+1, ..., total]
      pages.push(1)
      pages.push('...')
      for (let i = current - 1; i <= current + 1; i++) pages.push(i)
      pages.push('...')
      pages.push(total)
    }
  }

  return pages
})

function handlePageChange(page: number) {
  if (page < 1 || page > totalPages.value || page === props.current) {
    return
  }
  emit('update:current', page)
}

function handlePageSizeChange(value: string) {
  const newSize = parseInt(value)
  if (newSize !== props.pageSize) {
    // 缓存到 localStorage
    if (props.cacheKey) {
      localStorage.setItem(props.cacheKey, value)
    }
    emit('update:pageSize', newSize)
    // 切换每页数量时，重置到第一页
    emit('update:current', 1)
  }
}

// 组件挂载时自动从缓存恢复 pageSize（仅当显示选择器时）
onMounted(() => {
  if (props.cacheKey && props.showPageSizeSelector) {
    const cached = localStorage.getItem(props.cacheKey)
    if (cached) {
      const cachedSize = parseInt(cached, 10)
      if (!isNaN(cachedSize) && props.pageSizeOptions?.includes(cachedSize)) {
        if (cachedSize !== props.pageSize) {
          emit('update:pageSize', cachedSize)
        }
      }
    }
  }
})

function handleJumpPage() {
  const page = parseInt(jumpPageInput.value)
  if (!isNaN(page) && page >= 1 && page <= totalPages.value && page !== props.current) {
    emit('update:current', page)
  }
  jumpPageInput.value = ''
}

function filterNumericInput(event: Event) {
  const input = event.target as HTMLInputElement
  input.value = input.value.replace(/[^0-9]/g, '')
  jumpPageInput.value = input.value
}
</script>
