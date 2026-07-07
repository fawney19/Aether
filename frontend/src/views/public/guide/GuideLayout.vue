<template>
  <AppShell
    :main-class="mainClasses"
    :sidebar-class="sidebarClasses"
    :content-class="contentClasses"
  >
    <!-- GLOBAL TEXTURE (Paper Noise) -->
    <div
      class="absolute inset-0 pointer-events-none z-0 opacity-[0.03] mix-blend-multiply fixed"
      :style="{ backgroundImage: `url(\&quot;data:image/svg+xml,%3Csvg viewBox='0 0 200 200' xmlns='http://www.w3.org/2000/svg'%3E%3Cfilter id='noise'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.8' numOctaves='3' stitchTiles='stitch'/%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23noise)'/%3E%3C/svg%3E\&quot;)` }"
    />

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
        <nav class="w-full px-3">
          <div class="space-y-0.5">
            <template
              v-for="item in resolvedGuideNavItems"
              :key="item.id"
            >
              <RouterLink
                :to="item.path"
                class="group relative flex items-center justify-between px-2.5 py-2 rounded-lg transition-all duration-200"
                :class="[
                  isNavActive(item.path)
                    ? 'bg-primary/10 text-primary font-medium'
                    : 'text-muted-foreground hover:text-foreground hover:bg-muted/50'
                ]"
              >
                <div class="flex items-center gap-2.5">
                  <component
                    :is="item.icon"
                    class="h-4 w-4 transition-colors duration-200"
                    :class="isNavActive(item.path) ? 'text-primary' : 'text-muted-foreground/70 group-hover:text-foreground'"
                    :stroke-width="isNavActive(item.path) ? 2 : 1.75"
                  />
                  <span class="text-[13px] tracking-tight">{{ item.name }}</span>
                </div>
                <div
                  v-if="isNavActive(item.path)"
                  class="w-1 h-1 rounded-full bg-primary"
                />
              </RouterLink>

              <!-- 子导航 -->
              <div
                v-if="item.subItems && isNavActive(item.path)"
                class="ml-7 space-y-0.5 mt-0.5 mb-2"
              >
                <a
                  v-for="sub in item.subItems"
                  :key="sub.hash"
                  :href="sub.hash"
                  class="flex items-center gap-2 px-2.5 py-1.5 rounded-md text-[12px] transition-colors"
                  :class="activeHash === sub.hash
                    ? 'text-primary font-medium'
                    : 'text-muted-foreground/70 hover:text-foreground hover:bg-muted/30'"
                  @click.prevent="scrollToHash(sub.hash)"
                >
                  <span
                    class="w-1 h-1 rounded-full flex-shrink-0"
                    :class="activeHash === sub.hash ? 'bg-primary' : 'bg-muted-foreground/30'"
                  />
                  {{ sub.name }}
                </a>
              </div>
            </template>
          </div>
        </nav>
      </div>

      <!-- FOOTER (Base URL) -->
      <div class="p-4 border-t border-[#3d3929]/5 dark:border-white/5">
        <label class="block text-[10px] font-semibold text-muted-foreground/70 uppercase tracking-[0.1em] mb-2">
          Base URL
        </label>
        <input
          v-model="baseUrl"
          type="text"
          class="w-full px-3 py-2 text-sm rounded-lg border border-[#3d3929]/5 dark:border-white/5 bg-white/50 dark:bg-white/5 text-[#191919] dark:text-white placeholder-[#91918d] focus:outline-none focus:ring-2 focus:ring-[#cc785c]/30 transition"
          placeholder="https://your-aether.com"
        >
      </div>
    </template>

    <template #header>
      <!-- Mobile Header -->
      <header class="lg:hidden fixed top-0 left-0 right-0 z-50 border-b border-[var(--shell-border)] bg-[var(--shell-glass)] backdrop-blur-xl transition-all">
        <div class="mx-auto max-w-7xl px-6 py-4">
          <div class="flex items-center justify-between">
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

            <div class="flex items-center gap-3">
              <ThemeModeButton />
              <LanguageSwitcher />
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
            </div>
          </div>
        </Transition>
      </header>

      <!-- Desktop Page Header -->
      <header class="hidden lg:flex h-16 px-8 items-center justify-between shrink-0 border-b border-[#3d3929]/5 dark:border-white/5 sticky top-0 z-40 backdrop-blur-md bg-[#faf9f5]/90 dark:bg-[#191714]/90">
        <div class="flex flex-col gap-0.5">
          <div class="flex items-center gap-2 text-sm text-muted-foreground">
            <RouterLink
              to="/guide"
              class="hover:text-foreground transition-colors"
            >
              {{ t('guide.title') }}
            </RouterLink>
            <template v-if="currentNavItem && currentNavItem.id !== 'overview'">
              <ChevronRight class="w-3 h-3 opacity-50" />
              <span class="text-foreground font-medium">
                {{ currentNavItem.name }}
              </span>
            </template>
          </div>
        </div>

        <div class="flex items-center gap-2">
          <ThemeModeButton />
          <a
            href="https://github.com/fawney19/Aether"
            target="_blank"
            rel="noopener noreferrer"
            class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
            :title="t('common.githubRepository')"
          >
            <GithubIcon class="h-4 w-4" />
          </a>
          <LanguageSwitcher />
        </div>
      </header>
    </template>

    <article
      class="max-w-4xl mx-auto pb-24"
      @click="onArticleClick"
    >
      <RouterView
        v-slot="{ Component }"
      >
        <transition
          name="fade"
          mode="out-in"
        >
          <component
            :is="Component"
            :base-url="baseUrl"
            class="literary-content"
          />
        </transition>
      </RouterView>
    </article>

    <!-- Image Lightbox -->
    <Teleport to="body">
      <Transition
        enter-active-class="transition duration-200 ease-out"
        enter-from-class="opacity-0"
        enter-to-class="opacity-100"
        leave-active-class="transition duration-150 ease-in"
        leave-from-class="opacity-100"
        leave-to-class="opacity-0"
      >
        <div
          v-if="lightboxSrc"
          class="fixed inset-0 z-[100] flex items-center justify-center bg-black/80 backdrop-blur-sm cursor-zoom-out"
          @click="lightboxSrc = ''"
        >
          <img
            :src="lightboxSrc"
            :alt="lightboxAlt"
            class="max-w-[90vw] max-h-[90vh] object-contain rounded-xl shadow-2xl"
            @click.stop
          >
        </div>
      </Transition>
    </Teleport>
  </AppShell>
