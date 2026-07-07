<template>
  <Dialog
    v-model="isOpen"
    size="lg"
    title=""
  >
    <div class="flex flex-col items-center text-center py-2">
      <!-- Logo -->
      <HeaderLogo
        size="h-16 w-16"
        class-name="text-primary"
      />

      <!-- Reconnecting State -->
      <template v-if="updatePhase === 'reconnecting'">
        <h2 class="text-xl font-semibold text-foreground mt-4 mb-2">
          {{ legacyT('正在重启服务') }}
        </h2>
        <p class="text-sm text-muted-foreground max-w-xs mt-2 mb-2">
          {{ legacyT('服务正在切换版本并重启，请稍候...') }}
        </p>
        <div class="flex items-center gap-2 text-primary mt-2 mb-4">
          <svg
            class="animate-spin h-5 w-5"
            xmlns="http://www.w3.org/2000/svg"
            fill="none"
            viewBox="0 0 24 24"
          >
            <circle
              class="opacity-25"
              cx="12"
              cy="12"
              r="10"
              stroke="currentColor"
              stroke-width="4"
            />
            <path
              class="opacity-75"
              fill="currentColor"
              d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
            />
          </svg>
          <span class="text-sm font-medium">
            {{ reconnectMessage }}
          </span>
        </div>
      </template>

      <!-- Normal Update State -->
      <template v-else>
        <h2 class="text-xl font-semibold text-foreground mt-4 mb-2">
          {{ dialogTitleText }}
        </h2>

        <!-- Version Info -->
        <div class="mx-auto mb-2 w-full max-w-sm rounded-lg bg-muted/20 px-4 py-3 text-center">
          <p class="text-xs text-muted-foreground">
            {{ versionLabelText }}
          </p>
          <p class="mt-1 break-all font-mono text-base font-semibold text-primary">
            {{ formatDisplayVersion(latestVersion) }}
          </p>
        </div>

        <!-- Release Notes -->
        <div
          v-if="displayReleaseNotes"
          class="w-full mt-3 mb-4"
        >
          <div
            v-if="publishedAt"
            class="mb-2 text-left text-xs text-muted-foreground"
          >
            {{ legacyT('发布于') }} {{ formattedPublishedAt }}
          </div>
          <div class="mb-2 text-left text-xs font-medium uppercase tracking-[0.18em] text-muted-foreground/80">
            {{ legacyT('更新内容') }}
          </div>
          <!-- eslint-disable vue/no-v-html -->
          <div
            class="max-h-64 w-full overflow-y-auto rounded-xl border border-border/60 bg-muted/25 px-4 py-3 text-left text-sm leading-6 text-foreground/90 shadow-inner shadow-black/[0.02] max-w-none prose prose-sm dark:prose-invert prose-headings:mb-2 prose-headings:mt-4 prose-headings:font-semibold prose-headings:text-foreground prose-h3:text-sm prose-p:my-2 prose-ul:my-2 prose-ul:list-disc prose-ul:pl-5 prose-li:my-1 prose-li:marker:text-primary prose-a:text-primary prose-strong:text-foreground prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5"
            v-html="renderedReleaseNotes"
          />
          <!-- eslint-enable vue/no-v-html -->
        </div>

        <!-- Description (fallback when no release notes) -->
        <p
          v-else
          class="text-sm text-muted-foreground max-w-xs mt-2 mb-4"
        >
          {{ fallbackDescriptionText }}
        </p>

        <p
          v-if="updatePhase === 'restart'"
          class="mt-1 text-xs text-primary"
        >
          {{ legacyT('更新包已下载，点击"立即重启"完成安装') }}
        </p>

        <div
          v-if="updating && updatePhase === 'download'"
          class="mt-3 w-full max-w-sm"
        >
          <div class="mb-1.5 flex items-center justify-between gap-3 text-xs text-muted-foreground">
            <span class="truncate">{{ downloadProgressText }}</span>
            <span
              v-if="downloadProgressPercent !== null"
              class="shrink-0 font-mono text-primary"
            >
              {{ downloadProgressPercent }}%
            </span>
          </div>
          <div class="h-1.5 w-full overflow-hidden rounded-full bg-muted">
            <div
              class="h-full rounded-full bg-primary transition-all duration-300"
              :style="{ width: progressBarWidth }"
            />
          </div>
        </div>

        <!-- Source Build Hint -->
        <p
          v-if="!canApplyUpdate"
          class="mt-1 text-xs text-muted-foreground"
        >
          {{ updateBlockerText }}
        </p>
        <div
          v-if="isDockerUpdate && dockerUpdateCommand"
          class="mt-3 w-full max-w-sm rounded-lg border border-border/60 bg-muted/30 px-3 py-2 text-left"
        >
          <p class="text-xs text-muted-foreground">
            {{ legacyT('在 docker-compose.yml 所在目录执行') }}
          </p>
          <code class="mt-1 block break-all rounded bg-background/70 px-2 py-1.5 font-mono text-xs text-foreground">
            {{ dockerUpdateCommand }}
          </code>
        </div>
      </template>
    </div>

    <template #footer>
      <div
        v-if="updatePhase !== 'reconnecting'"
        class="flex w-full gap-3"
      >
        <Button
          variant="outline"
          class="flex-1"
          :disabled="updating || rollingBack"
          @click="handleLater"
        >
          {{ legacyT('稍后提醒') }}
        </Button>
        <Button
          v-if="rollbackAvailable"
          variant="outline"
          class="flex-1"
          :disabled="updating || rollingBack"
          @click="handleRollback"
        >
          {{ rollingBack ? legacyT('回滚中...') : legacyT('回滚上一版本') }}
        </Button>
        <Button
          v-else
          variant="outline"
          class="flex-1"
          :disabled="updating || rollingBack"
          @click="handleViewRelease"
        >
          {{ releaseLinkLabelText }}
        </Button>
        <Button
          v-if="updateSupported"
          class="flex-1"
          :disabled="updating || rollingBack || !canApplyUpdate"
          @click="handleApplyUpdate"
        >
          {{ actionButtonLabel }}
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
import { formatDisplayVersion } from '@/utils/version'
import { normalizeReleaseNotesForDisplay } from '@/utils/releaseNotes'
import { sanitizeMarkdown } from '@/utils/sanitize'
import { marked } from 'marked'
import { useI18n } from '@/i18n'

