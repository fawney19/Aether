<template>
  <AppShell
    :show-notice="showAuthError"
    :main-class="mainClasses"
    :sidebar-class="sidebarClasses"
    :content-class="contentClasses"
  >
    <template #notice>
      <div class="flex w-full max-w-3xl items-center justify-between rounded-3xl bg-orange-500 px-6 py-3 text-white shadow-2xl ring-1 ring-white/30">
        <div class="flex items-center gap-3">
          <AlertTriangle class="h-5 w-5" />
          <span>{{ t('auth.expired') }}</span>
        </div>
        <Button
          variant="outline"
          size="sm"
          class="border-white/60 text-white hover:bg-white/10"
          @click="handleRelogin"
        >
          {{ t('auth.relogin') }}
        </Button>
      </div>
    </template>

    <template #sidebar>
      <!-- HEADER (Brand) -->
      <div class="shrink-0 flex items-center px-6 h-20">
        <RouterLink
          to="/"
          class="flex items-center gap-3 group transition-opacity hover:opacity-80"
        >
          <HeaderLogo
            size="h-9 w-9"
            class-name="text-[#191919] dark:text-white"
          />
          <div class="flex flex-col justify-center">
            <h1 class="text-lg font-bold text-[#191919] dark:text-white leading-none">
              {{ siteName }}
            </h1>
            <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1.5 font-medium tracking-wide">{{ siteSubtitle }}</span>
          </div>
        </RouterLink>
      </div>

      <!-- NAVIGATION -->
      <div class="flex-1 overflow-y-auto py-2 scrollbar-none">
        <SidebarNav
          :items="navigation"
          :is-active="isNavActive"
          @prefetch="prefetchNavigationItem"
        />
      </div>

      <!-- FOOTER (Profile) -->
      <div class="p-4 border-t border-[#3d3929]/5 dark:border-white/5">
        <div class="flex items-center justify-between p-2 rounded-xl">
          <div class="flex items-center gap-3 min-w-0">
            <div class="w-8 h-8 rounded-full bg-[#f0f0eb] dark:bg-white/10 border border-black/5 flex items-center justify-center text-xs font-bold text-[#3d3929] dark:text-[#d4a27f] shrink-0">
              {{ authStore.user?.username?.substring(0, 2).toUpperCase() }}
            </div>
            <div class="flex flex-col min-w-0">
              <span class="text-xs font-semibold leading-none truncate opacity-90 text-foreground">{{ authStore.user?.username }}</span>
              <span class="text-[10px] opacity-50 leading-none mt-1.5 text-muted-foreground">{{ currentRoleLabel }}</span>
            </div>
          </div>

          <div class="flex items-center gap-1">
            <RouterLink
              to="/dashboard/settings"
              class="p-1.5 hover:bg-muted/50 rounded-md text-muted-foreground hover:text-foreground transition-colors"
              :title="t('common.settings')"
            >
              <Settings class="w-4 h-4" />
            </RouterLink>
            <button
              class="p-1.5 rounded-md text-muted-foreground hover:text-red-500 transition-colors"
              :title="t('common.logout')"
              @click="handleLogout"
            >
              <LogOut class="w-4 h-4" />
            </button>
          </div>
        </div>
      </div>
    </template>

    <template #header>
      <!-- Mobile Header (matches Home page style) -->
      <header class="lg:hidden fixed top-0 left-0 right-0 z-50 border-b border-[var(--shell-border)] bg-[var(--shell-glass)] backdrop-blur-xl transition-all">
        <div class="mx-auto max-w-7xl px-6 py-4">
          <div class="flex items-center justify-between">
            <!-- Logo & Brand -->
            <RouterLink
              to="/"
              class="flex items-center gap-3 group"
            >
              <HeaderLogo
                size="h-9 w-9"
                class-name="text-[#191919] dark:text-white"
              />
              <div class="flex flex-col justify-center">
                <h1 class="text-lg font-bold text-[#191919] dark:text-white leading-none">
                  {{ siteName }}
                </h1>
                <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1.5 font-medium tracking-wide">{{ siteSubtitle }}</span>
              </div>
            </RouterLink>

            <!-- Right Actions -->
            <div class="flex items-center gap-3">
              <VersionButton
                v-if="isAdmin"
                :status="versionStatus"
                :loading="loadingVersionStatus"
                :updating="applyingSystemUpdate"
                :update-phase="systemUpdatePhase"
                :update-supported="updateSupported"
                :rollback-available="rollbackAvailable"
                :rolling-back="rollingBack"
                :download-progress-text="updateProgressText"
                :download-progress-percent="updateProgressPercent"
                @refresh="handleVersionRefresh"
                @open-release="openVersionReleasePage"
                @preview-release="openReleaseUpdateDialog"
                @apply-update="handleApplySystemUpdate"
                @rollback="handleRollback"
              />
              <LanguageSwitcher />
              <ThemeModeButton />
              <button
                class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
                @click="mobileMenuOpen = !mobileMenuOpen"
              >
                <div class="relative w-5 h-5">
                  <Transition
                    enter-active-class="transition-all duration-200 ease-out"
                    enter-from-class="opacity-0 rotate-90 scale-75"
                    enter-to-class="opacity-100 rotate-0 scale-100"
                    leave-active-class="transition-all duration-150 ease-in absolute inset-0"
                    leave-from-class="opacity-100 rotate-0 scale-100"
                    leave-to-class="opacity-0 -rotate-90 scale-75"
                    mode="out-in"
                  >
                    <Menu
                      v-if="!mobileMenuOpen"
                      class="h-5 w-5"
                    />
                    <X
                      v-else
                      class="h-5 w-5"
                    />
                  </Transition>
                </div>
              </button>
            </div>
          </div>
        </div>

        <!-- Mobile Dropdown Menu -->
        <Transition
          enter-active-class="transition-all duration-300 ease-out"
          enter-from-class="opacity-0 -translate-y-2"
          enter-to-class="opacity-100 translate-y-0"
          leave-active-class="transition-all duration-200 ease-in"
          leave-from-class="opacity-100 translate-y-0"
          leave-to-class="opacity-0 -translate-y-2"
        >
          <div
            v-if="mobileMenuOpen"
            class="absolute inset-x-0 top-full max-h-[calc(100dvh-73px)] overflow-y-auto overscroll-contain border-t border-[var(--shell-border)] bg-background shadow-xl [-webkit-overflow-scrolling:touch] touch-pan-y"
          >
            <div class="mx-auto max-w-7xl px-6 py-4 pb-28">
              <!-- Navigation Groups -->
              <div class="space-y-4">
                <div
                  v-for="group in navigation"
                  :key="group.title"
                >
                  <div
                    v-if="group.title"
                    class="text-[10px] font-semibold text-[#91918d] dark:text-muted-foreground uppercase tracking-wider mb-2"
                  >
                    {{ group.title }}
                  </div>
                  <div class="grid grid-cols-2 gap-2">
                    <RouterLink
                      v-for="item in group.items"
                      :key="item.href"
                      :to="item.href"
                      class="flex items-center gap-2.5 px-3 py-2.5 rounded-xl text-sm font-medium transition-all"
                      :class="isNavActive(item.href)
                        ? 'bg-[#cc785c]/10 dark:bg-[#cc785c]/20 text-[#cc785c] dark:text-[#d4a27f]'
                        : 'text-[#666663] dark:text-muted-foreground hover:bg-black/5 dark:hover:bg-white/5 hover:text-[#191919] dark:hover:text-white'"
                      @pointerdown="prefetchNavigationItem(item.href)"
                      @click="mobileMenuOpen = false"
                    >
                      <component
                        :is="item.icon"
                        class="h-4 w-4 shrink-0"
                      />
                      <span class="truncate">{{ item.name }}</span>
                    </RouterLink>
                  </div>
                </div>
              </div>

              <!-- User Section -->
              <div class="mt-4 pt-4 border-t border-[#cc785c]/10 dark:border-[rgba(227,224,211,0.12)]">
                <div class="flex items-center justify-between">
                  <div class="flex items-center gap-3 min-w-0">
                    <div class="w-8 h-8 rounded-full bg-[#f0f0eb] dark:bg-white/10 border border-black/5 flex items-center justify-center text-xs font-bold text-[#3d3929] dark:text-[#d4a27f] shrink-0">
                      {{ authStore.user?.username?.substring(0, 2).toUpperCase() }}
                    </div>
                    <div class="flex flex-col min-w-0">
                      <span class="text-sm font-semibold leading-none truncate text-[#191919] dark:text-white">{{ authStore.user?.username }}</span>
                      <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1">{{ currentRoleLabel }}</span>
                    </div>
                  </div>
                  <div class="flex items-center gap-1">
                    <RouterLink
                      to="/dashboard/settings"
                      class="p-2 hover:bg-muted/50 rounded-lg text-muted-foreground hover:text-foreground transition-colors"
                      :title="t('common.settings')"
                      @click="mobileMenuOpen = false"
                    >
                      <Settings class="w-4 h-4" />
                    </RouterLink>
                    <button
                      class="p-2 rounded-lg text-muted-foreground hover:text-red-500 transition-colors"
                      :title="t('common.logout')"
                      @click="handleLogout"
                    >
                      <LogOut class="w-4 h-4" />
                    </button>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </Transition>
      </header>

      <!-- Desktop Page Header -->
      <header class="hidden lg:flex h-16 px-8 items-center justify-between shrink-0 border-b border-[#3d3929]/5 dark:border-white/5 sticky top-0 z-40 backdrop-blur-md bg-[#faf9f5]/90 dark:bg-[#191714]/90">
        <div class="flex flex-col gap-0.5">
          <div class="flex items-center gap-2 text-sm text-muted-foreground">
            <template
              v-for="(crumb, index) in breadcrumbs"
              :key="index"
            >
              <template v-if="index > 0">
                <ChevronRight class="w-3 h-3 opacity-50" />
              </template>
              <RouterLink
                v-if="crumb.href && index < breadcrumbs.length - 1"
                :to="crumb.href"
                class="hover:text-foreground transition-colors"
              >
                {{ crumb.label }}
              </RouterLink>
              <span
                v-else
                :class="index === breadcrumbs.length - 1 ? 'text-foreground font-medium' : ''"
              >
                {{ crumb.label }}
              </span>
            </template>
            <!-- 页面级操作插入点 -->
            <div id="breadcrumb-actions" />
          </div>
        </div>

        <!-- Demo Mode Badge (center) -->
        <div
          v-if="isDemo"
          class="flex items-center gap-2 px-3 py-1.5 rounded-full bg-amber-100 dark:bg-amber-900/30 text-amber-700 dark:text-amber-400 text-xs font-medium"
        >
          <AlertTriangle class="w-3.5 h-3.5" />
          <span>{{ t('demo.mode') }}</span>
        </div>

        <div class="flex items-center gap-2">
          <!-- Page-level header actions (right side) -->
          <div
            id="header-actions-right"
            class="flex items-center"
          />
          <VersionButton
            v-if="isAdmin"
            :status="versionStatus"
            :loading="loadingVersionStatus"
            :updating="applyingSystemUpdate"
            :update-phase="systemUpdatePhase"
            :update-supported="updateSupported"
            :rollback-available="rollbackAvailable"
            :rolling-back="rollingBack"
            :download-progress-text="updateProgressText"
            :download-progress-percent="updateProgressPercent"
            @refresh="handleVersionRefresh"
            @open-release="openVersionReleasePage"
            @preview-release="openReleaseUpdateDialog"
            @apply-update="handleApplySystemUpdate"
            @rollback="handleRollback"
          />
          <LanguageSwitcher />
          <!-- Theme Toggle -->
          <ThemeModeButton />
          <!-- GitHub Link -->
          <a
            href="https://github.com/fawney19/Aether"
            target="_blank"
            rel="noopener noreferrer"
            class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
            :title="t('common.githubRepository')"
          >
            <GithubIcon class="h-4 w-4" />
          </a>
        </div>
      </header>
    </template>

    <RouterView />

    <Dialog
      v-model="requiredAnnouncementOpen"
      persistent
      size="lg"
      :title="t('announcement.requiredTitle')"
      :description="t('announcement.requiredDescription')"
    >
      <div
        v-if="currentRequiredAnnouncement"
        class="space-y-4"
      >
        <div>
          <h3 class="text-lg font-semibold text-foreground">
            {{ currentRequiredAnnouncement.title }}
          </h3>
          <p class="mt-1 text-xs text-muted-foreground">
            {{ formatRequiredAnnouncementDate(currentRequiredAnnouncement.created_at) }}
          </p>
        </div>
        <!-- eslint-disable vue/no-v-html -->
        <div
          class="prose prose-sm dark:prose-invert max-h-[50vh] max-w-none overflow-y-auto"
          v-html="renderRequiredAnnouncement(currentRequiredAnnouncement.content)"
        />
        <!-- eslint-enable vue/no-v-html -->
      </div>
      <template #footer>
        <Button
          type="button"
          :disabled="acknowledgingRequiredAnnouncement"
          @click="acknowledgeRequiredAnnouncement"
        >
          {{ acknowledgingRequiredAnnouncement ? t('common.confirming') : t('common.confirmRead') }}
        </Button>
      </template>
    </Dialog>

    <!-- 更新提示弹窗 -->
    <UpdateDialog
      v-if="updateInfo"
      v-model="showUpdateDialog"
      :current-version="updateInfo.current_version"
      :latest-version="updateInfo.latest_version || ''"
      :release-url="updateInfo.release_url"
      :release-notes="updateInfo.release_notes"
      :published-at="updateInfo.published_at"
      :dialog-title="updateDialogTitle"
      :version-label="updateDialogVersionLabel"
      :release-link-label="updateDialogReleaseLinkLabel"
      :updating="applyingSystemUpdate"
      :update-phase="systemUpdatePhase"
      :update-supported="updateSupported"
      :updatable="updateInfo.updatable"
      :update-blocker="updateInfo.update_blocker"
      :update-strategy="updateStrategy"
      :docker-update-command="dockerUpdateCommand"
      :reconnect-message="reconnectMessage"
      :rollback-available="rollbackAvailable"
      :rolling-back="rollingBack"
      :download-progress-text="updateProgressText"
      :download-progress-percent="updateProgressPercent"
      @apply-update="handleApplySystemUpdate"
      @rollback="handleRollback"
    />
  </AppShell>