</template>

<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { RouterLink, RouterView, useRoute } from 'vue-router'
import {
  Menu,
  ChevronRight,
  X
} from 'lucide-vue-next'
import GithubIcon from '@/components/icons/GithubIcon.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import LanguageSwitcher from '@/components/common/LanguageSwitcher.vue'
import ThemeModeButton from '@/components/common/ThemeModeButton.vue'
import AppShell from '@/components/layout/AppShell.vue'
import { useSiteInfo } from '@/composables/useSiteInfo'
import { useI18n } from '@/i18n'
import { guideNavItems } from './guide-config'

const route = useRoute()
const { siteName, siteSubtitle } = useSiteInfo()
const { t } = useI18n()

const mobileMenuOpen = ref(false)
const baseUrl = ref(typeof window !== 'undefined' ? window.location.origin : 'https://your-aether.com')
const activeHash = ref('')
const lightboxSrc = ref('')
const lightboxAlt = ref('')
const resolvedGuideNavItems = computed(() => guideNavItems.map(item => ({
  ...item,
  name: t(item.nameKey),
  description: item.descriptionKey ? t(item.descriptionKey) : undefined,
  subItems: item.subItems?.map(subItem => ({
    ...subItem,
    name: t(subItem.nameKey),
  })),
})))

function onArticleClick(e: MouseEvent) {
  const target = e.target as HTMLElement
  if (target.tagName === 'IMG' && target.closest('.literary-content')) {
    const img = target as HTMLImageElement
    lightboxSrc.value = img.src
    lightboxAlt.value = img.alt || ''
  }
}

let observer: IntersectionObserver | null = null

function getScrollContainer(): Element | null {
  return document.querySelector('.app-shell__content')
}

function setupIntersectionObserver() {
  if (observer) {
    observer.disconnect()
  }

  const scrollRoot = getScrollContainer()

  observer = new IntersectionObserver(
    (entries) => {
      const visibleEntries = entries.filter((entry) => entry.isIntersecting)
      if (visibleEntries.length > 0) {
        const topEntry = visibleEntries.reduce((prev, current) => {
          return (current.boundingClientRect.top < prev.boundingClientRect.top) ? current : prev
        })
        activeHash.value = `#${topEntry.target.id}`
      }
    },
    {
      root: scrollRoot,
      rootMargin: '-80px 0px -70% 0px',
      threshold: 0
    }
  )

  const sections = document.querySelectorAll('article section[id]')
  sections.forEach((section) => observer?.observe(section))
}

