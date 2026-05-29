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
          正在重启服务
        </h2>
        <p class="text-sm text-muted-foreground max-w-xs mt-2 mb-2">
          服务正在切换版本并重启，请稍候...
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

        <div
          v-if="loadingUpdatePreflight || updatePreflight || updatePreflightError"
          class="w-full rounded-xl border border-border/60 bg-muted/20 px-4 py-3 text-left"
        >
          <div class="flex items-center justify-between gap-3">
            <div>
              <div class="text-xs font-semibold text-foreground">
                升级前检查
              </div>
              <div class="mt-0.5 text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                Preflight
              </div>
            </div>
            <div class="flex items-center gap-2 text-[11px] text-muted-foreground">
              <span>通过 {{ preflightOkCount }}</span>
              <span v-if="preflightWarningCount">警告 {{ preflightWarningCount }}</span>
              <span v-if="preflightBlockedCount" class="text-destructive">阻塞 {{ preflightBlockedCount }}</span>
            </div>
          </div>

          <div
            v-if="loadingUpdatePreflight"
            class="mt-3 flex items-center gap-2 text-xs text-muted-foreground"
          >
            <Loader2 class="h-4 w-4 animate-spin" />
            正在检查安装目录、磁盘空间和数据库状态...
          </div>

          <div
            v-else-if="updatePreflightError"
            class="mt-3 flex items-start gap-2 rounded-lg border border-amber-500/20 bg-amber-500/10 px-3 py-2 text-amber-600 dark:text-amber-400"
          >
            <AlertTriangle class="mt-0.5 h-4 w-4 shrink-0" />
            <div class="space-y-1 text-xs leading-5">
              <div class="font-medium text-foreground">
                无法完成升级前检查
              </div>
              <p class="text-muted-foreground">
                {{ updatePreflightError }}
              </p>
            </div>
          </div>

          <div
            v-else-if="updatePreflight && updatePreflight.checks.length > 0"
            class="mt-3 space-y-2"
          >
            <div
              v-for="check in updatePreflight.checks"
              :key="check.key"
              class="flex flex-col gap-2 rounded-lg border px-3 py-2 sm:flex-row sm:items-start sm:justify-between"
              :class="preflightStatusClass(check.status)"
            >
              <div class="min-w-0 space-y-1">
                <div class="flex items-center gap-2">
                  <component
                    :is="preflightStatusIcon(check.status)"
                    class="h-4 w-4 shrink-0"
                  />
                  <span class="text-sm font-medium text-foreground">
                    {{ check.label }}
                  </span>
                </div>
                <p class="text-xs leading-5 text-muted-foreground">
                  {{ check.message }}
                </p>
              </div>
              <span class="shrink-0 rounded-full border px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide">
                {{ check.status }}
              </span>
            </div>
          </div>
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
            发布于 {{ formattedPublishedAt }}
          </div>
          <div class="mb-2 text-left text-xs font-medium uppercase tracking-[0.18em] text-muted-foreground/80">
            更新内容
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
          更新包已下载，点击"立即重启"完成安装
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
          class="mt-3 w-full rounded-xl border border-sky-500/20 bg-sky-500/[0.06] px-4 py-3 text-left"
        >
          <div class="flex items-start gap-3">
            <div class="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-sky-500/10 text-sky-600 dark:text-sky-300">
              <Terminal class="h-4 w-4" />
            </div>
            <div class="min-w-0 flex-1 space-y-3">
              <div class="flex flex-col gap-2 sm:flex-row sm:items-start sm:justify-between">
                <div class="min-w-0">
                  <div class="text-sm font-semibold text-foreground">
                    Docker 更新操作
                  </div>
                  <p class="mt-1 text-xs leading-5 text-muted-foreground">
                    需要在宿主机的 compose 目录执行，页面不会直接操作容器。
                  </p>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-7 shrink-0 px-2 text-xs"
                  @click="copyDockerUpdateCommand"
                >
                  <CheckCircle2
                    v-if="dockerCommandCopied"
                    class="mr-1.5 h-3.5 w-3.5 text-emerald-500"
                  />
                  <Copy
                    v-else
                    class="mr-1.5 h-3.5 w-3.5"
                  />
                  {{ dockerCommandCopied ? '已复制' : dockerCommandCopyError ? '复制失败' : '复制命令' }}
                </Button>
              </div>

              <div class="grid gap-2 text-xs sm:grid-cols-3">
                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2">
                  <span class="mr-1.5 font-mono text-sky-600 dark:text-sky-300">1</span>
                  进入 docker-compose.yml 所在目录
                </div>
                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2">
                  <span class="mr-1.5 font-mono text-sky-600 dark:text-sky-300">2</span>
                  执行更新命令并等待镜像拉取
                </div>
                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2">
                  <span class="mr-1.5 font-mono text-sky-600 dark:text-sky-300">3</span>
                  容器健康后刷新管理端
                </div>
              </div>

              <div class="overflow-hidden rounded-lg border border-border/60 bg-background/80">
                <div class="flex items-center gap-2 border-b border-border/60 px-3 py-1.5 text-[11px] text-muted-foreground">
                  <Terminal class="h-3.5 w-3.5" />
                  Shell
                </div>
                <code class="block break-all px-3 py-2 font-mono text-xs leading-5 text-foreground">
                  {{ dockerUpdateCommand }}
                </code>
              </div>

              <p class="text-[11px] leading-5 text-muted-foreground">
                GitHub 检查代理走 AETHER_UPDATE_PROXY_URL；镜像拉取是否走代理取决于 Docker 守护进程配置。
              </p>
            </div>
          </div>
        </div>

        <div class="mt-4 w-full rounded-xl border border-border/60 bg-muted/20 px-4 py-3 text-left">
          <div class="flex items-center justify-between gap-3">
            <div class="flex items-center gap-3">
              <div class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
                <History class="h-4 w-4" />
              </div>
              <div>
                <div class="text-sm font-semibold text-foreground">
                  最近更新记录
                </div>
                <div class="mt-0.5 text-[11px] text-muted-foreground">
                  {{ updateHistorySummaryText }}
                </div>
              </div>
            </div>
            <Button
              variant="ghost"
              size="sm"
              class="h-7 px-2 text-xs"
              :disabled="loadingUpdateHistory"
              @click="loadUpdateHistory(true)"
            >
              <Loader2
                v-if="loadingUpdateHistory"
                class="mr-1.5 h-3.5 w-3.5 animate-spin"
              />
              <RefreshCw
                v-else
                class="mr-1.5 h-3.5 w-3.5"
              />
              刷新
            </Button>
          </div>

          <div
            v-if="loadingUpdateHistory && visibleUpdateHistory.length === 0"
            class="mt-3 flex items-center gap-2 text-xs text-muted-foreground"
          >
            <Loader2 class="h-4 w-4 animate-spin" />
            正在读取更新记录...
          </div>
          <p
            v-else-if="updateHistoryError"
            class="mt-3 flex items-start gap-2 rounded-lg border border-amber-500/20 bg-amber-500/10 px-3 py-2 text-xs text-amber-600 dark:text-amber-400"
          >
            <AlertTriangle class="mt-0.5 h-4 w-4 shrink-0" />
            <span>{{ updateHistoryError }}</span>
          </p>
          <p
            v-else-if="visibleUpdateHistory.length === 0"
            class="mt-3 rounded-lg bg-background/50 px-3 py-3 text-center text-xs text-muted-foreground"
          >
            暂无更新记录。
          </p>
          <div
            v-else
            class="relative mt-3 max-h-64 space-y-3 overflow-y-auto pr-1 before:absolute before:bottom-2 before:left-[15px] before:top-2 before:w-px before:bg-border"
          >
            <div
              v-for="entry in visibleUpdateHistory"
              :key="`${entry.timestamp}-${entry.operation}`"
              class="relative flex gap-3"
            >
              <div
                class="z-10 mt-1 flex h-8 w-8 shrink-0 items-center justify-center rounded-full border bg-background"
                :class="entry.success
                  ? 'border-emerald-500/30 text-emerald-500'
                  : 'border-destructive/30 text-destructive'"
              >
                <component
                  :is="entry.success ? CheckCircle2 : XCircle"
                  class="h-4 w-4"
                />
              </div>
              <div class="min-w-0 flex-1 rounded-lg border border-border/60 bg-background/65 px-3 py-2">
                <div class="flex flex-col gap-1 sm:flex-row sm:items-start sm:justify-between">
                  <div class="min-w-0">
                    <span class="text-sm font-medium text-foreground">
                      {{ updateHistoryOperationLabel(entry.operation) }}
                    </span>
                    <p class="mt-1 flex items-center gap-1.5 text-[11px] text-muted-foreground">
                      <Clock3 class="h-3.5 w-3.5" />
                      {{ formatUpdateHistoryTime(entry.timestamp) }}
                    </p>
                  </div>
                  <div class="flex shrink-0 items-center gap-2">
                    <span
                      class="rounded-full border px-2 py-0.5 text-[10px] font-semibold"
                      :class="entry.success
                        ? 'border-emerald-500/20 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                        : 'border-destructive/20 bg-destructive/10 text-destructive'"
                    >
                      {{ entry.success ? '成功' : '失败' }}
                    </span>
                  </div>
                </div>
                <p
                  v-if="entry.error"
                  class="mt-2 rounded-md bg-destructive/10 px-2 py-1.5 text-xs leading-5 text-destructive"
                >
                  {{ entry.error }}
                </p>
                <pre
                  v-else-if="entry.output_tail"
                  class="mt-2 max-h-24 overflow-y-auto whitespace-pre-wrap rounded-md bg-muted/50 px-2 py-1.5 font-mono text-[11px] leading-5 text-muted-foreground"
                >{{ entry.output_tail }}</pre>
              </div>
            </div>
          </div>
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
          稍后提醒
        </Button>
        <Button
          v-if="rollbackAvailable"
          variant="outline"
          class="flex-1"
          :disabled="updating || rollingBack"
          @click="handleRollback"
        >
          {{ rollingBack ? '回滚中...' : '回滚上一版本' }}
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
          :disabled="updating || rollingBack || !canApplyUpdate || loadingUpdatePreflight || preflightBlocking"
          @click="handleApplyUpdate"
        >
          {{ actionButtonLabel }}
        </Button>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, watch, computed, onMounted } from 'vue'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import { formatDisplayVersion } from '@/utils/version'