</template>

<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { marked } from 'marked'
import { useAuthStore } from '@/stores/auth'
import { useModuleStore } from '@/stores/modules'
import { useSiteInfo } from '@/composables/useSiteInfo'
import { useToast } from '@/composables/useToast'
import { isDemoMode } from '@/config/demo'
import { adminApi, type CheckUpdateResponse, type ReleaseEntry, type SystemUpdateCapabilityResponse, type UpdateTaskStatusResponse } from '@/api/admin'
import { announcementApi, type Announcement } from '@/api/announcements'
import { parseApiError } from '@/utils/errorParser'
import Button from '@/components/ui/button.vue'
import { Dialog } from '@/components/ui'
import AppShell from '@/components/layout/AppShell.vue'
import SidebarNav from '@/components/layout/SidebarNav.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import LanguageSwitcher from '@/components/common/LanguageSwitcher.vue'
import ThemeModeButton from '@/components/common/ThemeModeButton.vue'
import UpdateDialog from '@/components/common/UpdateDialog.vue'
import VersionButton from '@/components/common/VersionButton.vue'
import { buildUpdateErrorStatus } from '@/utils/updateStatus'
import {
  Settings,
  AlertTriangle,
  LogOut,
  ChevronRight,
  Menu,
  X,
} from 'lucide-vue-next'

