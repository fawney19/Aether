<template>
  <CardSection
    title="系统更新"
    description="检查版本并从 Web 执行受控更新"
  >
    <div class="space-y-4">
      <div class="grid gap-3 md:grid-cols-4">
        <div class="rounded-lg border border-border/70 bg-muted/20 px-3 py-2.5">
          <p class="text-xs text-muted-foreground">
            当前版本
          </p>
          <p class="mt-1 break-all font-mono text-sm text-foreground">
            {{ currentVersion }}
          </p>
        </div>
        <div class="rounded-lg border border-border/70 bg-muted/20 px-3 py-2.5">
          <p class="text-xs text-muted-foreground">
            最新版本
          </p>
          <p class="mt-1 break-all font-mono text-sm text-foreground">
            {{ latestVersion }}
          </p>
        </div>
        <div class="rounded-lg border border-border/70 bg-muted/20 px-3 py-2.5">
          <p class="text-xs text-muted-foreground">
            部署模式
          </p>
          <p class="mt-1 text-sm font-medium text-foreground">
            {{ modeLabel }}
          </p>
        </div>
        <div class="rounded-lg border border-border/70 bg-muted/20 px-3 py-2.5">
          <p class="text-xs text-muted-foreground">
            更新状态
          </p>
          <p
            class="mt-1 text-sm font-medium"
            :class="statusClass"
          >
            {{ statusLabel }}
          </p>
        </div>
      </div>

      <div
        v-if="versionStatus?.error"
        class="rounded-lg border border-destructive/25 bg-destructive/5 px-3 py-2 text-sm text-destructive"
      >
        检查更新失败：{{ versionStatus.error }}
      </div>

      <div
        v-if="versionStatus?.has_update && !canApply"
        class="rounded-lg border border-amber-500/25 bg-amber-500/5 px-3 py-2 text-sm text-amber-700 dark:text-amber-300"
      >
        {{ versionStatus.update_blocker || '当前部署暂不支持从 Web 执行更新。' }}
        <span v-if="dockerUpdateCommand">可在服务器执行：{{ dockerUpdateCommand }}</span>
      </div>

      <div
        v-if="isBusy"
        class="rounded-lg border border-primary/20 bg-primary/5 px-3 py-2"
      >
        <div class="flex items-center justify-between gap-3 text-sm text-primary">
          <span class="truncate">{{ updateProgressText }}</span>
          <span
            v-if="updateProgressPercent !== null"
            class="shrink-0 font-mono text-xs"
          >
            {{ updateProgressPercent }}%
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
        v-if="showDockerOutput"
        class="rounded-lg border border-border bg-background"
      >
        <div class="flex items-center justify-between border-b border-border px-3 py-2">
          <span class="text-xs font-medium text-muted-foreground">Docker updater 输出</span>
          <span class="text-[10px] uppercase tracking-wide text-muted-foreground">{{ updateTaskStatus?.docker_status || 'idle' }}</span>
        </div>
        <pre class="max-h-56 overflow-auto whitespace-pre-wrap break-words p-3 text-xs leading-5 text-muted-foreground">{{ dockerOutputText }}</pre>
      </div>

      <div class="flex flex-wrap items-center gap-2">
        <Button
          variant="outline"
          :disabled="loadingVersionStatus || applyingSystemUpdate || rollingBack"
          @click="loadVersionStatus(true)"
        >
          <RefreshCw
            class="mr-2 h-4 w-4"
            :class="loadingVersionStatus ? 'animate-spin' : ''"
          />
          检查更新
        </Button>

        <Button
          v-if="showApplyButton"
          :disabled="applyingSystemUpdate || rollingBack"
          @click="confirmApply"
        >
          <Download
            v-if="systemUpdatePhase === 'download' && updateExecutionMode !== 'docker'"
            class="mr-2 h-4 w-4"
          />
          <RefreshCw
            v-else
            class="mr-2 h-4 w-4"
            :class="applyingSystemUpdate ? 'animate-spin' : ''"
          />
          {{ primaryActionLabel }}
        </Button>

        <Button
          v-if="rollbackAvailable"
          variant="outline"
          :disabled="applyingSystemUpdate || rollingBack"
          @click="confirmRollback"
        >
          <Undo2 class="mr-2 h-4 w-4" />
          {{ rollingBack ? '回滚中...' : '回滚上一版本' }}
        </Button>

        <Button
          v-if="versionStatus?.release_url"
          variant="ghost"
          @click="openVersionReleasePage"
        >
          <ExternalLink class="mr-2 h-4 w-4" />
          查看发布页
        </Button>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { Download, ExternalLink, RefreshCw, Undo2 } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import { CardSection } from '@/components/layout'
