<template>
  <Dialog
    v-model="isOpen"
    size="4xl"
    title=""
    :close-on-backdrop="true"
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
          <button
            type="button"
            class="flex w-full items-center justify-between gap-3 text-left"
            :aria-expanded="preflightExpanded"
            @click="preflightExpanded = !preflightExpanded"
          >
            <div class="min-w-0">
              <div class="flex items-center gap-2 text-xs font-semibold text-foreground">
                <span>升级前检查</span>
                <ChevronDown
                  class="h-3.5 w-3.5 text-muted-foreground transition-transform"
                  :class="{ 'rotate-180': preflightExpanded }"
                />
              </div>
              <div class="mt-0.5 text-[10px] uppercase tracking-[0.18em] text-muted-foreground">
                Preflight
              </div>
            </div>
            <div class="flex shrink-0 items-center gap-2 text-[11px] text-muted-foreground">
              <span>通过 {{ preflightOkCount }}</span>
              <span v-if="preflightWarningCount">警告 {{ preflightWarningCount }}</span>
              <span v-if="preflightBlockedCount" class="text-destructive">阻塞 {{ preflightBlockedCount }}</span>
            </div>
          </button>

          <template v-if="preflightExpanded">
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
              class="mt-3 grid gap-2 sm:grid-cols-2 lg:grid-cols-3"
            >
              <div
                v-for="check in updatePreflight.checks"
                :key="check.key"
                class="flex min-h-[92px] flex-col justify-between gap-2 rounded-lg border px-3 py-2"
                :class="preflightStatusClass(check.status)"
              >
                <div class="flex items-start justify-between gap-2">
                  <div class="flex min-w-0 items-center gap-2">
                    <span class="flex h-6 w-6 shrink-0 items-center justify-center rounded-full bg-background/70">
                      <component
                        :is="preflightStatusIcon(check.status)"
                        class="h-3.5 w-3.5"
                      />
                    </span>
                    <span class="truncate text-sm font-medium text-foreground">
                      {{ check.label }}
                    </span>
                  </div>
                  <span class="shrink-0 rounded-full border bg-background/60 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wide">
                    {{ check.status }}
                  </span>
                </div>
                <p
                  class="line-clamp-2 text-xs leading-5 text-muted-foreground"
                  :title="check.message"
                >
                  {{ check.message }}
                </p>
              </div>
            </div>
          </template>
        </div>

        <!-- Release Notes -->
        <div
          v-if="displayReleaseNotes"
          class="mt-3 mb-4 w-full rounded-xl border border-border/60 bg-muted/20 px-4 py-3 text-left"
        >
          <button
            type="button"
            class="flex w-full items-center justify-between gap-3 text-left"
            :aria-expanded="releaseNotesExpanded"
            @click="releaseNotesExpanded = !releaseNotesExpanded"
          >
            <div class="min-w-0">
              <div class="flex items-center gap-2 text-xs font-medium uppercase tracking-[0.18em] text-muted-foreground/80">
                <span>更新内容</span>
                <ChevronDown
                  class="h-3.5 w-3.5 transition-transform"
                  :class="{ 'rotate-180': releaseNotesExpanded }"
                />
              </div>
              <div
                v-if="publishedAt"
                class="mt-1 text-xs text-muted-foreground"
              >
                发布于 {{ formattedPublishedAt }}
              </div>
            </div>
            <span class="shrink-0 rounded-full border border-border/70 bg-background/60 px-2 py-0.5 text-[11px] text-muted-foreground">
              {{ releaseNotesExpanded ? '收起' : '展开' }}
            </span>
          </button>
          <!-- eslint-disable vue/no-v-html -->
          <div
            v-if="releaseNotesExpanded"
            class="mt-3 max-h-64 w-full overflow-y-auto rounded-xl border border-border/60 bg-background/50 px-4 py-3 text-sm leading-6 text-foreground/90 shadow-inner shadow-black/[0.02] max-w-none prose prose-sm dark:prose-invert prose-headings:mb-2 prose-headings:mt-4 prose-headings:font-semibold prose-headings:text-foreground prose-h3:text-sm prose-p:my-2 prose-ul:my-2 prose-ul:list-disc prose-ul:pl-5 prose-li:my-1 prose-li:marker:text-primary prose-a:text-primary prose-strong:text-foreground prose-code:rounded prose-code:bg-muted prose-code:px-1 prose-code:py-0.5"
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

        <div
          v-if="showUpdateProgressPanel"
          class="mt-4 w-full overflow-hidden rounded-xl border border-primary/15 bg-primary/[0.035] text-left shadow-sm shadow-primary/5"
        >
          <div class="flex flex-col gap-3 px-4 py-3 sm:flex-row sm:items-start sm:justify-between">
            <div class="min-w-0">
              <div class="text-[11px] font-semibold uppercase text-primary">
                {{ progressPanelEyebrow }}
              </div>
              <div class="mt-1 flex items-center gap-2">
                <Loader2
                  v-if="updating"
                  class="h-4 w-4 shrink-0 animate-spin text-primary"
                />
                <CheckCircle2
                  v-else
                  class="h-4 w-4 shrink-0 text-emerald-500"
                />
                <span class="truncate text-sm font-semibold text-foreground">
                  {{ progressPanelTitle }}
                </span>
              </div>
              <p class="mt-1 text-xs leading-5 text-muted-foreground">
                {{ progressPanelDescription }}
              </p>
            </div>
            <span class="shrink-0 rounded-full border border-primary/20 bg-background/70 px-2 py-0.5 font-mono text-[11px] text-primary">
              {{ progressMetricText }}
            </span>
          </div>

          <div class="px-4 pb-3">
            <div class="h-2 w-full overflow-hidden rounded-full bg-primary/10">
              <div
                class="update-progress-bar h-full rounded-full bg-primary transition-all duration-500"
                :class="{ 'update-progress-bar--indeterminate': isIndeterminateProgress }"
                :style="{ width: progressBarWidth }"
              />
            </div>
            <div class="mt-2 flex items-center justify-between gap-3 text-[11px] text-muted-foreground">
              <span class="min-w-0 truncate">{{ downloadProgressText }}</span>
              <span
                v-if="downloadProgressPercent !== null"
                class="shrink-0 font-mono text-primary"
              >
                {{ downloadProgressPercent }}%
              </span>
            </div>
          </div>

          <div class="grid grid-cols-3 border-t border-primary/10 bg-background/35">
            <div
              v-for="step in updateProgressSteps"
              :key="step.key"
              class="relative px-3 py-2.5"
            >
              <div
                class="mx-auto mb-1 flex h-6 w-6 items-center justify-center rounded-full border text-[11px] font-semibold"
                :class="progressStepClass(step.state)"
              >
                <CheckCircle2
                  v-if="step.state === 'done'"
                  class="h-3.5 w-3.5"
                />
                <Loader2
                  v-else-if="step.state === 'active'"
                  class="h-3.5 w-3.5 animate-spin"
                />
                <span v-else>{{ step.index }}</span>
              </div>
              <div
                class="truncate text-center text-xs font-medium"
                :class="step.state === 'pending' ? 'text-muted-foreground' : 'text-foreground'"
              >
                {{ step.label }}
              </div>
              <div class="mt-0.5 truncate text-center text-[10px] text-muted-foreground">
                {{ step.detail }}
              </div>
            </div>
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
          v-if="isDockerUpdate && dockerGuidedCommands"
          class="mt-3 w-full rounded-xl border border-sky-500/20 bg-sky-500/[0.06] px-4 py-3 text-left"
        >
          <div class="flex items-start gap-3">
            <div class="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-sky-500/10 text-sky-600 dark:text-sky-300">
              <Terminal class="h-4 w-4" />
            </div>
            <div class="min-w-0 flex-1 space-y-3">
              <div class="min-w-0">
                <div class="text-sm font-semibold text-foreground">
                  Docker 两阶段更新
                </div>
                <p class="mt-1 text-xs leading-5 text-muted-foreground">
                  先在宿主机准备新镜像，确认时再快速重建 app 容器。
                </p>
              </div>

              <div class="grid gap-2 sm:grid-cols-2">
                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2.5">
                  <div class="flex items-start justify-between gap-2">
                    <div class="min-w-0">
                      <div class="flex items-center gap-2 text-sm font-medium text-foreground">
                        <Download class="h-4 w-4 text-sky-600 dark:text-sky-300" />
                        准备镜像
                      </div>
                      <p class="mt-1 text-xs leading-5 text-muted-foreground">
                        拉取新 app 镜像，当前服务继续运行。
                      </p>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      class="h-7 shrink-0 px-2 text-xs"
                      @click="copyDockerUpdateCommand(dockerGuidedCommands.prepareCommand, 'prepare')"
                    >
                      <CheckCircle2
                        v-if="dockerCommandCopied === 'prepare'"
                        class="mr-1.5 h-3.5 w-3.5 text-emerald-500"
                      />
                      <Copy
                        v-else
                        class="mr-1.5 h-3.5 w-3.5"
                      />
                      {{ dockerCopyLabel('prepare') }}
                    </Button>
                  </div>
                  <code class="mt-2 block break-all rounded-md bg-background/80 px-2 py-1.5 font-mono text-xs leading-5 text-foreground">
                    {{ dockerGuidedCommands.prepareCommand }}
                  </code>
                </div>

                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2.5">
                  <div class="flex items-start justify-between gap-2">
                    <div class="min-w-0">
                      <div class="flex items-center gap-2 text-sm font-medium text-foreground">
                        <Rocket class="h-4 w-4 text-sky-600 dark:text-sky-300" />
                        快速切换
                      </div>
                      <p class="mt-1 text-xs leading-5 text-muted-foreground">
                        跳过拉取，执行备份、重建和健康检查。
                      </p>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      class="h-7 shrink-0 px-2 text-xs"
                      @click="copyDockerUpdateCommand(dockerGuidedCommands.applyCommand, 'apply')"
                    >
                      <CheckCircle2
                        v-if="dockerCommandCopied === 'apply'"
                        class="mr-1.5 h-3.5 w-3.5 text-emerald-500"
                      />
                      <Copy
                        v-else
                        class="mr-1.5 h-3.5 w-3.5"
                      />
                      {{ dockerCopyLabel('apply') }}
                    </Button>
                  </div>
                  <code class="mt-2 block break-all rounded-md bg-background/80 px-2 py-1.5 font-mono text-xs leading-5 text-foreground">
                    {{ dockerGuidedCommands.applyCommand }}
                  </code>
                </div>
              </div>

              <div class="overflow-hidden rounded-lg border border-border/60 bg-background/80">
                <div class="flex items-center justify-between gap-2 border-b border-border/60 px-3 py-1.5 text-[11px] text-muted-foreground">
                  <div class="flex items-center gap-2">
                    <Terminal class="h-3.5 w-3.5" />
                    一次性完整更新
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    class="h-6 px-2 text-[11px]"
                    @click="copyDockerUpdateCommand(dockerGuidedCommands.updateCommand, 'full')"
                  >
                    <CheckCircle2
                      v-if="dockerCommandCopied === 'full'"
                      class="mr-1 h-3 w-3 text-emerald-500"
                    />
                    <Copy
                      v-else
                      class="mr-1 h-3 w-3"
                    />
                    {{ dockerCopyLabel('full') }}
                  </Button>
                </div>
                <code class="block break-all px-3 py-2 font-mono text-xs leading-5 text-foreground">
                  {{ dockerGuidedCommands.updateCommand }}
                </code>
              </div>

              <div class="grid gap-2 text-xs sm:grid-cols-3">
                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2">
                  <span class="mr-1.5 font-mono text-sky-600 dark:text-sky-300">1</span>
                  进入 compose 目录
                </div>
                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2">
                  <span class="mr-1.5 font-mono text-sky-600 dark:text-sky-300">2</span>
                  先准备镜像
                </div>
                <div class="rounded-lg border border-border/60 bg-background/55 px-3 py-2">
                  <span class="mr-1.5 font-mono text-sky-600 dark:text-sky-300">3</span>
                  低峰快速切换
                </div>
              </div>

              <p class="text-[11px] leading-5 text-muted-foreground">
                切换阶段的中断主要来自容器重建和健康检查；代理变量只影响 GitHub 检查，镜像拉取代理取决于 Docker 守护进程配置。
              </p>
            </div>
          </div>
        </div>

        <div class="mt-4 w-full rounded-xl border border-border/60 bg-muted/20 px-4 py-3 text-left">
          <div class="flex items-center justify-between gap-3">
            <button
              type="button"
              class="flex min-w-0 items-center gap-3 text-left"
              :aria-expanded="updateHistoryExpanded"
              @click="updateHistoryExpanded = !updateHistoryExpanded"
            >
              <div class="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
                <History class="h-4 w-4" />
              </div>
              <div class="min-w-0">
                <div class="flex items-center gap-2 text-sm font-semibold text-foreground">
                  <span>最近更新记录</span>
                  <ChevronDown
                    class="h-3.5 w-3.5 text-muted-foreground transition-transform"
                    :class="{ 'rotate-180': updateHistoryExpanded }"
                  />
                </div>
                <div class="mt-0.5 text-[11px] text-muted-foreground">
                  {{ updateHistorySummaryText }}
                </div>
              </div>
            </button>
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

          <template v-if="updateHistoryExpanded">
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
          </template>
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
          :disabled="rollingBack"
          @click="handleSecondaryDismiss"
        >
          {{ secondaryDismissLabel }}
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
import {
  buildDockerGuidedCommands,
  isPreflightBlocking,
} from './updateDialogLogic'
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  Clock3,
  Copy,
  Download,
  History,
  Loader2,
  RefreshCw,
  Rocket,
  Terminal,
  XCircle,
} from 'lucide-vue-next'

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
  dockerPrepareCommand?: string | null
  dockerApplyCommand?: string | null
  reconnectMessage?: string
  rollbackAvailable?: boolean
  rollingBack?: boolean
  updatePreflight?: SystemUpdatePreflightResponse | null
  loadingUpdatePreflight?: boolean
  updatePreflightError?: string | null
  downloadProgressText?: string | null
  downloadProgressPercent?: number | null
  updateTaskPhase?: string | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  applyUpdate: []
  rollback: []
}>()