import GithubIcon from '@/components/icons/GithubIcon.vue'
import { prefetchAdminNavigationTarget } from '@/utils/adminNavigationPrefetch'
import { sanitizeMarkdown } from '@/utils/sanitize'
import { useI18n, type MessageKey } from '@/i18n'
import { buildBreadcrumbs, buildNavigation } from './main-layout/navigation'

type SystemUpdatePhase = 'download' | 'restart' | 'reconnecting'

const router = useRouter()
const route = useRoute()
const authStore = useAuthStore()
const moduleStore = useModuleStore()
const { siteName, siteSubtitle } = useSiteInfo()
const { success, error: showError } = useToast()
const { t, locale } = useI18n()
const isDemo = computed(() => isDemoMode())
const isAdmin = computed(() => authStore.user?.role === 'admin')

const showAuthError = ref(false)
const mobileMenuOpen = ref(false)
const requiredAnnouncements = ref<Announcement[]>([])
const acknowledgingRequiredAnnouncement = ref(false)
const requiredAnnouncementOpen = computed({
  get: () => requiredAnnouncements.value.length > 0,
  set: (value) => {
    if (value) void loadRequiredAnnouncements()
  }
})
const currentRequiredAnnouncement = computed(() => requiredAnnouncements.value[0] ?? null)