import { normalizeReleaseNotesForDisplay } from '@/utils/releaseNotes'
import { sanitizeMarkdown } from '@/utils/sanitize'
import { marked } from 'marked'
import { adminApi, type SystemUpdatePreflightResponse, type UpdateHistoryEntry } from '@/api/admin'
import { isPreflightBlocking } from './updateDialogLogic'
import { AlertTriangle, CheckCircle2, Clock3, Copy, History, Loader2, RefreshCw, Terminal, XCircle } from 'lucide-vue-next'

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
  updatePreflight?: SystemUpdatePreflightResponse | null
  loadingUpdatePreflight?: boolean
  updatePreflightError?: string | null
  downloadProgressText?: string | null
  downloadProgressPercent?: number | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  applyUpdate: []
  rollback: []
}>()

const SOURCE_BUILD_UPDATE_HINT = '当前为源码构建，请使用 git pull 后重新编译。'
const UPDATE_HISTORY_LIMIT = 5

const isOpen = ref(props.modelValue)
const updateHistory = ref<UpdateHistoryEntry[]>([])
const loadingUpdateHistory = ref(false)
const updateHistoryError = ref<string | null>(null)
const dockerCommandCopied = ref(false)
const dockerCommandCopyError = ref(false)
const updating = computed(() => props.updating ?? false)
const updatePhase = computed(() => props.updatePhase ?? 'download')
const updateSupported = computed(() => props.updateSupported ?? true)
const updatable = computed(() => props.updatable ?? true)
const canApplyUpdate = computed(() => updateSupported.value && updatable.value)
const updatePreflight = computed(() => props.updatePreflight ?? null)
const loadingUpdatePreflight = computed(() => props.loadingUpdatePreflight ?? false)
const updatePreflightError = computed(() => props.updatePreflightError ?? null)
const updateStrategy = computed(() => props.updateStrategy ?? 'manual')
const isDockerUpdate = computed(() => updateStrategy.value === 'docker' && !canApplyUpdate.value)
const dockerUpdateCommand = computed(() => props.dockerUpdateCommand || '')
const updateBlockerText = computed(() => {
  if (!updateSupported.value) return props.updateBlocker || SOURCE_BUILD_UPDATE_HINT
  return props.updateBlocker || '当前版本暂不支持在线更新'
})
const reconnectMessage = computed(() => props.reconnectMessage ?? '等待服务恢复...')
const rollbackAvailable = computed(() => props.rollbackAvailable ?? false)
const rollingBack = computed(() => props.rollingBack ?? false)
const downloadProgressText = computed(() => props.downloadProgressText || '正在下载更新包...')
const dialogTitleText = computed(() => props.dialogTitle ?? '发现新版本')
const versionLabelText = computed(() => props.versionLabel ?? '最新版本')
const releaseLinkLabelText = computed(() => props.releaseLinkLabel ?? '查看发布')
const fallbackDescriptionText = computed(() => {
  if (!canApplyUpdate.value) return updateBlockerText.value
  return '新版本已发布，建议更新以获得最新功能和安全修复'
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
    return updatePhase.value === 'restart' ? '重启中...' : '下载中...'
  }
  return updatePhase.value === 'restart' ? '立即重启' : '立即更新'
})