type DockerCommandKind = 'prepare' | 'apply' | 'full'
type UpdateProgressStepState = 'done' | 'active' | 'pending'

const SOURCE_BUILD_UPDATE_HINT = '当前为源码构建，请使用 git pull 后重新编译。'
const UPDATE_HISTORY_LIMIT = 5

const isOpen = ref(props.modelValue)
const updateHistory = ref<UpdateHistoryEntry[]>([])
const loadingUpdateHistory = ref(false)
const updateHistoryError = ref<string | null>(null)
const dockerCommandCopied = ref<DockerCommandKind | null>(null)
const dockerCommandCopyError = ref<DockerCommandKind | null>(null)
const preflightExpanded = ref(false)
const releaseNotesExpanded = ref(false)
const updateHistoryExpanded = ref(false)
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
const dockerGuidedCommands = computed(() =>
  buildDockerGuidedCommands({
    updateCommand: props.dockerUpdateCommand,
    prepareCommand: props.dockerPrepareCommand,
    applyCommand: props.dockerApplyCommand,
  })
)
const updateBlockerText = computed(() => {
  if (!updateSupported.value) return props.updateBlocker || SOURCE_BUILD_UPDATE_HINT
  return props.updateBlocker || '当前版本暂不支持在线更新'
})
const reconnectMessage = computed(() => props.reconnectMessage ?? '等待服务恢复...')
const rollbackAvailable = computed(() => props.rollbackAvailable ?? false)
const rollingBack = computed(() => props.rollingBack ?? false)
const downloadProgressText = computed(() => props.downloadProgressText || '正在下载更新包...')
const updateTaskPhase = computed(() => props.updateTaskPhase || null)
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
const showUpdateProgressPanel = computed(() => {
  return updating.value || updatePhase.value === 'restart'
})
const isIndeterminateProgress = computed(() => {
  return updating.value && updatePhase.value === 'download' && downloadProgressPercent.value === null
})
const progressBarWidth = computed(() => {
  if (downloadProgressPercent.value !== null) return `${downloadProgressPercent.value}%`
  if (updatePhase.value === 'restart') return updating.value ? '86%' : '100%'
  return isIndeterminateProgress.value ? '46%' : '18%'
})
const progressPanelEyebrow = computed(() => {
  return updateStrategy.value === 'docker' ? 'Docker 在线更新' : '在线更新'
})
const progressPanelTitle = computed(() => {
  if (updatePhase.value === 'restart') {
    return updating.value ? '正在快速切换' : '新版本已准备完成'
  }
  switch (updateTaskPhase.value) {
    case 'downloading':
      return updateStrategy.value === 'docker' ? '正在拉取镜像' : '正在下载安装包'
    case 'downloading_checksum':
      return '正在获取校验文件'
    case 'verifying':
      return '正在校验完整性'
    case 'extracting':
      return '正在解压更新包'
    case 'backing_up':
      return '正在备份数据'
    case 'prepared':
      return '新版本已准备完成'
    case 'preparing':
    default:
      return updateStrategy.value === 'docker' ? '正在准备镜像' : '正在准备更新'
  }
})
const progressPanelDescription = computed(() => {
  if (updatePhase.value === 'restart') {
    return updating.value
      ? '正在重建应用容器并等待健康检查，页面会在服务恢复后自动刷新。'
      : '新版本已经下载完成，当前服务仍在运行。点击立即重启后会快速切换。'
  }
  switch (updateTaskPhase.value) {
    case 'downloading':
      return updateStrategy.value === 'docker'
        ? '后台正在拉取新 app 镜像，当前服务保持运行。'
        : '后台正在下载更新包，当前服务保持运行。'
    case 'downloading_checksum':
    case 'verifying':
      return '正在确认更新包来源和完整性，完成后会进入待重启状态。'
    case 'extracting':
      return '正在展开新版本文件，当前服务保持运行。'
    case 'backing_up':
      return '正在备份数据，随后会切换到新版本。'
    default:
      return '后台任务已启动，准备完成后会提示你确认重启。'
  }
})
const progressMetricText = computed(() => {
  if (downloadProgressPercent.value !== null) return `${downloadProgressPercent.value}%`
  if (updatePhase.value === 'restart' && !updating.value) return '就绪'
  return '进行中'
})
const updateProgressSteps = computed(() => {
  const prepareState: UpdateProgressStepState = updatePhase.value === 'download'
    ? (updating.value ? 'active' : 'pending')
    : 'done'
  const switchState: UpdateProgressStepState = updatePhase.value === 'restart'
    ? (updating.value ? 'active' : 'pending')
    : 'pending'

  return [
    {
      key: 'prepare',
      index: 1,
      label: updateStrategy.value === 'docker' ? '准备镜像' : '下载更新',
      detail: '不中断服务',
      state: prepareState,
    },
    {
      key: 'switch',
      index: 2,
      label: '快速切换',
      detail: '确认后重启',
      state: switchState,
    },
    {
      key: 'refresh',
      index: 3,
      label: '自动刷新',
      detail: '恢复后返回',
      state: 'pending' as UpdateProgressStepState,
    },
  ]
})
const actionButtonLabel = computed(() => {
  if (updating.value) {
    return updatePhase.value === 'restart' ? '重启中...' : '下载中...'
  }
  return updatePhase.value === 'restart' ? '立即重启' : '立即更新'
})
const secondaryDismissLabel = computed(() => {
  if (updating.value) return '收起后台'
  return updatePhase.value === 'restart' ? '稍后重启' : '稍后提醒'
})

