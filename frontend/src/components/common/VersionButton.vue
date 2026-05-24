<template>
  <Popover v-model:open="isOpen">
    <PopoverTrigger as-child>
      <button
        type="button"
        class="flex h-9 w-9 items-center justify-center rounded-lg transition"
        :class="buttonClass"
        :title="buttonTitle"
        aria-label="版本信息"
      >
        <Info
          v-if="!isReconnecting"
          class="h-4 w-4"
          :class="loading ? 'animate-pulse' : ''"
        />
        <RefreshCw
          v-else
          class="h-4 w-4 animate-spin"
        />
      </button>
    </PopoverTrigger>

    <PopoverContent
      align="end"
      side="bottom"
      :side-offset="8"
      class="w-[22rem] max-w-[calc(100vw-1rem)] overflow-hidden rounded-xl border-border/60 bg-card/95 p-0 text-card-foreground shadow-xl shadow-black/5 backdrop-blur supports-[backdrop-filter]:bg-card/90"
    >
      <div class="text-left">
        <div class="flex items-center justify-between gap-3 border-b border-border/60 bg-muted/30 px-3 py-2.5">
          <div>
            <div class="text-xs font-semibold text-foreground">
              版本信息
            </div>
            <div class="mt-0.5 text-[10px] uppercase tracking-[0.3em] text-muted-foreground">
              System version
            </div>
          </div>
          <span
            class="rounded-full border px-2 py-0.5 text-[10px] font-semibold"
            :class="statusPillClass"
          >
            {{ statusLabel }}
          </span>
        </div>

        <div class="space-y-3 px-3 py-3">
          <div class="rounded-lg border border-border/60 bg-muted/20 px-3 py-2.5">
            <div>
              <p class="text-xs text-muted-foreground">
                当前版本
              </p>
              <p class="mt-1 break-all font-mono text-sm text-foreground">
                {{ currentVersionLabel }}
              </p>
            </div>
            <div
              v-if="latestVersionLabel"
              class="mt-2"
            >
              <p class="text-xs text-muted-foreground">
                最新版本
              </p>
              <p class="mt-1 break-all font-mono text-sm text-foreground">
                {{ latestVersionLabel }}
              </p>
            </div>
          </div>

          <p
            v-if="status?.error"
            class="text-xs text-muted-foreground"
          >
            检查更新失败：{{ status.error }}
          </p>

          <p
            v-if="status?.has_update && !canApplyUpdate"
            class="text-xs text-muted-foreground"
          >
            {{ updateBlockerText }}
          </p>

          <!-- Reconnecting / busy banner -->
          <div
            v-if="isReconnecting"
            class="flex items-center justify-center gap-2 rounded-lg border border-primary/20 bg-primary/5 px-3 py-2 text-primary"
          >
            <RefreshCw class="h-3.5 w-3.5 animate-spin" />
            <span class="text-xs font-medium">服务重启中，请稍候...</span>
          </div>

          <div
            v-else-if="isDownloadingUpdate"
            class="rounded-lg border border-primary/20 bg-primary/5 px-3 py-2"
          >
            <div class="flex items-center justify-between gap-3 text-xs text-primary">
              <span class="truncate">{{ downloadProgressText }}</span>
              <span
                v-if="downloadProgressPercent !== null"
                class="shrink-0 font-mono"
              >
                {{ downloadProgressPercent }}%
              </span>
            </div>
            <div class="mt-2 h-1.5 overflow-hidden rounded-full bg-primary/15">
              <div
                class="h-full rounded-full bg-primary transition-all duration-300"
                :style="{ width: progressBarWidth }"
              />
            </div>
          </div>

          <div
            v-else
            class="flex items-center gap-2"
          >
            <Button
              variant="outline"
              size="sm"
              class="flex-1"
              :disabled="isBusy || loading"
              @click="handleRefresh"
            >
              <RefreshCw
                class="mr-2 h-3.5 w-3.5"
                :class="loading ? 'animate-spin' : ''"
              />
              重新检查
            </Button>
            <Button
              v-if="rollbackAvailable"
              variant="outline"
              size="sm"
              class="flex-1"
              :disabled="isBusy"
              @click="handleRollback"
            >
              {{ rollingBack ? '回滚中...' : '回滚' }}
            </Button>
            <Button
              v-if="status?.has_update && status.release_url && !rollbackAvailable"
              size="sm"
              class="flex-1"
              @click="handleOpenRelease"
            >
              <ExternalLink class="mr-2 h-3.5 w-3.5" />
              {{ releaseButtonLabel }}
            </Button>
            <Button
              v-if="status?.has_update && canApplyUpdate"
              size="sm"
              class="flex-1"
              :disabled="isBusy"
              @click="handleApplyUpdate"
            >
              <RefreshCw
                class="mr-2 h-3.5 w-3.5"
                :class="updating ? 'animate-spin' : ''"
              />
              {{ actionButtonLabel }}
            </Button>
          </div>

          <!-- Releases List -->
          <div v-if="!isReconnecting">
            <button
              type="button"
              class="flex w-full items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition"
              @click="toggleReleases"
            >
              <ChevronRight
                class="h-3 w-3 transition-transform"
                :class="showReleases ? 'rotate-90' : ''"
              />
              历史版本
              <span
                v-if="releases.length > 0"
                class="text-[10px] text-muted-foreground/60"
              >({{ releases.length }})</span>
              <RefreshCw
                v-if="loadingReleases"
                class="ml-auto h-3 w-3 animate-spin text-muted-foreground"
              />
            </button>
            <div
              v-if="showReleases"
              class="mt-2 max-h-48 space-y-1 overflow-y-auto"
            >
              <p
                v-if="releasesError"
                class="text-xs text-muted-foreground"
              >
                {{ releasesError }}
              </p>
              <button
                v-for="release in releases"
                :key="release.version"
                type="button"
                class="flex w-full items-center justify-between rounded-md border border-border/40 px-2.5 py-1.5 text-left text-xs transition hover:border-primary/30 hover:bg-primary/5"
                :class="release.is_current ? 'bg-primary/5 border-primary/20' : 'bg-muted/10'"
                @click="openReleaseDetails(release)"
              >
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-1.5">
                    <span class="break-all font-mono font-medium text-foreground">
                      {{ formatDisplayVersion(release.version) }}
                    </span>
                    <span
                      v-if="release.is_current"
                      class="shrink-0 rounded-full bg-primary/10 px-1.5 py-0.5 text-[10px] font-medium text-primary"
                    >当前</span>
                    <span
                      v-else-if="release.is_newer"
                      class="shrink-0 rounded-full bg-emerald-500/10 px-1.5 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-400"
                    >新</span>
                    <span
                      v-if="release.is_newer && release.updatable === false"
                      class="shrink-0 rounded-full bg-amber-500/10 px-1.5 py-0.5 text-[10px] font-medium text-amber-600 dark:text-amber-400"
                    >不可在线更新</span>
                  </div>
                  <div
                    v-if="release.published_at"
                    class="mt-0.5 text-[10px] text-muted-foreground"
                  >
                    {{ formatDate(release.published_at) }}
                  </div>
                  <div
                    v-if="release.update_blocker"
                    class="mt-0.5 text-[10px] text-muted-foreground"
                  >
                    {{ release.update_blocker }}
                  </div>
                </div>
                <span class="ml-2 shrink-0 text-[10px] font-medium text-muted-foreground">
                  详情
                </span>
              </button>
              <p
                v-if="!loadingReleases && releases.length === 0 && !releasesError"
                class="py-2 text-center text-xs text-muted-foreground"
              >
                暂无版本信息
              </p>
            </div>
          </div>
        </div>
      </div>
    </PopoverContent>
  </Popover>

  <Dialog
    v-model="showReleaseDetails"
    size="xl"
    :title="selectedReleaseTitle"
    :description="selectedReleaseDescription"
  >
    <div
      v-if="selectedRelease"
      class="space-y-4"
    >
      <div class="flex flex-wrap items-center gap-2">
        <span class="rounded-full border border-border/60 bg-muted/20 px-2 py-0.5 font-mono text-[11px] text-foreground">
          {{ formatDisplayVersion(selectedRelease.version) }}
        </span>
        <span
          v-if="selectedRelease.is_current"
          class="rounded-full bg-primary/10 px-2 py-0.5 text-[11px] font-medium text-primary"
        >
          当前运行版本
        </span>
        <span
          v-else-if="selectedRelease.is_newer"
          class="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[11px] font-medium text-emerald-600 dark:text-emerald-400"
        >
          可升级版本
        </span>
        <span
          v-else
          class="rounded-full bg-muted px-2 py-0.5 text-[11px] font-medium text-muted-foreground"
        >
          历史版本
        </span>
      </div>

      <p
        v-if="selectedReleaseHelpText"
        class="text-xs text-muted-foreground"
      >
        {{ selectedReleaseHelpText }}
      </p>

      <div
        v-if="selectedReleaseDisplayNotes"
        class="max-h-[26rem] overflow-y-auto rounded-xl border border-border/60 bg-muted/25 px-4 py-3 text-sm leading-6 text-foreground/90 shadow-inner shadow-black/[0.02] max-w-none prose prose-sm dark:prose-invert prose-headings:mb-2 prose-headings:mt-4 prose-headings:font-semibold prose-headings:text-foreground prose-h3:text-sm prose-p:my-2 prose-ul:my-2 prose-ul:list-disc prose-ul:pl-5 prose-li:my-1 prose-li:marker:text-primary prose-a:text-primary prose-strong:text-foreground prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5"
        v-html="selectedReleaseNotesHtml"
      />
      <p
        v-else
        class="rounded-lg bg-muted/30 px-3 py-4 text-sm text-muted-foreground"
      >
        这个版本没有附带更新说明。
      </p>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="showReleaseDetails = false"
      >
        关闭
      </Button>
      <Button
        v-if="selectedRelease?.release_url"
        variant="outline"
        @click="handleOpenSelectedReleasePage"
      >
        <ExternalLink class="mr-2 h-3.5 w-3.5" />
        查看标签页
      </Button>
      <Button
        v-if="canUseSelectedRelease"
        :disabled="isBusy"
        @click="handleUseSelectedRelease"
      >
        <RefreshCw
          class="mr-2 h-3.5 w-3.5"
          :class="isBusy ? 'animate-spin' : ''"
        />
        {{ selectedReleaseActionLabel }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import type { CheckUpdateResponse, ReleaseEntry } from '@/api/admin'
import { adminApi } from '@/api/admin'
import { Button, Dialog, Popover, PopoverContent, PopoverTrigger } from '@/components/ui'
import { normalizeReleaseNotesForDisplay } from '@/utils/releaseNotes'
import { formatDisplayVersion } from '@/utils/version'
import { describeUpdateStatus } from '@/utils/updateStatus'
import { sanitizeMarkdown } from '@/utils/sanitize'
import { marked } from 'marked'
import { ChevronRight, ExternalLink, Info, RefreshCw } from 'lucide-vue-next'

const SOURCE_BUILD_UPDATE_HINT = '当前为源码构建，请使用 git pull 后重新编译。'
const SOURCE_BUILD_RELEASE_HINT = '当前为源码构建，请手动切换到对应标签后重新编译。'

const props = defineProps<{
  status: CheckUpdateResponse | null
  loading?: boolean
  updating?: boolean
  updatePhase?: 'download' | 'restart' | 'reconnecting'
  updateSupported?: boolean
  rollbackAvailable?: boolean
  rollingBack?: boolean
  downloadProgressText?: string | null
  downloadProgressPercent?: number | null
}>()

const emit = defineEmits<{
  refresh: []
  openRelease: []
  applyUpdate: []
  previewRelease: [release: ReleaseEntry]
  rollback: []
}>()

const isOpen = ref(false)
const showReleases = ref(false)
const showReleaseDetails = ref(false)
const loadingReleases = ref(false)
const releases = ref<ReleaseEntry[]>([])
const releasesError = ref<string | null>(null)
const selectedRelease = ref<ReleaseEntry | null>(null)
let releasesFetched = false

const loading = computed(() => props.loading ?? false)
const updating = computed(() => props.updating ?? false)
const updatePhase = computed(() => props.updatePhase ?? 'download')
const updateSupported = computed(() => props.updateSupported ?? true)
const rollbackAvailable = computed(() => props.rollbackAvailable ?? false)
const rollingBack = computed(() => props.rollingBack ?? false)
const isReconnecting = computed(() => updatePhase.value === 'reconnecting')
const isDownloadingUpdate = computed(() => updating.value && updatePhase.value === 'download')
const isBusy = computed(() => updating.value || rollingBack.value || isReconnecting.value)
const canApplyUpdate = computed(() => updateSupported.value && props.status?.updatable !== false)
const downloadProgressText = computed(() => props.downloadProgressText || '正在下载更新包...')
const downloadProgressPercent = computed(() => {
  const value = props.downloadProgressPercent
  return typeof value === 'number' && Number.isFinite(value)
    ? Math.max(0, Math.min(100, Math.round(value)))
    : null
})
const progressBarWidth = computed(() => {
  return downloadProgressPercent.value === null ? '35%' : `${downloadProgressPercent.value}%`
})
const updateBlockerText = computed(() => {
  if (!updateSupported.value) {
    return props.status?.update_blocker || SOURCE_BUILD_UPDATE_HINT
  }
  return props.status?.update_blocker || '当前版本暂不支持在线更新'
})
const releaseButtonLabel = computed(() => updateSupported.value ? '查看更新' : '查看发布')
const buttonClass = computed(() => {
  const classes = []

  if (isReconnecting.value) {
    classes.push('bg-primary/10 text-primary animate-pulse')
    return classes
  }

  if (isOpen.value) {
    classes.push('bg-muted/50')
  } else {
    classes.push('hover:bg-muted/50')
  }

  if (props.status?.has_update) {
    classes.push('text-primary')
  } else if (isOpen.value) {
    classes.push('text-foreground')
  } else {
    classes.push('text-muted-foreground hover:text-foreground')
  }

  return classes
})
const statusLabel = computed(() => {
  if (isReconnecting.value) return '重启中'
  if (rollingBack.value) return '回滚中'
  if (updating.value) return '更新中'
  return describeUpdateStatus(props.status)
})
const currentVersionLabel = computed(() => {
  return props.status?.current_version
    ? formatDisplayVersion(props.status.current_version)
    : '加载中...'
})
const latestVersionLabel = computed(() => {
  return props.status?.latest_version
    ? formatDisplayVersion(props.status.latest_version)
    : ''
})
const statusPillClass = computed(() => {
  if (isReconnecting.value || updating.value || rollingBack.value) {
    return 'border-primary/20 bg-primary/10 text-primary'
  }
  if (!props.status) return 'border-border/60 bg-background/70 text-muted-foreground'
  if (props.status.has_update) return 'border-primary/20 bg-primary/10 text-primary'
  if (props.status.error) return 'border-destructive/20 bg-destructive/10 text-destructive'
  return 'border-border/60 bg-background/70 text-muted-foreground'
})
const buttonTitle = computed(() => {
  if (isReconnecting.value) return '服务重启中...'
  if (!props.status) return '版本信息'
  return `版本信息：${statusLabel.value}`
})
const actionButtonLabel = computed(() => {
  if (updating.value) {
    return updatePhase.value === 'restart' ? '重启中...' : '下载中...'
  }
  return updatePhase.value === 'restart' ? '立即重启' : '立即更新'
})
const selectedReleaseTitle = computed(() => {
  return selectedRelease.value
    ? `版本详情 · ${formatDisplayVersion(selectedRelease.value.version)}`
    : '版本详情'
})
const selectedReleaseDescription = computed(() => {
  return selectedRelease.value?.published_at
    ? `发布于 ${formatDate(selectedRelease.value.published_at)}`
    : '查看该版本的发布说明'
})
const canUseSelectedRelease = computed(() => {
  return !!selectedRelease.value &&
    !selectedRelease.value.is_current &&
    selectedRelease.value.updatable !== false &&
    updateSupported.value
})
const selectedReleaseActionLabel = computed(() => {
  if (!selectedRelease.value) return '切换到此版本'
  return selectedRelease.value.is_newer ? '更新到此版本' : '切换到此版本'
})
const selectedReleaseHelpText = computed(() => {
  if (!selectedRelease.value) return ''
  if (selectedRelease.value.is_current) return '当前正在运行这个版本。'
  if (!updateSupported.value) {
    return selectedRelease.value.update_blocker || SOURCE_BUILD_RELEASE_HINT
  }
  if (selectedRelease.value.update_blocker) return selectedRelease.value.update_blocker
  return selectedRelease.value.is_newer
    ? '将这个版本作为在线更新目标。'
    : '将切换到这个历史版本。'
})
const selectedReleaseDisplayNotes = computed(() => {
  return normalizeReleaseNotesForDisplay(selectedRelease.value?.release_notes)
})
const selectedReleaseNotesHtml = computed(() => {
  if (!selectedReleaseDisplayNotes.value) return ''
  try {
    const html = marked.parse(selectedReleaseDisplayNotes.value, {
      async: false,
      breaks: true
    }) as string
    return sanitizeMarkdown(html)
  } catch {
    return selectedReleaseDisplayNotes.value
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/\n/g, '<br>')
  }
})