watch(() => props.modelValue, (val) => {
  isOpen.value = val
})

watch(isOpen, (val) => {
  emit('update:modelValue', val)
  if (val) {
    void loadUpdateHistory(true)
  }
})

onMounted(() => {
  if (isOpen.value) {
    void loadUpdateHistory()
  }
})

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

const preflightOkCount = computed(() => {
  return updatePreflight.value?.checks.filter(item => item.status === 'ok').length ?? 0
})
const preflightWarningCount = computed(() => {
  return updatePreflight.value?.checks.filter(item => item.status === 'warning').length ?? 0
})
const preflightBlockedCount = computed(() => {
  return updatePreflight.value?.checks.filter(item => item.status === 'blocked').length ?? 0
})
const preflightBlocking = computed(() => isPreflightBlocking(updatePreflight.value))
const visibleUpdateHistory = computed(() => {
  return [...updateHistory.value].reverse().slice(0, UPDATE_HISTORY_LIMIT)
})
const updateHistorySuccessCount = computed(() => {
  return updateHistory.value.filter(entry => entry.success).length
})
const updateHistoryFailureCount = computed(() => {
  return updateHistory.value.filter(entry => !entry.success).length
})
const historyDisplayCountText = computed(() => {
  const total = updateHistory.value.length
  if (total === 0) return ''
  return `最近 ${Math.min(total, UPDATE_HISTORY_LIMIT)} / ${total} 条`
})
const updateHistorySummaryText = computed(() => {
  const total = updateHistory.value.length
  if (loadingUpdateHistory.value && total === 0) return '正在读取历史记录'
  if (total === 0) return '暂无历史记录'
  const failed = updateHistoryFailureCount.value
  return failed > 0
    ? `${historyDisplayCountText.value} · 成功 ${updateHistorySuccessCount.value} · 失败 ${failed}`
    : `${historyDisplayCountText.value} · 全部成功`
})

