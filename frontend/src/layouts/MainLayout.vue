<template>
  <AppShell
    :show-notice="showAuthError"
    :main-class="mainClasses"
    :sidebar-class="sidebarClasses"
    :content-class="contentClasses"
  >
    <!-- GLOBAL TEXTURE (Paper Noise) -->
    <div
      class="absolute inset-0 pointer-events-none z-0 opacity-[0.03] mix-blend-multiply fixed"
      :style="{ backgroundImage: `url(\&quot;data:image/svg+xml,%3Csvg viewBox='0 0 200 200' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noise'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.8' numOctaves='3' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noise)'/%3E%3C/svg%3E\&quot;)` }"
    />

    <template #notice>
      <div class="flex w-full max-w-3xl items-center justify-between rounded-3xl bg-orange-500 px-6 py-3 text-white shadow-2xl ring-1 ring-white/30">
        <div class="flex items-center gap-3">
          <AlertTriangle class="h-5 w-5" />
          <span>认证已过期，请重新登录</span>
        </div>
        <Button
          variant="outline"
          size="sm"
          class="border-white/60 text-white hover:bg-white/10"
          @click="handleRelogin"
        >
          重新登录
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
              title="个人设置"
            >
              <Settings class="w-4 h-4" />
            </RouterLink>
            <button
              class="p-1.5 rounded-md text-muted-foreground hover:text-red-500 transition-colors"
              title="退出登录"
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
              <button
                class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
                :title="themeMode === 'system' ? '跟随系统' : themeMode === 'dark' ? '深色模式' : '浅色模式'"
                @click="toggleDarkMode"
              >
                <SunMoon
                  v-if="themeMode === 'system'"
                  class="h-4 w-4"
                />
                <SunMedium
                  v-else-if="themeMode === 'light'"
                  class="h-4 w-4"
                />
                <Moon
                  v-else
                  class="h-4 w-4"
                />
              </button>
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
          enter-active-class="transition-all duration-300 ease-out overflow-hidden"
          enter-from-class="opacity-0 max-h-0"
          enter-to-class="opacity-100 max-h-[500px]"
          leave-active-class="transition-all duration-200 ease-in overflow-hidden"
          leave-from-class="opacity-100 max-h-[500px]"
          leave-to-class="opacity-0 max-h-0"
        >
          <div
            v-if="mobileMenuOpen"
            class="border-t border-[var(--shell-border)] bg-[var(--shell-glass)] backdrop-blur-xl"
          >
            <div class="mx-auto max-w-7xl px-6 py-4">
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
                      @mouseenter="prefetchNavigationItem(item.href)"
                      @focus="prefetchNavigationItem(item.href)"
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
                      @click="mobileMenuOpen = false"
                    >
                      <Settings class="w-4 h-4" />
                    </RouterLink>
                    <button
                      class="p-2 rounded-lg text-muted-foreground hover:text-red-500 transition-colors"
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
          <span>演示模式</span>
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
          <!-- Theme Toggle -->
          <button
            class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
            :title="themeMode === 'system' ? '跟随系统' : themeMode === 'dark' ? '深色模式' : '浅色模式'"
            @click="toggleDarkMode"
          >
            <SunMoon
              v-if="themeMode === 'system'"
              class="h-4 w-4"
            />
            <SunMedium
              v-else-if="themeMode === 'light'"
              class="h-4 w-4"
            />
            <Moon
              v-else
              class="h-4 w-4"
            />
          </button>
          <!-- GitHub Link -->
          <a
            href="https://github.com/fawney19/Aether"
            target="_blank"
            rel="noopener noreferrer"
            class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
            title="GitHub 仓库"
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
      title="必读公告"
      description="请确认后继续使用"
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
          {{ acknowledgingRequiredAnnouncement ? '确认中...' : '确认已读' }}
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
import { useDarkMode } from '@/composables/useDarkMode'
import { useSiteInfo } from '@/composables/useSiteInfo'
import { isDemoMode } from '@/config/demo'
import { announcementApi, type Announcement } from '@/api/announcements'
import Button from '@/components/ui/button.vue'
import { Dialog } from '@/components/ui'
import AppShell from '@/components/layout/AppShell.vue'
import SidebarNav from '@/components/layout/SidebarNav.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import UpdateDialog from '@/components/common/UpdateDialog.vue'
import VersionButton from '@/components/common/VersionButton.vue'
import { useSystemUpdate } from '@/composables/useSystemUpdate'
import {
  Home,
  Users,
  Key,
  KeyRound,
  BarChart3,
  Cog,
  Settings,
  Activity,
  Shield,
  AlertTriangle,
  SunMedium,
  Moon,
  Gauge,
  Layers,
  FolderTree,
  Database,
  Box,
  LogOut,
  SunMoon,
  ChevronRight,
  Megaphone,
  Wallet,
  CreditCard,
  Package,
  Gift,
  Menu,
  X,
  Puzzle,
  Zap,
  FileUp,
  Send,
  Server,
  SlidersHorizontal,
  type LucideIcon,
} from 'lucide-vue-next'