// 更新检查相关
const showUpdateDialog = ref(false)
const updateInfo = ref<CheckUpdateResponse | null>(null)
const versionStatus = ref<CheckUpdateResponse | null>(null)
const loadingVersionStatus = ref(false)
const applyingSystemUpdate = ref(false)
const updateSupported = ref(true)
const updateStrategy = ref('manual')
const updateCapabilityMessage = ref<string | null>(null)
const dockerUpdateCommand = ref<string | null>(null)
const reconnectMessage = ref(t('update.reconnect.waiting'))
const rollbackAvailable = ref(false)
const rollingBack = ref(false)
const updateTaskStatus = ref<UpdateTaskStatusResponse | null>(null)
const updateDialogMode = ref<'latest' | 'selected'>('latest')
const systemUpdatePhase = ref<SystemUpdatePhase>(readStoredSystemUpdatePhase())
const preparedUpdateVersion = ref<string | null>(
  readSessionStorageItem('aether_prepared_update_version')
)
const SOURCE_BUILD_UPDATE_HINT: MessageKey = 'update.error.sourceBuildUpdateHint'
const SOURCE_BUILD_RELEASE_HINT: MessageKey = 'update.error.sourceBuildReleaseHint'
const MANUAL_UPDATE_HINT: MessageKey = 'update.error.manualHint'
const VERSION_STATUS_CACHE_KEY = 'aether_version_status_cache'
const VERSION_STATUS_CACHE_TTL_MS = 20 * 60 * 1000
const VERSION_STATUS_ERROR_CACHE_TTL_MS = 5 * 60 * 1000
let versionStatusLoadPromise: Promise<CheckUpdateResponse | null> | null = null
let updateStatusPollTimer: number | null = null
let updateCheckTimer: number | null = null
let requiredAnnouncementsPromise: Promise<void> | null = null
const updateProgressPercent = computed(() => updateTaskStatus.value?.progress_percent ?? null)
const updateProgressText = computed(() => formatUpdateProgressText(updateTaskStatus.value))
const updateDialogTitle = computed(() => {
  if (updateDialogMode.value === 'selected') {
    return updateSupported.value ? t('update.title.selected') : t('update.title.selectedReadOnly')
  }
  return t('update.title.latest')
})
const updateDialogVersionLabel = computed(() => {
  if (updateDialogMode.value === 'selected') {
    return updateSupported.value ? t('update.version.target') : t('update.version.tag')
  }
  return t('update.version.latest')
})
const updateDialogReleaseLinkLabel = computed(() => {
  if (updateDialogMode.value === 'selected') return t('update.link.tag')
  return updateSupported.value ? t('update.link.update') : t('update.link.release')
})
watch(systemUpdatePhase, (val) => {
  setSessionStorageItem('aether_update_phase', val)
})
watch(preparedUpdateVersion, (val) => {
  if (val) {
    setSessionStorageItem('aether_prepared_update_version', val)
  } else {
    removeSessionStorageItem('aether_prepared_update_version')
  }
})

function readStoredSystemUpdatePhase(): SystemUpdatePhase {
  const stored = readSessionStorageItem('aether_update_phase')
  if (stored === 'restart' || stored === 'reconnecting') return stored
  return 'download'
}

function readSessionStorageItem(key: string): string | null {
  try {
    return sessionStorage.getItem(key)
  } catch {
    return null
  }
}