const props = defineProps<{
  modelValue: boolean
  currentVersion: string
  latestVersion: string
  releaseUrl: string | null
  releaseNotes: string | null
  publishedAt: string | null
  dialogTitle?: string
  versionLabel?: string
  releaseLinkLabel?: string
  updatePhase?: 'download' | 'restart' | 'reconnecting'
  updating?: boolean
  updateSupported?: boolean
  updateStrategy?: string
  updatable?: boolean
  updateBlocker?: string | null
  dockerUpdateCommand?: string | null
  reconnectMessage?: string
  rollbackAvailable?: boolean
  rollingBack?: boolean
  downloadProgressText?: string | null
  downloadProgressPercent?: number | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  applyUpdate: []
  rollback: []
}>()
const { legacyT, locale } = useI18n()

const SOURCE_BUILD_UPDATE_HINT = '当前为源码构建，请使用 git pull 后重新编译。'

const isOpen = ref(props.modelValue)
const updating = computed(() => props.updating ?? false)
const updatePhase = computed(() => props.updatePhase ?? 'download')
const updateSupported = computed(() => props.updateSupported ?? true)
const updatable = computed(() => props.updatable ?? true)
const canApplyUpdate = computed(() => updateSupported.value && updatable.value)
const updateStrategy = computed(() => props.updateStrategy ?? 'manual')
const isDockerUpdate = computed(() => updateStrategy.value === 'docker' && !canApplyUpdate.value)
const dockerUpdateCommand = computed(() => props.dockerUpdateCommand || '')
const updateBlockerText = computed(() => {
  if (!updateSupported.value) return legacyT(props.updateBlocker || SOURCE_BUILD_UPDATE_HINT)
  return legacyT(props.updateBlocker || '当前版本暂不支持在线更新')
})
const reconnectMessage = computed(() => legacyT(props.reconnectMessage ?? '等待服务恢复...'))
const rollbackAvailable = computed(() => props.rollbackAvailable ?? false)
const rollingBack = computed(() => props.rollingBack ?? false)
const downloadProgressText = computed(() => legacyT(props.downloadProgressText || '正在下载更新包...'))
const dialogTitleText = computed(() => legacyT(props.dialogTitle ?? '发现新版本'))
const versionLabelText = computed(() => legacyT(props.versionLabel ?? '最新版本'))
const releaseLinkLabelText = computed(() => legacyT(props.releaseLinkLabel ?? '查看发布'))
const fallbackDescriptionText = computed(() => {
  if (!canApplyUpdate.value) return updateBlockerText.value
  return legacyT('新版本已发布，建议更新以获得最新功能和安全修复')
})
const downloadProgressPercent = computed(() => {
  const value = props.downloadProgressPercent
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.max(0, Math.min(100, Math.round(value)))
    : null
})
const progressBarWidth = computed(() => {
  return downloadProgressPercent.value === null ? '35%' : `${downloadProgressPercent.value}%`
})
const actionButtonLabel = computed(() => {
  if (updating.value) {
    return legacyT(updatePhase.value === 'restart' ? '重启中...' : '下载中...')
  }
  return legacyT(updatePhase.value === 'restart' ? '立即重启' : '立即更新')
})

watch(() => props.modelValue, (val) => {
  isOpen.value = val
})

watch(isOpen, (val) => {
  emit('update:modelValue', val)
})

const formattedPublishedAt = computed(() => {
  if (!props.publishedAt) return ''
  try {
    const date = new Date(props.publishedAt)
    return date.toLocaleDateString(locale.value, {
      year: 'numeric',
      month: 'long',
      day: 'numeric'
    })
  } catch {
    return props.publishedAt
  }
})

const displayReleaseNotes = computed(() => {
  return normalizeReleaseNotesForDisplay(props.releaseNotes)
})

const renderedReleaseNotes = computed(() => {
  if (!displayReleaseNotes.value) return ''
  try {
    const html = marked.parse(displayReleaseNotes.value, {
      async: false,
      breaks: true
    }) as string
    return sanitizeMarkdown(html)
  } catch {
    return displayReleaseNotes.value
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/\n/g, '<br>')
  }
})

function handleLater() {
  const ignoreKey = 'aether_update_ignore'
  const ignoreData = {
    version: props.latestVersion,
    until: Date.now() + 24 * 60 * 60 * 1000
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

function handleApplyUpdate() {
  if (!canApplyUpdate.value) return
  emit('applyUpdate')
}

function handleRollback() {
  emit('rollback')
}
</script>