function formatDate(dateStr: string): string {
  try {
    const date = new Date(dateStr)
    return date.toLocaleDateString('zh-CN', {
      year: 'numeric',
      month: 'short',
      day: 'numeric'
    })
  } catch {
    return dateStr
  }
}

async function fetchReleases(force = false) {
  if (releasesFetched && !force) return
  loadingReleases.value = true
  releasesError.value = null
  try {
    const data = await adminApi.getSystemReleases(force)
    releases.value = data.releases
    releasesError.value = data.error
    releasesFetched = true
  } catch (err) {
    releasesError.value = err instanceof Error ? err.message : '获取版本列表失败'
  } finally {
    loadingReleases.value = false
  }
}

function toggleReleases() {
  showReleases.value = !showReleases.value
  if (showReleases.value) {
    fetchReleases()
  }
}

function openReleaseDetails(release: ReleaseEntry) {
  selectedRelease.value = release
  isOpen.value = false
  showReleaseDetails.value = true
}

function handleRefresh() {
  releasesFetched = false
  if (showReleases.value) {
    fetchReleases(true)
  }
  emit('refresh')
}

function handleOpenRelease() {
  isOpen.value = false
  emit('openRelease')
}

function handleOpenSelectedReleasePage() {
  if (selectedRelease.value?.release_url) {
    window.open(selectedRelease.value.release_url, '_blank', 'noopener,noreferrer')
  }
}

function handleUseSelectedRelease() {
  if (!selectedRelease.value || !canUseSelectedRelease.value) return
  showReleaseDetails.value = false
  isOpen.value = false
  emit('previewRelease', selectedRelease.value)
}

function handleApplyUpdate() {
  emit('applyUpdate')
}

function handleRollback() {
  emit('rollback')
}
</script>
