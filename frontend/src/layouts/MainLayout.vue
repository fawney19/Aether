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
              Aether
            </h1>
            <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1.5 font-medium tracking-wide">Multi Private Gateway</span>
          </div>
        </RouterLink>
      </div>

      <!-- NAVIGATION -->
      <div class="flex-1 overflow-y-auto py-2 scrollbar-none">
        <SidebarNav
          :items="navigation"
          :is-active="isNavActive"
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
              <span class="text-[10px] opacity-50 leading-none mt-1.5 text-muted-foreground">{{ authStore.user?.role === 'admin' ? '管理员' : '用户' }}</span>
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
      <header class="lg:hidden fixed top-0 left-0 right-0 z-50 border-b border-[#cc785c]/10 dark:border-[rgba(227,224,211,0.12)] bg-[#fafaf7]/90 dark:bg-[#191714]/95 backdrop-blur-xl transition-all">
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
                  Aether
                </h1>
                <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1.5 font-medium tracking-wide">Multi Private Gateway</span>
              </div>
            </RouterLink>

            <!-- Right Actions -->
            <div class="flex items-center gap-3">
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
            class="border-t border-[#cc785c]/10 dark:border-[rgba(227,224,211,0.12)] bg-[#fafaf7]/95 dark:bg-[#191714]/98 backdrop-blur-xl"
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
                      <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1">{{ authStore.user?.role === 'admin' ? '管理员' : '用户' }}</span>
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
            <span>{{ currentSectionName }}</span>
            <ChevronRight class="w-3 h-3 opacity-50" />
            <span class="text-foreground font-medium">{{ currentPageName }}</span>
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

    <!-- 更新提示弹窗 -->
    <UpdateDialog
      v-if="updateInfo"
      v-model="showUpdateDialog"
      :current-version="updateInfo.current_version"
      :latest-version="updateInfo.latest_version || ''"
      :release-url="updateInfo.release_url"
      :release-notes="updateInfo.release_notes"
      :published-at="updateInfo.published_at"
    />
  </AppShell>
</template>

<script setup lang="ts">
import { computed, ref, watch, onMounted, onUnmounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { useDarkMode } from '@/composables/useDarkMode'
import { isDemoMode } from '@/config/demo'
import { adminApi, type CheckUpdateResponse } from '@/api/admin'
import Button from '@/components/ui/button.vue'
import AppShell from '@/components/layout/AppShell.vue'
import SidebarNav from '@/components/layout/SidebarNav.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import UpdateDialog from '@/components/common/UpdateDialog.vue'
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
  Box,
  LogOut,
  SunMoon,
  ChevronRight,
  Megaphone,
  Menu,
  X,
  Mail,
} from 'lucide-vue-next'
import GithubIcon from '@/components/icons/GithubIcon.vue'

const router = useRouter()
const route = useRoute()
const authStore = useAuthStore()
const { themeMode, toggleDarkMode } = useDarkMode()
const isDemo = computed(() => isDemoMode())

const showAuthError = ref(false)
const mobileMenuOpen = ref(false)
let authCheckInterval: number | null = null

// 更新检查相关
const showUpdateDialog = ref(false)
const updateInfo = ref<CheckUpdateResponse | null>(null)

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

// 检查更新
async function checkForUpdate() {
  // 只有管理员才检查更新
  if (authStore.user?.role !== 'admin') return

  // 同一会话内只检查一次
  const sessionKey = 'aether_update_checked'
  if (sessionStorage.getItem(sessionKey)) return
  sessionStorage.setItem(sessionKey, '1')

  try {
    const result = await adminApi.checkUpdate()
    if (result.has_update && result.latest_version) {
      if (shouldShowUpdatePrompt(result.latest_version)) {
        updateInfo.value = result
        showUpdateDialog.value = true
      }
    }
  } catch {
    // 静默失败，不影响用户体验
  }
}

onMounted(() => {
  authCheckInterval = setInterval(() => {
    if (authStore.user && !authStore.token) {
      showAuthError.value = true
    }
  }, 5000)

  // 延迟检查更新，避免影响页面加载
  setTimeout(() => {
    checkForUpdate()
  }, 2000)
})

onUnmounted(() => {
  if (authCheckInterval) {
    clearInterval(authCheckInterval)
    authCheckInterval = null
  }
})

function handleRelogin() {
  showAuthError.value = false
  router.push('/').then(() => {
    authStore.logout()
  })
}

function handleLogout() {
  authStore.logout()
  router.push('/')
}

function isNavActive(href: string) {
  if (href === '/dashboard' || href === '/admin/dashboard') {
    return route.path === href
  }
  return route.path === href || route.path.startsWith(`${href}/`)
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
        { name: '访问令牌', href: '/dashboard/management-tokens', icon: KeyRound },
      ]
    },
    {
      title: '账户',
      items: [
         { name: '使用统计', href: '/dashboard/usage', icon: BarChart3 },
      ]
    }
  ]

  const adminNavigation = [
     {
      title: '概览',
      items: [
        { name: '仪表盘', href: '/admin/dashboard', icon: Home },
        { name: '健康监控', href: '/admin/health-monitor', icon: Activity },
      ]
    },
    {
      title: '管理',
      items: [
        { name: '用户管理', href: '/admin/users', icon: Users },
        { name: '提供商', href: '/admin/providers', icon: FolderTree },
        { name: '模型管理', href: '/admin/models', icon: Layers },
        { name: '独立密钥', href: '/admin/keys', icon: Key },
        { name: '访问令牌', href: '/admin/management-tokens', icon: KeyRound },
        { name: '使用记录', href: '/admin/usage', icon: BarChart3 },
      ]
    },
    {
      title: '系统',
      items: [
        { name: '公告管理', href: '/admin/announcements', icon: Megaphone },
        { name: '缓存监控', href: '/admin/cache-monitoring', icon: Gauge },
        { name: 'IP 安全', href: '/admin/ip-security', icon: Shield },
        { name: '审计日志', href: '/admin/audit-logs', icon: AlertTriangle },
        { name: '邮件配置', href: '/admin/email', icon: Mail },
        { name: 'LDAP 配置', href: '/admin/ldap', icon: Shield },
        { name: '系统设置', href: '/admin/system', icon: Cog },
      ]
    }
  ]

  return authStore.user?.role === 'admin' ? adminNavigation : baseNavigation
})

// Dynamic Header Title
const currentSectionName = computed(() => {
    // Special case: personal settings page accessed by admin
    if (route.path === '/dashboard/settings') {
      return '账户'
    }
    // Find the group that contains the active item
    for (const group of navigation.value) {
      const hasActiveItem = group.items.some(item => isNavActive(item.href))
      if (hasActiveItem) {
        return group.title || ''
      }
    }
    return ''
})

const currentPageName = computed(() => {
    // Special case: personal settings page accessed by admin
    if (route.path === '/dashboard/settings') {
      return '个人设置'
    }
    // Flatten navigation to find matching item name
    const allItems = navigation.value.flatMap(group => group.items)
    const active = allItems.find(item => isNavActive(item.href))
    return active ? active.name : route.name?.toString() || '仪表盘'
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