import GithubIcon from '@/components/icons/GithubIcon.vue'
import { BUILTIN_TOOL_BREADCRUMBS } from '@/config/builtin-tools'
import { prefetchAdminNavigationTarget } from '@/utils/adminNavigationPrefetch'
import { sanitizeMarkdown } from '@/utils/sanitize'

const router = useRouter()
const route = useRoute()
const authStore = useAuthStore()
const moduleStore = useModuleStore()
const { themeMode, toggleDarkMode } = useDarkMode()
const { siteName, siteSubtitle } = useSiteInfo()
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

const {
  showUpdateDialog,
  updateInfo,
  versionStatus,
  loadingVersionStatus,
  applyingSystemUpdate,
  updateSupported,
  updateStrategy,
  dockerUpdateCommand,
  reconnectMessage,
  rollbackAvailable,
  rollingBack,
  systemUpdatePhase,
  updateProgressText,
  updateProgressPercent,
  updateDialogTitle,
  updateDialogVersionLabel,
  updateDialogReleaseLinkLabel,
  loadVersionStatus,
  handleVersionRefresh,
  openVersionReleasePage,
  openReleaseUpdateDialog,
  handleApplySystemUpdate,
  handleRollback,
  checkForUpdate,
  showDebugUpdateDialog,
  showDebugVersionStatus,
} = useSystemUpdate()

// 路由变化时自动关闭移动端菜单
watch(() => route.path, () => {
  mobileMenuOpen.value = false
})

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
  try {
    const response = await announcementApi.getRequiredUnreadAnnouncements()
    requiredAnnouncements.value = response.items.filter(item => item.requires_ack && !item.is_read)
  } catch {
    requiredAnnouncements.value = []
  }
}

function renderRequiredAnnouncement(content: string): string {
  return sanitizeMarkdown(marked(content || '') as string)
}