function preflightStatusClass(status: 'ok' | 'warning' | 'blocked'): string {
  if (status === 'ok') return 'border-emerald-500/20 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
  if (status === 'warning') return 'border-amber-500/20 bg-amber-500/10 text-amber-600 dark:text-amber-400'
  return 'border-destructive/20 bg-destructive/10 text-destructive'
}

function preflightStatusIcon(status: 'ok' | 'warning' | 'blocked') {
  if (status === 'ok') return CheckCircle2
  if (status === 'warning') return AlertTriangle
  return XCircle
}

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
  if (loadingUpdatePreflight.value || preflightBlocking.value) return
  emit('applyUpdate')
}

function handleRollback() {
  emit('rollback')
}

async function copyDockerUpdateCommand() {
  dockerCommandCopied.value = false
  dockerCommandCopyError.value = false
  try {
    await navigator.clipboard.writeText(dockerUpdateCommand.value)
    dockerCommandCopied.value = true
    window.setTimeout(() => {
      dockerCommandCopied.value = false
    }, 1600)
  } catch {
    dockerCommandCopyError.value = true
    window.setTimeout(() => {
      dockerCommandCopyError.value = false
    }, 1600)
  }
}

async function loadUpdateHistory(force = false) {
  if (loadingUpdateHistory.value) return
  if (!force && updateHistory.value.length > 0) return

  loadingUpdateHistory.value = true
  updateHistoryError.value = null
  try {
    const response = await adminApi.getUpdateHistory()
    updateHistory.value = response.entries
  } catch (error) {
    updateHistoryError.value = error instanceof Error
      ? error.message
      : '读取更新记录失败'
  } finally {
    loadingUpdateHistory.value = false
  }
}

function updateHistoryOperationLabel(operation: string): string {
  switch (operation) {
    case 'prepare':
      return '准备更新'
    case 'apply':
      return '应用更新'
    case 'rollback':
      return '回滚版本'
    default:
      return operation || '更新任务'
  }
}

function formatUpdateHistoryTime(value: string): string {
  try {
    const date = new Date(value)
    if (Number.isNaN(date.getTime())) return value
    return date.toLocaleString('zh-CN', {
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
    })
  } catch {
    return value
  }
}
</script>