function setSessionStorageItem(key: string, value: string) {
  try {
    sessionStorage.setItem(key, value)
  } catch {
    // Ignore storage failures; update state still lives in memory for this page.
  }
}

function removeSessionStorageItem(key: string) {
  try {
    sessionStorage.removeItem(key)
  } catch {
    // Ignore storage failures; update state still lives in memory for this page.
  }
}

function readCachedVersionStatus(): CheckUpdateResponse | null {
  const raw = readSessionStorageItem(VERSION_STATUS_CACHE_KEY)
  if (!raw) return null

  try {
    const parsed = JSON.parse(raw) as { cachedAt?: unknown; status?: unknown }
    const cachedAt = typeof parsed.cachedAt === 'number' ? parsed.cachedAt : 0
    const status = parsed.status as CheckUpdateResponse | undefined
    if (!status || typeof status !== 'object') return null

    const ttl = status.error ? VERSION_STATUS_ERROR_CACHE_TTL_MS : VERSION_STATUS_CACHE_TTL_MS
    if (Date.now() - cachedAt > ttl) {
      removeSessionStorageItem(VERSION_STATUS_CACHE_KEY)
      return null
    }
    return status
  } catch {
    removeSessionStorageItem(VERSION_STATUS_CACHE_KEY)
    return null
  }
}

function cacheVersionStatus(status: CheckUpdateResponse | null) {
  if (!status) return
  setSessionStorageItem(
    VERSION_STATUS_CACHE_KEY,
    JSON.stringify({ cachedAt: Date.now(), status })
  )
}

function applyCachedVersionStatus(): boolean {
  const cached = readCachedVersionStatus()
  if (!cached) return false
  versionStatus.value = cached
  syncSystemUpdatePhase(cached)
  return true
}

function formatUpdateProgressText(status: UpdateTaskStatusResponse | null): string {
  if (!status) return t('update.progress.downloadPackage')
  const label = status.progress_label
    ? t('update.progress.downloadingLabel', { label: status.progress_label })
    : formatUpdateTaskPhase(status.phase)
  const downloaded = status.downloaded_bytes
  const total = status.total_bytes
  if (typeof downloaded === 'number' && typeof total === 'number' && total > 0) {
    return `${label} ${formatFileSize(downloaded)} / ${formatFileSize(total)}`
  }
  if (typeof downloaded === 'number' && downloaded > 0) {
    return `${label} ${formatFileSize(downloaded)}`
  }
  return label
}

function formatUpdateTaskPhase(phase: string): string {
  switch (phase) {
    case 'downloading':
      return t('update.progress.downloadingPackage')
    case 'downloading_checksum':
      return t('update.progress.downloadingChecksum')
    case 'verifying':
      return t('update.progress.verifying')
    case 'extracting':
      return t('update.progress.extracting')
    case 'prepared':
      return t('update.progress.prepared')
    default:
      return t('update.progress.preparing')
  }
}

function formatFileSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${bytes} B`
}

async function refreshUpdateTaskStatus() {
  try {
    updateTaskStatus.value = await adminApi.getUpdateStatus()
  } catch {
    // Keep the last progress snapshot while the request is in flight or the service restarts.
  }
}

async function waitForPreparedUpdate(): Promise<UpdateTaskStatusResponse> {
  const deadline = Date.now() + 10 * 60 * 1000
  while (Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 1000))
    await refreshUpdateTaskStatus()
    const status = updateTaskStatus.value
    if (status?.phase === 'prepared') return status
    if (status?.phase === 'failed') {
      throw new Error(status.error || t('update.error.downloadFailed'))
    }
  }
  throw new Error(t('update.error.downloadTimeout'))
}

function startUpdateStatusPolling() {
  stopUpdateStatusPolling()
  void refreshUpdateTaskStatus()
  updateStatusPollTimer = window.setInterval(() => {
    void refreshUpdateTaskStatus()
  }, 1000)
}

function stopUpdateStatusPolling() {
  if (updateStatusPollTimer !== null) {
    window.clearInterval(updateStatusPollTimer)
    updateStatusPollTimer = null
  }
}

// 路由变化时自动关闭移动端菜单
watch(() => route.path, () => {
  mobileMenuOpen.value = false
})

// 检查是否应该显示更新提示
function shouldShowUpdatePrompt(latestVersion: string): boolean {
  const ignoreKey = 'aether_update_ignore'
  const ignoreData = localStorage.getItem(ignoreKey)
  if (!ignoreData) return true

  try {
    const { version, until } = JSON.parse(ignoreData)
    // 如果忽略的是同一版本且未过期，则不显示
    if (version === latestVersion && Date.now() < until) {
      return false
    }
  } catch {
    // 解析失败，显示提示
  }
  return true
}

async function loadVersionStatus(force = false) {
  if (!isAdmin.value) return null
  if (!force && applyCachedVersionStatus()) {
    return versionStatus.value
  }
  if (versionStatusLoadPromise) return versionStatusLoadPromise

  loadingVersionStatus.value = true
  versionStatusLoadPromise = (async () => {
    try {
      const [status, capability] = await Promise.all([
        adminApi.checkUpdate(force),
        adminApi.getSystemUpdateCapability().catch(() => null),
      ])
      if (capability) {
        applyUpdateCapability(capability)
      }
      versionStatus.value = updateSupported.value === false && status.has_update
        ? {
            ...status,
            updatable: false,
            update_blocker: updateUnsupportedMessage(SOURCE_BUILD_UPDATE_HINT),
          }
        : status
      syncSystemUpdatePhase(versionStatus.value)
      cacheVersionStatus(versionStatus.value)
      return versionStatus.value
    } catch (error) {
      versionStatus.value = buildUpdateErrorStatus(versionStatus.value, error)
      cacheVersionStatus(versionStatus.value)
      return versionStatus.value
    } finally {
      loadingVersionStatus.value = false
      versionStatusLoadPromise = null
    }
  })()

  return versionStatusLoadPromise
}

function applyUpdateCapability(capability: SystemUpdateCapabilityResponse) {
  updateSupported.value = capability.supported
  rollbackAvailable.value = capability.supported && capability.rollback_available
  updateStrategy.value = capability.update_strategy || capability.strategy || 'manual'
  updateCapabilityMessage.value = capability.message || null
  dockerUpdateCommand.value = capability.docker_update_command || null
}

function updateUnsupportedMessage(fallback: MessageKey = MANUAL_UPDATE_HINT): string {
  return updateCapabilityMessage.value || t(fallback)
}

function syncSystemUpdatePhase(status: CheckUpdateResponse | null) {
  if (systemUpdatePhase.value === 'reconnecting') return
  if (systemUpdatePhase.value === 'restart') {
    if (!preparedUpdateVersion.value) {
      systemUpdatePhase.value = 'download'
    }
    return
  }
  if (!status?.has_update) {
    systemUpdatePhase.value = 'download'
    preparedUpdateVersion.value = null
  }
}

function handleVersionRefresh() {
  void loadVersionStatus(true)
}

function openVersionReleasePage() {
  if (versionStatus.value?.release_url) {
    window.open(versionStatus.value.release_url, '_blank', 'noopener,noreferrer')
  }
}

function buildUpdateInfoFromRelease(release: ReleaseEntry): CheckUpdateResponse {
  const currentVersion =
    versionStatus.value?.current_version ||
    updateInfo.value?.current_version ||
    __APP_VERSION__ ||
    ''
  const canSelfUpdate = updateSupported.value
  return {
    current_version: currentVersion,
    latest_version: release.version,
    has_update: !release.is_current,
    updatable: canSelfUpdate && !release.is_current && release.updatable,
    update_blocker: release.is_current
      ? t('update.error.alreadyCurrent')
      : !canSelfUpdate
        ? updateUnsupportedMessage(SOURCE_BUILD_RELEASE_HINT)
      : release.update_blocker,
    release_url: release.release_url,
    release_notes: release.release_notes,
    published_at: release.published_at,
    error: null,
  }
}

function openReleaseUpdateDialog(release: ReleaseEntry) {
  updateDialogMode.value = 'selected'
  updateInfo.value = buildUpdateInfoFromRelease(release)
  if (systemUpdatePhase.value !== 'reconnecting') {
    systemUpdatePhase.value = 'download'
    preparedUpdateVersion.value = null
  }
  showUpdateDialog.value = true
}

async function handleApplySystemUpdate() {
  if (applyingSystemUpdate.value) return
  applyingSystemUpdate.value = true
  try {
    const capability = await adminApi.getSystemUpdateCapability()
    applyUpdateCapability(capability)
    if (!capability.supported) {
      showError(
        updateUnsupportedMessage('update.error.unsupported'),
        t('update.error.unsupportedTitle')
      )
      return
    }

    if (systemUpdatePhase.value === 'download') {
      const targetStatus = updateInfo.value || versionStatus.value
      if (targetStatus?.has_update && targetStatus.updatable === false) {
        showError(
          targetStatus.update_blocker || t('update.error.notUpdatable'),
          t('update.error.cannotUpdateOnline')
        )
        return
      }
      const targetVersion = updateInfo.value?.latest_version || versionStatus.value?.latest_version || null
      updateTaskStatus.value = null
      startUpdateStatusPolling()
      try {
        const result = await adminApi.prepareSystemUpdate(targetVersion)
        const finalStatus = await waitForPreparedUpdate()
        preparedUpdateVersion.value = targetVersion
        systemUpdatePhase.value = 'restart'
        success(finalStatus.output || result.message || t('update.success.prepared'))
      } finally {
        stopUpdateStatusPolling()
        void refreshUpdateTaskStatus()
      }
      return
    }

    const result = await adminApi.applySystemUpdate(preparedUpdateVersion.value)
    success(result.message || t('update.success.restartStarted'))
    systemUpdatePhase.value = 'reconnecting'
    reconnectMessage.value = t('update.reconnect.restarting')
    showUpdateDialog.value = true
    applyingSystemUpdate.value = false
    await pollHealthUntilReady()
  } catch (err) {
    const fallback = systemUpdatePhase.value === 'download' ? t('update.error.downloadFailed') : t('update.error.restartFailed')
    showError(parseApiError(err, fallback))
  } finally {
    applyingSystemUpdate.value = false
  }
}

async function handleRollback() {
  if (rollingBack.value) return
  rollingBack.value = true
  try {
    const result = await adminApi.rollbackSystemUpdate()
    success(result.message || t('update.success.rollbackStarted'))
    systemUpdatePhase.value = 'reconnecting'
    reconnectMessage.value = t('update.reconnect.rollback')
    showUpdateDialog.value = true
    rollingBack.value = false
    await pollHealthUntilReady()
  } catch (err) {
    showError(parseApiError(err, t('update.error.rollbackFailed')))
  } finally {
    rollingBack.value = false
  }
}

async function pollHealthUntilReady() {
  const maxAttempts = 60
  const intervalMs = 2000

  for (let i = 0; i < maxAttempts; i++) {
    if (i < 3) {
      reconnectMessage.value = i === 0
        ? t('update.reconnect.restarting')
        : t('update.reconnect.restartingWithSeconds', { seconds: i * 2 })
      await new Promise(r => setTimeout(r, intervalMs))
      continue
    }

    const elapsed = i * 2
    reconnectMessage.value = t('update.reconnect.waitingWithSeconds', { seconds: elapsed })
    try {
      const resp = await fetch('/_gateway/health', {
        method: 'GET',
        signal: AbortSignal.timeout(3000),
      })
      if (resp.ok) {
        reconnectMessage.value = t('update.reconnect.ready')
        await new Promise(r => setTimeout(r, 500))
        window.location.replace(buildFreshReloadUrl())
        return
      }
    } catch {
      // expected while service is down
    }

    // After 30 seconds, start checking if the task actually failed
    if (i > 15) {
      try {
        const status = await adminApi.getUpdateStatus()
        if (status.phase === 'failed' && status.error) {
          reconnectMessage.value = t('update.error.updateFailedWithReason', { reason: status.error })
          systemUpdatePhase.value = 'download'
          return
        }
      } catch {
        // service still down, continue polling
      }
    }

    await new Promise(r => setTimeout(r, intervalMs))
  }

  reconnectMessage.value = t('update.reconnect.timeout')
  systemUpdatePhase.value = 'download'
}

function buildFreshReloadUrl(): string {
  const url = new URL(window.location.href)
  url.searchParams.set('__aether_reload', Date.now().toString())
  return url.toString()
}

function showDebugUpdateDialog() {
  const currentVersion = versionStatus.value?.current_version || __APP_VERSION__ || '0.7.0-rc28'
  updateDialogMode.value = 'latest'
  updateInfo.value = {
    current_version: currentVersion,
    latest_version: 'v0.7.0-rc99',
    has_update: true,
    release_url: 'https://github.com/fawney19/Aether/releases',
    release_notes: [
      "### What's Changed",
      '- 调整版本更新提示样式',
      '- 修复开发分支版本误判',
      '- 统一版本号显示格式',
    ].join('\n'),
    published_at: new Date().toISOString(),
    updatable: true,
    update_blocker: null,
    error: null,
  }
  systemUpdatePhase.value = 'download'
  preparedUpdateVersion.value = null
  showUpdateDialog.value = true
}

function showDebugVersionStatus(hasUpdate = true) {
  const currentVersion = versionStatus.value?.current_version || __APP_VERSION__ || '0.7.0-rc28'
  versionStatus.value = {
    current_version: currentVersion,
    latest_version: hasUpdate ? 'v0.7.0-rc99' : currentVersion,
    has_update: hasUpdate,
    release_url: hasUpdate ? 'https://github.com/fawney19/Aether/releases' : null,
    release_notes: hasUpdate
      ? [
        "### What's Changed",
        '- 调整版本更新提示样式',
        '- 修复开发分支版本误判',
        '- 统一版本号显示格式',
      ].join('\n')
      : null,
    published_at: hasUpdate ? new Date().toISOString() : null,
    updatable: hasUpdate,
    update_blocker: null,
    error: null,
  }
  systemUpdatePhase.value = 'download'
  preparedUpdateVersion.value = null
}

// 检查更新
async function checkForUpdate() {
  // 只有管理员才检查更新
  if (!authStore.canOperateAdmin) return

  // 同一会话内只检查一次
  const sessionKey = 'aether_update_checked'
  if (sessionStorage.getItem(sessionKey)) {
    applyCachedVersionStatus()
    return
  }
  sessionStorage.setItem(sessionKey, '1')

  const result = versionStatus.value ?? await loadVersionStatus()
  if (result?.has_update && result.latest_version) {
    if (shouldShowUpdatePrompt(result.latest_version)) {
      updateDialogMode.value = 'latest'
      updateInfo.value = result
      showUpdateDialog.value = true
    }
  }
}

function syncAuthNotice() {
  authStore.syncToken()
  showAuthError.value = !!authStore.user && !authStore.token
}

function handleStorageChange(event: StorageEvent) {
  if (event.key === null || event.key === 'access_token') {
    syncAuthNotice()
  }
}

function handleVisibilityChange() {
  if (!document.hidden) {
    syncAuthNotice()
  }
}

watch(
  () => [authStore.user, authStore.token] as const,
  () => {
    showAuthError.value = !!authStore.user && !authStore.token
    if (authStore.user && authStore.token) {
      void loadRequiredAnnouncements()
    } else {
      requiredAnnouncements.value = []
    }
  },
  { immediate: true }
)

async function loadRequiredAnnouncements() {
  if (!authStore.user || !authStore.token) return
  if (requiredAnnouncementsPromise) return requiredAnnouncementsPromise

  requiredAnnouncementsPromise = (async () => {
    try {
      const response = await announcementApi.getRequiredUnreadAnnouncements()
      requiredAnnouncements.value = response.items.filter(item => item.requires_ack && !item.is_read)
    } catch {
      requiredAnnouncements.value = []
    } finally {
      requiredAnnouncementsPromise = null
    }
  })()

  return requiredAnnouncementsPromise
}

function renderRequiredAnnouncement(content: string): string {
  return sanitizeMarkdown(marked(content || '') as string)
}

function formatRequiredAnnouncementDate(value: string): string {
  return new Date(value).toLocaleString(locale.value)
}

async function acknowledgeRequiredAnnouncement() {
  const announcement = currentRequiredAnnouncement.value
  if (!announcement) return
  acknowledgingRequiredAnnouncement.value = true
  try {
    await announcementApi.markAsRead(announcement.id)
    requiredAnnouncements.value = requiredAnnouncements.value.slice(1)
  } finally {
    acknowledgingRequiredAnnouncement.value = false
  }
}

onMounted(() => {
  window.addEventListener('storage', handleStorageChange)
  document.addEventListener('visibilitychange', handleVisibilityChange)
  syncAuthNotice()
  applyCachedVersionStatus()

  // 管理员预加载模块状态（路由守卫会按需加载，这里提前加载以避免菜单闪烁）
  if (authStore.canAccessAdmin && !moduleStore.loaded && !moduleStore.loading) {
    void moduleStore.fetchModules().catch(() => {
      // 路由守卫会在需要模块状态时按需处理失败场景。
    })
  }
  void loadRequiredAnnouncements()

  // 延迟检查更新，避免 GitHub Releases 检查和首屏业务数据争抢资源。
  updateCheckTimer = window.setTimeout(() => {
    updateCheckTimer = null
    void checkForUpdate()
  }, 2000)

  if (import.meta.env.DEV) {
    window.__aetherShowUpdateDialog = showDebugUpdateDialog
    window.__aetherMockVersionStatus = showDebugVersionStatus
  }
})

onUnmounted(() => {
  window.removeEventListener('storage', handleStorageChange)
  document.removeEventListener('visibilitychange', handleVisibilityChange)
  if (updateCheckTimer !== null) {
    window.clearTimeout(updateCheckTimer)
    updateCheckTimer = null
  }
  stopUpdateStatusPolling()
  if (import.meta.env.DEV && window.__aetherShowUpdateDialog === showDebugUpdateDialog) {
    delete window.__aetherShowUpdateDialog
  }
  if (import.meta.env.DEV && window.__aetherMockVersionStatus === showDebugVersionStatus) {
    delete window.__aetherMockVersionStatus
  }
})

async function handleRelogin() {
  showAuthError.value = false
  await authStore.logout()
  await router.push('/')
}

async function handleLogout() {
  await authStore.logout()
  await router.push('/')
}

function isNavActive(href: string) {
  if (href === '/dashboard' || href === '/admin/dashboard') {
    return route.path === href
  }
  return route.path === href || route.path.startsWith(`${href}/`)
}

function prefetchNavigationItem(href: string) {
  prefetchAdminNavigationTarget(href)
}

const navigation = computed(() => {
  return buildNavigation({
    canAccessAdmin: authStore.canAccessAdmin,
    modules: moduleStore.modules,
    isModuleActive: moduleStore.isActive,
    t,
  })
})

const currentRoleLabel = computed(() => {
  if (authStore.isAdmin) return t('auth.role.admin')
  if (authStore.isAuditAdmin) return t('auth.role.auditAdmin')
  return t('auth.role.user')
})

const breadcrumbs = computed(() => buildBreadcrumbs({
  route,
  navigation: navigation.value,
  modules: moduleStore.modules,
  isNavActive,
  t,
}))

// Styling Classes (Editorial)
const sidebarClasses = computed(() => {
    // Fixed width, border right, background match
    return `w-[260px] flex flex-col hidden lg:flex border-r border-[#3d3929]/5 dark:border-white/5 bg-[#faf9f5] dark:bg-[#1e1c19] h-screen sticky top-0`
})

const contentClasses = computed(() => {
    return `flex-1 min-w-0 bg-[#faf9f5] dark:bg-[#191714] text-[#3d3929] dark:text-[#d4a27f]`
})

const mainClasses = computed(() => {
    // 移动端需要 pt-24 来避开固定头部（约69px）+ 额外间距
    // 桌面端内容在 sticky header 下方，但需要一些内边距让内容不紧贴
    return `pt-24 lg:pt-6`
})

</script>

<style scoped>
.scrollbar-none::-webkit-scrollbar { display: none; }
.scrollbar-none { -ms-overflow-style: none; scrollbar-width: none; }
</style>