watch(() => props.modelValue, (val) => {
  isOpen.value = val
  if (val) {
    preflightExpanded.value = false
    releaseNotesExpanded.value = false
    updateHistoryExpanded.value = false
  }
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

function progressStepClass(state: UpdateProgressStepState): string {
  if (state === 'done') {
    return 'border-emerald-500/25 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
  }
  if (state === 'active') {
    return 'border-primary/25 bg-primary/10 text-primary shadow-sm shadow-primary/10'
  }
  return 'border-border/70 bg-background/70 text-muted-foreground'
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

function handleSecondaryDismiss() {
  if (updating.value) {
    isOpen.value = false
    return
  }
  handleLater()
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

async function copyDockerUpdateCommand(command: string, kind: DockerCommandKind) {
  dockerCommandCopied.value = null
  dockerCommandCopyError.value = null
  try {
    await navigator.clipboard.writeText(command)
    dockerCommandCopied.value = kind
    window.setTimeout(() => {
      if (dockerCommandCopied.value === kind) {
        dockerCommandCopied.value = null
      }
    }, 1600)
  } catch {
    dockerCommandCopyError.value = kind
    window.setTimeout(() => {
      if (dockerCommandCopyError.value === kind) {
        dockerCommandCopyError.value = null
      }
    }, 1600)
  }
}

function dockerCopyLabel(kind: DockerCommandKind): string {
  if (dockerCommandCopied.value === kind) return '已复制'
  if (dockerCommandCopyError.value === kind) return '复制失败'
  return '复制'
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

<style scoped>
.update-progress-bar {
  position: relative;
  overflow: hidden;
}

.update-progress-bar::after {
  content: "";
  position: absolute;
  inset: 0;
  background: linear-gradient(90deg, transparent, rgb(255 255 255 / 0.38), transparent);
  transform: translateX(-100%);
  animation: update-progress-sheen 1.8s ease-in-out infinite;
}

.update-progress-bar--indeterminate {
  animation: update-progress-drift 1.35s ease-in-out infinite alternate;
}

@keyframes update-progress-sheen {
  to {
    transform: translateX(100%);
  }
}

@keyframes update-progress-drift {
  from {
    transform: translateX(-8%);
  }
  to {
    transform: translateX(118%);
  }
}
</style>
