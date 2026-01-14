<template>
  <Dialog
    v-model="isOpen"
    size="md"
    title=""
  >
    <div class="flex flex-col items-center text-center py-2">
      <!-- Logo -->
      <HeaderLogo
        size="h-16 w-16"
        class-name="text-primary"
      />

      <!-- Title -->
      <h2 class="text-xl font-semibold text-foreground mt-4 mb-2">
        发现新版本
      </h2>

      <!-- Version Info -->
      <div class="flex items-center gap-3 mb-2">
        <span class="px-3 py-1.5 rounded-lg bg-muted text-sm font-mono text-muted-foreground">
          v{{ currentVersion }}
        </span>
        <svg
          class="h-4 w-4 text-muted-foreground"
          fill="none"
          stroke="currentColor"
          viewBox="0 0 24 24"
        >
          <path
            stroke-linecap="round"
            stroke-linejoin="round"
            stroke-width="2"
            d="M13 7l5 5m0 0l-5 5m5-5H6"
          />
        </svg>
        <span class="px-3 py-1.5 rounded-lg bg-primary/10 text-sm font-mono font-medium text-primary">
          v{{ latestVersion }}
        </span>
      </div>

      <!-- Published At -->
      <p
        v-if="publishedAt"
        class="text-xs text-muted-foreground mb-4"
      >
        发布于 {{ formattedPublishedAt }}
      </p>

      <!-- Release Notes -->
      <div
        v-if="releaseNotes"
        class="w-full mt-2 mb-4"
      >
        <div class="text-left text-xs font-medium text-muted-foreground mb-2">
          更新内容
        </div>
        <div
          class="w-full max-h-48 overflow-y-auto rounded-lg bg-muted/50 p-3 text-left text-sm text-foreground/80 prose prose-sm dark:prose-invert prose-p:my-1 prose-ul:my-1 prose-li:my-0"
          v-html="renderedReleaseNotes"
        />
      </div>

      <!-- Description (fallback when no release notes) -->
      <p
        v-else
        class="text-sm text-muted-foreground max-w-xs mb-4"
      >
        新版本已发布，建议更新以获得最新功能和安全修复
      </p>
    </div>

    <template #footer>
      <div class="flex w-full gap-3">
        <Button
          variant="outline"
          class="flex-1"
          @click="handleLater"
        >
          稍后提醒
        </Button>
        <Button
          class="flex-1"
          @click="handleViewRelease"
        >
          查看更新
        </Button>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, watch, computed } from 'vue'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import { marked } from 'marked'
import DOMPurify from 'dompurify'

const props = defineProps<{
  modelValue: boolean
  currentVersion: string
  latestVersion: string
  releaseUrl: string | null
  releaseNotes: string | null
  publishedAt: string | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
}>()

const isOpen = ref(props.modelValue)

watch(() => props.modelValue, (val) => {
  isOpen.value = val
})

watch(isOpen, (val) => {
  emit('update:modelValue', val)
})

// 格式化发布时间
const formattedPublishedAt = computed(() => {
  if (!props.publishedAt) return ''
  try {
    const date = new Date(props.publishedAt)
    return date.toLocaleDateString('zh-CN', {
      year: 'numeric',
      month: 'long',
      day: 'numeric'
    })
  } catch {
    return props.publishedAt
  }
})

// 渲染 Markdown 格式的 Release Notes（使用 DOMPurify 防止 XSS）
const renderedReleaseNotes = computed(() => {
  if (!props.releaseNotes) return ''
  try {
    const html = marked.parse(props.releaseNotes, { async: false }) as string
    return DOMPurify.sanitize(html)
  } catch {
    // 如果 markdown 解析失败，返回原始文本（转义 HTML）
    return props.releaseNotes
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/\n/g, '<br>')
  }
})

function handleLater() {
  // 记录忽略的版本，24小时内不再提醒
  const ignoreKey = 'aether_update_ignore'
  const ignoreData = {
    version: props.latestVersion,
    until: Date.now() + 24 * 60 * 60 * 1000 // 24小时
  }
  localStorage.setItem(ignoreKey, JSON.stringify(ignoreData))
  isOpen.value = false
}

function handleViewRelease() {
  if (props.releaseUrl) {
    window.open(props.releaseUrl, '_blank')
  }
  isOpen.value = false
}
</script>