function scrollToHash(hash: string) {
  activeHash.value = hash
  const el = document.querySelector(hash)
  const container = getScrollContainer()
  if (el && container) {
    const elTop = el.getBoundingClientRect().top
    const containerTop = container.getBoundingClientRect().top
    const offset = elTop - containerTop + container.scrollTop - 80
    container.scrollTo({ top: offset, behavior: 'smooth' })
  }
}

// 路由变化时管理状态和观察者
watch(
  () => route.path,
  () => {
    mobileMenuOpen.value = false
    activeHash.value = ''
    const container = getScrollContainer()
    if (container) {
      container.scrollTo({ top: 0 })
    }
    nextTick(() => {
      setupIntersectionObserver()
      const firstSection = document.querySelector('article section[id]')
      if (!activeHash.value && firstSection) {
        activeHash.value = `#${firstSection.id}`
      }
    })
  },
  { immediate: true }
)

onMounted(() => {
  nextTick(() => {
    setupIntersectionObserver()
  })
})

onUnmounted(() => {
  if (observer) {
    observer.disconnect()
  }
})

const currentNavItem = computed(() => {
  return resolvedGuideNavItems.value.find(item => item.path === route.path)
})

function isNavActive(href: string) {
  if (href === '/guide') {
    return route.path === '/guide'
  }
  return route.path === href || route.path.startsWith(`${href}/`)
}

// 移动端菜单用的导航数据
const navigation = computed(() => [
  {
    title: '',
    items: resolvedGuideNavItems.value.map(item => ({
      name: item.name,
      href: item.path,
      icon: item.icon
    }))
  }
])

// 样式类 - 与 MainLayout 保持一致
const sidebarClasses = computed(() => {
  return 'w-[260px] flex flex-col hidden lg:flex border-r border-[#3d3929]/5 dark:border-white/5 bg-[#faf9f5] dark:bg-[#1e1c19] h-screen sticky top-0'
})

const contentClasses = computed(() => {
  return 'flex-1 min-w-0 bg-[#faf9f5] dark:bg-[#191714] text-[#3d3929] dark:text-[#d4a27f]'
})

const mainClasses = computed(() => {
  return 'pt-24 lg:pt-8'
})
</script>

<style scoped>
.scrollbar-none::-webkit-scrollbar { display: none; }
.scrollbar-none { -ms-overflow-style: none; scrollbar-width: none; }

/* Literary Tech Typography Overrides for Guide Content */
:deep(.literary-content) h2 {
  @apply text-2xl mb-8 mt-12 flex items-center gap-3 transition-colors;
  font-family: var(--serif);
  font-weight: 500;
  letter-spacing: -0.015em;
  color: var(--color-text);
}

:deep(.literary-content) h3 {
  @apply text-xl mb-6 mt-10 transition-colors;
  font-family: var(--serif);
  font-weight: 500;
  letter-spacing: -0.01em;
  color: var(--color-text);
}

:deep(.literary-content) p:not([class*="text-sm"]):not([class*="text-xs"]) {
  font-family: var(--serif);
  font-weight: 400;
  @apply leading-relaxed text-[1.05rem] mb-4;
  color: var(--color-text);
  opacity: 0.9;
}

:deep(.literary-content) li:not([class*="text-sm"]):not([class*="text-xs"]) {
  font-family: var(--serif);
  font-weight: 400;
  @apply leading-relaxed text-[1.05rem] mb-2;
  color: var(--color-text);
  opacity: 0.9;
}

/* UI Elements inside content should remain sans-serif */
:deep(.literary-content) button,
:deep(.literary-content) input,
:deep(.literary-content) select,
:deep(.literary-content) label,
:deep(.literary-content) table,
:deep(.literary-content) .font-mono,
:deep(.literary-content) [class*="font-mono"] {
  font-family: var(--sans-serif);
}

:deep(.literary-content) pre,
:deep(.literary-content) code {
  font-family: var(--monospace) !important;
}

:deep(.literary-content) img {
  cursor: zoom-in;
}
</style>