import { useSystemUpdate } from '@/composables/useSystemUpdate'

const {
  versionStatus,
  loadingVersionStatus,
  applyingSystemUpdate,
  updateSupported,
  updateExecutionMode,
  updateStrategy,
  dockerUpdateCommand,
  rollbackAvailable,
  rollingBack,
  updateTaskStatus,
  updateOutputTail,
  systemUpdatePhase,
  updateProgressText,
  updateProgressPercent,
  loadVersionStatus,
  openVersionReleasePage,
  handleApplySystemUpdate,
  handleRollback,
  refreshUpdateTaskStatus,
} = useSystemUpdate()

const currentVersion = computed(() => versionStatus.value?.current_version || __APP_VERSION__ || '加载中')
const latestVersion = computed(() => versionStatus.value?.latest_version || (loadingVersionStatus.value ? '检查中...' : '暂无更新'))
const modeLabel = computed(() => {
  if (updateExecutionMode.value === 'docker' || updateStrategy.value === 'docker') return 'Docker Compose'
  if (updateExecutionMode.value === 'self') return '二进制自更新'
  return '手动更新'
})
const canApply = computed(() => updateSupported.value && versionStatus.value?.updatable !== false)
const isBusy = computed(() => applyingSystemUpdate.value || systemUpdatePhase.value === 'reconnecting')
const showApplyButton = computed(() => {
  if (!canApply.value) return false
  if (systemUpdatePhase.value === 'restart') return true
  return versionStatus.value?.has_update === true
})
const primaryActionLabel = computed(() => {
  if (applyingSystemUpdate.value) return updateExecutionMode.value === 'docker' ? '执行中...' : '处理中...'
  if (updateExecutionMode.value === 'docker') return '执行更新'
  if (systemUpdatePhase.value === 'restart') return '立即重启'
  return '下载更新'
})
const statusLabel = computed(() => {
  if (systemUpdatePhase.value === 'reconnecting') return '服务重启中'
  if (applyingSystemUpdate.value) return '更新执行中'
  if (versionStatus.value?.has_update) return canApply.value ? '有可用更新' : '需手动更新'
  if (versionStatus.value?.error) return '检查失败'
  return '已是最新'
})
const statusClass = computed(() => {
  if (versionStatus.value?.error) return 'text-destructive'
  if (versionStatus.value?.has_update && canApply.value) return 'text-primary'
  if (versionStatus.value?.has_update) return 'text-amber-600 dark:text-amber-400'
  return 'text-emerald-600 dark:text-emerald-400'
})
const progressBarWidth = computed(() => {
  if (typeof updateProgressPercent.value === 'number') return `${updateProgressPercent.value}%`
  return applyingSystemUpdate.value ? '65%' : '100%'
})
const showDockerOutput = computed(() => updateExecutionMode.value === 'docker' && updateOutputTail.value.length > 0)
const dockerOutputText = computed(() => updateOutputTail.value.join('\n'))

function confirmApply() {
  if (!window.confirm('确认从 Web 执行系统更新？更新期间服务会短暂不可用。')) return
  void handleApplySystemUpdate()
}

function confirmRollback() {
  if (!window.confirm('确认回滚到上一版本？服务会短暂不可用。')) return
  void handleRollback()
}

onMounted(() => {
  void loadVersionStatus()
  void refreshUpdateTaskStatus()
})
</script>