function formatRequiredAnnouncementDate(value: string): string {
  return new Date(value).toLocaleString('zh-CN')
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

  // 管理员预加载模块状态（路由守卫会按需加载，这里提前加载以避免菜单闪烁）
  if (authStore.canAccessAdmin && !moduleStore.loaded && !moduleStore.loading) {
    moduleStore.fetchModules()
  }
  void loadVersionStatus()
  void loadRequiredAnnouncements()

  // 延迟检查更新，避免影响页面加载
  setTimeout(() => {
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

// Navigation Data
const navigation = computed(() => {
  const baseNavigation = [
    {
      title: '概览',
      items: [
        { name: '仪表盘', href: '/dashboard', icon: Home },
        { name: '健康监控', href: '/dashboard/endpoint-status', icon: Activity },
      ]
    },
    {
      title: '资源',
      items: [
        { name: '模型目录', href: '/dashboard/models', icon: Box },
        { name: 'API 密钥', href: '/dashboard/api-keys', icon: Key },
      ]
    },
    {
      title: '账户',
      items: [
         { name: '钱包中心', href: '/dashboard/wallet', icon: Wallet },
         { name: '套餐中心', href: '/dashboard/billing', icon: Package },
         ...(moduleStore.isActive('referral') ? [{ name: '我的邀请', href: '/dashboard/referral', icon: Gift }] : []),
         { name: '使用统计', href: '/dashboard/usage', icon: BarChart3 },
      ]
    }
  ]

  // 系统菜单项（静态部分）
  const systemItems: { name: string; href: string; icon: LucideIcon }[] = [
    { name: '公告管理', href: '/admin/announcements', icon: Megaphone },
    { name: '缓存监控', href: '/admin/cache-monitoring', icon: Gauge },
  ]

  // 动态添加已激活模块的菜单项
  // 图标映射
  const iconMap: Record<string, LucideIcon> = {
    Key,
    KeyRound,
    FileUp,
    Shield,
    Puzzle,
    Server,
    Send,
    SlidersHorizontal,
    CreditCard,
    Gift,
  }

  const activeModuleItems = (group: string) =>
    Object.values(moduleStore.modules)
      .filter(m => m.active && m.admin_route && m.admin_menu_group === group)
      .sort((a, b) => a.admin_menu_order - b.admin_menu_order)
      .map(m => ({
        name: m.display_name,
        href: m.admin_route ?? '',
        icon: iconMap[m.admin_menu_icon || ''] || Puzzle
      }))

  systemItems.push(...activeModuleItems('system'))

  // 模块管理和系统设置放在最后
  systemItems.push({ name: '模块管理', href: '/admin/modules', icon: Puzzle })
  systemItems.push({ name: '系统设置', href: '/admin/system', icon: Cog })

  const adminNavigation = [
     {
      title: '概览',
      items: [
        { name: '仪表盘', href: '/admin/dashboard', icon: Home },
        { name: '健康监控', href: '/admin/health-monitor', icon: Activity },
        { name: '用户统计', href: '/admin/user-stats', icon: BarChart3 },
        { name: '成本分析', href: '/admin/cost-analysis', icon: Gauge },
        { name: '性能分析', href: '/admin/performance-analysis', icon: Activity },
      ]
    },
    {
      title: '管理',
      items: [
        { name: '用户管理', href: '/admin/users', icon: Users },
        { name: '提供商', href: '/admin/providers', icon: FolderTree },
        { name: '模型管理', href: '/admin/models', icon: Layers },
        { name: '调度策略', href: '/admin/routing', icon: SlidersHorizontal },
        { name: '号池管理', href: '/admin/pool', icon: Database },
        { name: '独立密钥', href: '/admin/keys', icon: Key },
        { name: '钱包管理', href: '/admin/wallets', icon: Wallet },
        { name: '套餐管理', href: '/admin/billing-plans', icon: Package },
        ...activeModuleItems('management'),
        { name: '异步任务', href: '/admin/async-tasks', icon: Zap },
        { name: '使用记录', href: '/admin/usage', icon: BarChart3 },
      ]
    },
    {
      title: '系统',
      items: systemItems
    }
  ]

  return authStore.canAccessAdmin ? adminNavigation : baseNavigation
})

const currentRoleLabel = computed(() => {
  if (authStore.isAdmin) return '管理员'
  if (authStore.isAuditAdmin) return '审计管理员'
  return '用户'
})

// Breadcrumbs
interface BreadcrumbItem {
  label: string
  href?: string
}

const breadcrumbs = computed((): BreadcrumbItem[] => {
  // Special case: personal settings page accessed by admin
  if (route.path === '/dashboard/settings') {
    return [
      { label: '账户' },
      { label: '个人设置' }
    ]
  }

  // Special case: module config pages (e.g., /admin/ldap)
  if (route.meta?.module) {
    const moduleName = route.meta.module as string
    const moduleStatus = moduleStore.modules[moduleName]
    const displayName = moduleStatus?.display_name || moduleName
    return [
      { label: '系统' },
      { label: '模块管理', href: '/admin/modules' },
      { label: displayName }
    ]
  }

  // Special case: built-in tools under module management
  if (BUILTIN_TOOL_BREADCRUMBS[route.path]) {
    return [
      { label: '系统' },
      { label: '模块管理', href: '/admin/modules' },
      { label: BUILTIN_TOOL_BREADCRUMBS[route.path] }
    ]
  }

  // Find section and page from navigation
  for (const group of navigation.value) {
    const activeItem = group.items.find(item => isNavActive(item.href))
    if (activeItem) {
      return [
        { label: group.title || '' },
        { label: activeItem.name }
      ]
    }
  }

  // Special case: module pages not in navigation (module not active)
  // Check if current path matches a module's admin_route
  const currentModule = Object.values(moduleStore.modules).find(
    m => m.admin_route && route.path === m.admin_route
  )
  if (currentModule) {
    return [
      { label: '模块管理', href: '/admin/modules' },
      { label: currentModule.display_name }
    ]
  }

  return [{ label: '仪表盘' }]
})

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
