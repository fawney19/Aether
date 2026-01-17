<template>
  <div class="flex flex-col sm:flex-row gap-4 border-t border-border/60 px-6 py-4 bg-muted/20">
    <!-- 左侧：记录范围和每页数量 -->
    <div class="flex flex-col sm:flex-row items-start sm:items-center gap-3 text-sm text-muted-foreground">
      <span class="font-medium">
        显示 <span class="text-foreground font-semibold">{{ recordRange.start }}-{{ recordRange.end }}</span> 条，共 <span class="text-foreground font-semibold">{{ total }}</span> 条
      </span>
      <Select
        v-if="showPageSizeSelector"
        v-model:open="pageSizeSelectOpen"
        :model-value="String(pageSize)"
        @update:model-value="handlePageSizeChange"
      >
        <SelectTrigger class="w-36 h-9 border-border/60">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="size in pageSizeOptions"
            :key="size"
            :value="String(size)"
          >
            {{ size }} 条/页
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <!-- 右侧：分页按钮 -->
    <div class="flex flex-wrap items-center gap-2 sm:ml-auto">
      <Button
        variant="outline"
        size="sm"
        class="h-9 px-3"
        :disabled="current === 1"
        @click="handlePageChange(1)"
      >
        首页
      </Button>
      <Button
        variant="outline"
        size="sm"
        class="h-9 px-3"
        :disabled="current === 1"
        @click="handlePageChange(current - 1)"
      >
        上一页
      </Button>

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

      <Button
        variant="outline"
        size="sm"
        class="h-9 px-3"
        :disabled="current === totalPages"
        @click="handlePageChange(current + 1)"
      >
        下一页
      </Button>
      <Button
        variant="outline"
        size="sm"
        class="h-9 px-3"
        :disabled="current === totalPages"
        @click="handlePageChange(totalPages)"
      >
        末页
      </Button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { Button, Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui'

interface Props {
  current: number
  total: number
  pageSize?: number
  pageSizeOptions?: number[]
  showPageSizeSelector?: boolean
  /** 缓存键名，设置后会将 pageSize 缓存到 localStorage */
  cacheKey?: string
}

interface Emits {
  (e: 'update:current', value: number): void
  (e: 'update:pageSize', value: number): void
}

const props = withDefaults(defineProps<Props>(), {
  pageSize: 20,
  pageSizeOptions: () => [10, 20, 50, 100],
  showPageSizeSelector: true
})

const emit = defineEmits<Emits>()

const pageSizeSelectOpen = ref(false)

const totalPages = computed(() => Math.ceil(props.total / props.pageSize))

const recordRange = computed(() => {
  const start = (props.current - 1) * props.pageSize + 1
  const end = Math.min(props.current * props.pageSize, props.total)
  return { start, end }
})

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

</script>
