<template>
  <div
    ref="scrollContainer"
    class="relative h-screen overflow-y-auto snap-y snap-mandatory scroll-smooth literary-grid literary-paper"
  >
    <!-- Fixed scroll indicator -->
    <nav class="scroll-indicator">
      <button
        v-for="(section, index) in sections"
        :key="index"
        class="scroll-indicator-btn group"
        @click="scrollToSection(index)"
      >
        <span class="scroll-indicator-label">{{ section.name }}</span>
        <div
          class="scroll-indicator-dot"
          :class="{ active: currentSection === index }"
        />
      </button>
    </nav>

    <!-- Header -->
    <header class="fixed top-0 left-0 right-0 z-50 border-b border-[#cc785c]/10 dark:border-[rgba(227,224,211,0.12)] bg-[#fafaf7]/90 dark:bg-[#191714]/95 backdrop-blur-xl transition-all">
      <div class="mx-auto max-w-7xl px-6 py-4">
        <div class="flex items-center justify-between">
          <!-- Logo & Brand -->
          <div
            class="flex items-center gap-3 group/logo cursor-pointer"
            @click="scrollToSection(0)"
          >
            <HeaderLogo
              size="h-9 w-9"
              class-name="text-[#191919] dark:text-white"
            />
            <div class="flex flex-col justify-center">
              <h1 class="text-lg font-bold text-[#191919] dark:text-white leading-none">
                Aether
              </h1>
              <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1.5 font-medium tracking-wide">API Gateway</span>
            </div>
          </div>

          <!-- Center Navigation -->
          <nav class="hidden md:flex items-center gap-2">
            <button
              v-for="(section, index) in sections"
              :key="index"
              class="group relative px-3 py-2 text-sm font-medium transition"
              :class="currentSection === index
                ? 'text-[#cc785c] dark:text-[#d4a27f]'
                : 'text-[#666663] dark:text-muted-foreground hover:text-[#191919] dark:hover:text-white'"
              @click="scrollToSection(index)"
            >
              {{ section.name }}
              <div
                class="absolute bottom-0 left-0 right-0 h-0.5 rounded-full transition-all duration-300"
                :class="currentSection === index ? 'bg-[#cc785c] dark:bg-[#d4a27f] scale-x-100' : 'bg-transparent scale-x-0'"
              />
            </button>
          </nav>

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
              <Sun
                v-else-if="themeMode === 'light'"
                class="h-4 w-4"
              />
              <Moon
                v-else
                class="h-4 w-4"
              />
            </button>

            <RouterLink
              v-if="authStore.isAuthenticated"
              :to="dashboardPath"
              class="rounded-xl bg-[#191919] dark:bg-[#cc785c] px-4 py-2 text-sm font-medium text-white shadow-sm transition hover:bg-[#262625] dark:hover:bg-[#b86d52]"
            >
              控制台
            </RouterLink>
            <button
              v-else
              class="rounded-xl bg-[#cc785c] px-4 py-2 text-sm font-medium text-white shadow-lg shadow-[#cc785c]/30 transition hover:bg-[#d4a27f]"
              @click="showLoginDialog = true"
            >
              登录
            </button>
          </div>
        </div>
      </div>
    </header>

    <!-- Main Content -->
    <main class="relative z-10">
      <!-- Fixed Logo Container -->
      <div class="mt-4 fixed inset-0 z-20 pointer-events-none flex items-center justify-center overflow-hidden">
        <div
          class="mt-16 transform-gpu logo-container"
          :class="[currentSection === SECTIONS.HOME ? 'home-section' : '', `logo-transition-${scrollDirection}`]"
          :style="fixedLogoStyle"
        >
          <Transition :name="logoTransitionName">
            <AetherLineByLineLogo
              v-if="currentSection === SECTIONS.HOME"
              ref="aetherLogoRef"
              key="aether-logo"
              :size="400"
              :line-delay="50"
              :stroke-duration="1200"
              :fill-duration="1500"
              :auto-start="false"
              :loop="true"
              :loop-pause="800"
              :stroke-width="3.5"
              :cycle-colors="true"
              :is-dark="isDark"
            />
            <div
              v-else
              :key="`ripple-wrapper-${currentLogoType}`"
              :class="{ 'heartbeat-wrapper': currentSection === SECTIONS.GEMINI && geminiFillComplete }"
            >
              <RippleLogo
                ref="rippleLogoRef"
                :type="currentLogoType"
                :size="320"
                :use-adaptive="false"
                :disable-ripple="currentSection === SECTIONS.GEMINI || currentSection === SECTIONS.FEATURES"
                :anim-delay="logoTransitionDelay"
                :static="currentSection === SECTIONS.FEATURES"
                class="logo-active"
                :class="[currentLogoClass]"
              />
            </div>
          </Transition>
        </div>
      </div>

      <!-- Section 0: Introduction -->
      <section
        ref="section0"
        class="min-h-screen snap-start flex items-center justify-center px-16 lg:px-20 py-20"
      >
        <div class="max-w-4xl mx-auto text-center">
          <div class="h-80 w-full mb-16 mt-8" />
          <h1
            class="mb-6 text-5xl md:text-7xl font-bold text-[#191919] dark:text-white leading-tight transition-all duration-700"
            :style="getTitleStyle(SECTIONS.HOME)"
          >
            欢迎使用 <span class="text-primary">Aether</span>
          </h1>
          <p
            class="mb-8 text-xl text-[#666663] dark:text-gray-300 max-w-2xl mx-auto transition-all duration-700"
            :style="getDescStyle(SECTIONS.HOME)"
          >
            AI 开发工具统一接入平台<br>
            整合 Claude Code、Codex CLI、Gemini CLI 等多个 AI 编程助手
          </p>
          <button
            class="mt-8 transition-all duration-700 cursor-pointer hover:scale-110"
            :style="getScrollIndicatorStyle(SECTIONS.HOME)"
            @click="scrollToSection(SECTIONS.CLAUDE)"
          >
            <ChevronDown class="h-8 w-8 mx-auto text-[#91918d] dark:text-muted-foreground/80 animate-bounce" />
          </button>
        </div>
      </section>

      <!-- Section 1: Claude Code -->
      <CliSection
        ref="section1"
        v-model:platform-value="claudePlatform"
        title="Claude Code"
        description="直接在您的终端中释放Claude的原始力量。瞬间搜索百万行代码库。将数小时的流程转化为单一命令。您的工具。您的流程。您的代码库,以思维速度进化。"
        :badge-icon="Code2"
        badge-text="IDE 集成"
        badge-class="bg-[#cc785c]/10 dark:bg-amber-900/30 border border-[#cc785c]/20 dark:border-amber-800 text-[#cc785c] dark:text-amber-400"
        :platform-options="platformPresets.claude.options"
        :install-command="claudeInstallCommand"
        :config-files="[{ path: '~/.claude/settings.json', content: claudeConfig, language: 'json' }]"
        :badge-style="getBadgeStyle(SECTIONS.CLAUDE)"
        :title-style="getTitleStyle(SECTIONS.CLAUDE)"
        :desc-style="getDescStyle(SECTIONS.CLAUDE)"
        :card-style-fn="(idx) => getCardStyle(SECTIONS.CLAUDE, idx)"
        content-position="right"
        @copy="copyToClipboard"
      />

      <!-- Section 2: Codex CLI -->
      <CliSection
        ref="section2"
        v-model:platform-value="codexPlatform"
        title="Codex CLI"
        description="Codex CLI 是一款可在本地终端运行的编程助手工具，它能够读取、修改并执行用户指定目录中的代码。"
        :badge-icon="Terminal"
        badge-text="命令行工具"
        badge-class="bg-[#cc785c]/10 dark:bg-emerald-900/30 border border-[#cc785c]/20 dark:border-emerald-800 text-[#cc785c] dark:text-emerald-400"
        :platform-options="platformPresets.codex.options"
        :install-command="codexInstallCommand"
        :config-files="[
          { path: '~/.codex/config.toml', content: codexConfig, language: 'toml' },
          { path: '~/.codex/auth.json', content: codexAuthConfig, language: 'json' }
        ]"
        :badge-style="getBadgeStyle(SECTIONS.CODEX)"
        :title-style="getTitleStyle(SECTIONS.CODEX)"
        :desc-style="getDescStyle(SECTIONS.CODEX)"
        :card-style-fn="(idx) => getCardStyle(SECTIONS.CODEX, idx)"
        content-position="left"
        @copy="copyToClipboard"
      />

      <!-- Section 3: Gemini CLI -->
      <CliSection
        ref="section3"
        v-model:platform-value="geminiPlatform"
        title="Gemini CLI"
        description="Gemini CLI 是一款开源人工智能代理，可将 Gemini 的强大功能直接带入你的终端。它提供了对 Gemini 的轻量级访问，为你提供了从提示符到我们模型的最直接路径。"
        :badge-icon="Sparkles"
        badge-text="多模态 AI"
        badge-class="bg-[#cc785c]/10 dark:bg-primary/20 border border-[#cc785c]/20 dark:border-primary/30 text-[#cc785c] dark:text-primary"
        :platform-options="platformPresets.gemini.options"
        :install-command="geminiInstallCommand"
        :config-files="[
          { path: '~/.gemini/.env', content: geminiEnvConfig, language: 'dotenv' },
          { path: '~/.gemini/settings.json', content: geminiSettingsConfig, language: 'json' }
        ]"
        :badge-style="getBadgeStyle(SECTIONS.GEMINI)"
        :title-style="getTitleStyle(SECTIONS.GEMINI)"
        :desc-style="getDescStyle(SECTIONS.GEMINI)"
        :card-style-fn="(idx) => getCardStyle(SECTIONS.GEMINI, idx)"
        content-position="right"
        @copy="copyToClipboard"
      >
        <template #logo>
          <GeminiStarCluster :is-visible="currentSection === SECTIONS.GEMINI && sectionVisibility[SECTIONS.GEMINI] > 0.05" />
        </template>
      </CliSection>

      <!-- Section 4: Features -->
      <section
        ref="section4"
        class="min-h-screen snap-start flex items-center justify-center px-16 lg:px-20 py-20 relative overflow-hidden"
      >
        <div class="max-w-4xl mx-auto text-center relative z-10">
          <div
            class="inline-flex items-center gap-2 rounded-full bg-[#cc785c]/10 dark:bg-purple-500/20 border border-[#cc785c]/20 dark:border-purple-500/40 px-4 py-2 text-sm font-medium text-[#cc785c] dark:text-purple-300 mb-6 backdrop-blur-sm transition-all duration-500"
            :style="getBadgeStyle(SECTIONS.FEATURES)"
          >
            <Sparkles class="h-4 w-4" />
            项目进度
          </div>

          <h2
            class="text-4xl md:text-5xl font-bold text-[#191919] dark:text-white mb-6 transition-all duration-700"
            :style="getTitleStyle(SECTIONS.FEATURES)"
          >
            功能开发进度
          </h2>

          <p
            class="text-lg text-[#666663] dark:text-gray-300 mb-12 max-w-2xl mx-auto transition-all duration-700"
            :style="getDescStyle(SECTIONS.FEATURES)"
          >
            核心 API 代理功能已完成，正在载入更多功能
          </p>

          <div class="grid md:grid-cols-3 gap-6">
            <div
              v-for="(feature, idx) in featureCards"
              :key="idx"
              class="bg-white/70 dark:bg-[#262624]/80 backdrop-blur-sm rounded-2xl p-6 border border-[#e5e4df] dark:border-[rgba(227,224,211,0.16)] hover:border-[#cc785c]/30 dark:hover:border-[#d4a27f]/40 transition-all duration-700"
              :style="getFeatureCardStyle(SECTIONS.FEATURES, idx)"
            >
              <div
                class="flex h-12 w-12 items-center justify-center rounded-xl mb-4 mx-auto"
                :class="feature.status === 'completed'
                  ? 'bg-emerald-500/10 dark:bg-emerald-500/15'
                  : 'bg-[#cc785c]/10 dark:bg-[#cc785c]/15'"
              >
                <component
                  :is="feature.icon"
                  class="h-6 w-6"
                  :class="feature.status === 'completed'
                    ? 'text-emerald-500 dark:text-emerald-400'
                    : 'text-[#cc785c] dark:text-[#d4a27f] animate-spin'"
                />
              </div>
              <h3 class="text-lg font-bold text-[#191919] dark:text-white mb-2">
                {{ feature.title }}
              </h3>
              <p class="text-sm text-[#666663] dark:text-[#c9c3b4]">
                {{ feature.desc }}
              </p>
              <div
                class="mt-3 inline-flex items-center gap-1.5 px-2.5 py-1 rounded-full text-xs font-medium"
                :class="feature.status === 'completed'
                  ? 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400'
                  : 'bg-amber-500/10 text-amber-600 dark:text-amber-400'"
              >
                {{ feature.status === 'completed' ? '已完成' : '进行中' }}
              </div>
            </div>
          </div>

          <div
            class="mt-12 transition-all duration-700"
            :style="getButtonsStyle(SECTIONS.FEATURES)"
          >
            <RouterLink
              v-if="authStore.isAuthenticated"
              :to="dashboardPath"
              class="inline-flex items-center gap-2 rounded-xl bg-primary hover:bg-primary/90 px-6 py-3 text-base font-semibold text-white shadow-lg shadow-primary/30 transition hover:shadow-primary/50 hover:scale-105"
            >
              <Rocket class="h-5 w-5" />
              立即开始使用
            </RouterLink>
            <button
              v-else
              class="inline-flex items-center gap-2 rounded-xl bg-primary hover:bg-primary/90 px-6 py-3 text-base font-semibold text-white shadow-lg shadow-primary/30 transition hover:shadow-primary/50 hover:scale-105"
              @click="showLoginDialog = true"
            >
              <Rocket class="h-5 w-5" />
              立即开始使用
            </button>
          </div>
        </div>
      </section>
    </main>

    <!-- Footer -->
    <footer class="relative z-10 border-t border-[#cc785c]/10 dark:border-[rgba(227,224,211,0.12)] bg-[#fafaf7]/90 dark:bg-[#191714]/95 backdrop-blur-md py-8">
      <div class="mx-auto max-w-7xl px-6">
        <div class="flex flex-col items-center justify-between gap-4 sm:flex-row">
          <p class="text-sm text-[#91918d] dark:text-muted-foreground">
            © 2025 Aether. 团队内部使用
          </p>
          <div class="flex items-center gap-6 text-sm text-[#91918d] dark:text-muted-foreground">
            <a
              href="#"
              class="transition hover:text-[#191919] dark:hover:text-white"
            >使用条款</a>
            <a
              href="#"
              class="transition hover:text-[#191919] dark:hover:text-white"
            >隐私政策</a>
            <a
              href="#"
              class="transition hover:text-[#191919] dark:hover:text-white"
            >技术支持</a>
          </div>
        </div>
      </div>
    </footer>

    <LoginDialog v-model="showLoginDialog" />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { RouterLink } from 'vue-router'
import {
  ChevronDown,
  Code2,
  Moon,
  Rocket,
  Sparkles,
  Sun,
  SunMoon,
  Terminal
} from 'lucide-vue-next'
import { useAuthStore } from '@/stores/auth'
import { useDarkMode } from '@/composables/useDarkMode'
import { useClipboard } from '@/composables/useClipboard'
import LoginDialog from '@/features/auth/components/LoginDialog.vue'
import RippleLogo from '@/components/RippleLogo.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import AetherLineByLineLogo from '@/components/AetherLineByLineLogo.vue'
import GeminiStarCluster from '@/components/GeminiStarCluster.vue'
import CliSection from './CliSection.vue'
import {
  SECTIONS,
  sections,
  featureCards,
  useCliConfigs,
  platformPresets,
  getInstallCommand,
  getLogoType,
  getLogoClass
} from './home-config'
import {
  useSectionAnimations,
  useLogoPosition,
  useLogoTransition
} from './useSectionAnimations'

const authStore = useAuthStore()
const { isDark, themeMode, toggleDarkMode } = useDarkMode()
const { copyToClipboard } = useClipboard()

const dashboardPath = computed(() =>
  authStore.user?.role === 'admin' ? '/admin/dashboard' : '/dashboard'
)
const baseUrl = computed(() => window.location.origin)

// Scroll state
const scrollContainer = ref<HTMLElement | null>(null)
const currentSection = ref(0)
const previousSection = ref(0)
const scrollDirection = ref<'up' | 'down'>('down')
const windowWidth = ref(typeof window !== 'undefined' ? window.innerWidth : 1024)
const sectionVisibility = ref<number[]>([0, 0, 0, 0, 0])
let lastScrollY = 0

// Section refs - section0 and section4 are direct HTML elements, section1-3 are CliSection components
const section0 = ref<HTMLElement | null>(null)
const section1 = ref<InstanceType<typeof CliSection> | null>(null)
const section2 = ref<InstanceType<typeof CliSection> | null>(null)
const section3 = ref<InstanceType<typeof CliSection> | null>(null)
const section4 = ref<HTMLElement | null>(null)

// Helper to get DOM element from ref (handles both direct elements and component instances)
const getSectionElement = (index: number): HTMLElement | null => {
  switch (index) {
    case 0: return section0.value
    case 1: return (section1.value?.sectionEl as HTMLElement | null | undefined) ?? null
    case 2: return (section2.value?.sectionEl as HTMLElement | null | undefined) ?? null
    case 3: return (section3.value?.sectionEl as HTMLElement | null | undefined) ?? null
    case 4: return section4.value
    default: return null
  }
}

// Logo refs
const aetherLogoRef = ref<InstanceType<typeof AetherLineByLineLogo> | null>(null)
const rippleLogoRef = ref<InstanceType<typeof RippleLogo> | null>(null)
const hasLogoAnimationStarted = ref(false)
const geminiFillComplete = ref(false)

// Animation composables
const {
  getBadgeStyle,
  getTitleStyle,
  getDescStyle,
  getButtonsStyle,
  getScrollIndicatorStyle,
  getCardStyle,
  getFeatureCardStyle
} = useSectionAnimations(sectionVisibility)

const { fixedLogoStyle } = useLogoPosition(currentSection, windowWidth)
const { logoTransitionName } = useLogoTransition(currentSection, previousSection)

// Logo computed
const currentLogoType = computed(() => getLogoType(currentSection.value))
const currentLogoClass = computed(() => getLogoClass(currentSection.value))
const logoTransitionDelay = computed(() => {
  if (currentSection.value === SECTIONS.FEATURES) return 0
  if (previousSection.value === SECTIONS.FEATURES) return 200
  return 500
})

// Platform states
const claudePlatform = ref(platformPresets.claude.defaultValue)
const codexPlatform = ref(platformPresets.codex.defaultValue)
const geminiPlatform = ref(platformPresets.gemini.defaultValue)

// Install commands
const claudeInstallCommand = computed(() => getInstallCommand('claude', claudePlatform.value))
const codexInstallCommand = computed(() => getInstallCommand('codex', codexPlatform.value))
const geminiInstallCommand = computed(() => getInstallCommand('gemini', geminiPlatform.value))

// CLI configs
const { claudeConfig, codexConfig, codexAuthConfig, geminiEnvConfig, geminiSettingsConfig } =
  useCliConfigs(baseUrl)

// Dialog state
const showLoginDialog = ref(false)

// Scroll handling
let scrollEndTimer: ReturnType<typeof setTimeout> | null = null

const calculateVisibility = (element: HTMLElement | null): number => {
  if (!element) return 0
  const rect = element.getBoundingClientRect()
  const containerHeight = window.innerHeight
  if (rect.bottom < 0 || rect.top > containerHeight) return 0
  const elementCenter = rect.top + rect.height / 2
  const viewportCenter = containerHeight / 2
  const distanceFromCenter = Math.abs(elementCenter - viewportCenter)
  const maxDistance = containerHeight / 2 + rect.height / 2
  return Math.max(0, 1 - distanceFromCenter / maxDistance)
}

const handleScroll = () => {
  if (!scrollContainer.value) return

  const containerHeight = window.innerHeight
  const newScrollY = scrollContainer.value.scrollTop

  // Track scroll direction
  scrollDirection.value = newScrollY > lastScrollY ? 'down' : 'up'
  lastScrollY = newScrollY

  // Update visibility
  for (let i = 0; i < 5; i++) {
    sectionVisibility.value[i] = calculateVisibility(getSectionElement(i))
  }

  // Update current section
  const scrollMiddle = newScrollY + containerHeight / 2
  for (let i = 4; i >= 0; i--) {
    const section = getSectionElement(i)
    if (section && section.offsetTop <= scrollMiddle) {
      if (currentSection.value !== i) {
        previousSection.value = currentSection.value
        currentSection.value = i
        hasLogoAnimationStarted.value = false
      }
      break
    }
  }

  // Detect snap complete
  if (scrollEndTimer) clearTimeout(scrollEndTimer)
  scrollEndTimer = setTimeout(() => {
    if (currentSection.value === SECTIONS.HOME && !hasLogoAnimationStarted.value) {
      hasLogoAnimationStarted.value = true
      setTimeout(() => aetherLogoRef.value?.startAnimation(), 100)
    }
  }, 150)
}

const scrollToSection = (index: number) => {
  const target = getSectionElement(index)
  if (target) target.scrollIntoView({ behavior: 'smooth' })
}

// Watch Gemini fill complete
watch(
  () => rippleLogoRef.value?.fillComplete,
  (val) => {
    if (currentSection.value === SECTIONS.GEMINI && val) geminiFillComplete.value = true
  }
)

watch(currentSection, (_, old) => {
  if (old === SECTIONS.GEMINI) geminiFillComplete.value = false
})

const handleResize = () => {
  windowWidth.value = window.innerWidth
}

onMounted(() => {
  scrollContainer.value?.addEventListener('scroll', handleScroll, { passive: true })
  window.addEventListener('resize', handleResize, { passive: true })
  handleScroll()

  // Initial animation
  setTimeout(() => {
    if (currentSection.value === SECTIONS.HOME && !hasLogoAnimationStarted.value) {
      hasLogoAnimationStarted.value = true
      setTimeout(() => aetherLogoRef.value?.startAnimation(), 100)
    }
  }, 300)
})

onUnmounted(() => {
  scrollContainer.value?.removeEventListener('scroll', handleScroll)
  window.removeEventListener('resize', handleResize)
  if (scrollEndTimer) clearTimeout(scrollEndTimer)
})
</script>

<style scoped>
/* Typography */
h1, h2, h3 {
  font-family: var(--serif);
  letter-spacing: -0.02em;
  font-weight: 500;
}

p {
  font-family: var(--serif);
  letter-spacing: 0.01em;
  line-height: 1.7;
}

button, nav, a, .inline-flex {
  font-family: var(--sans-serif);
}

/* Panel styles */
.command-panel-surface {
  border-color: var(--color-border);
  background: rgba(255, 255, 255, 0.5);
  backdrop-filter: blur(12px);
}

.dark .command-panel-surface {
  background: rgba(38, 38, 36, 0.3);
}

/* Performance */
h1, h2, p {
  will-change: transform, opacity;
}

/* Scroll indicator */
.scroll-indicator {
  position: fixed;
  right: 2rem;
  top: 50%;
  transform: translateY(-50%);
  z-index: 9999;
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
}

@media (max-width: 1023px) {
  .scroll-indicator {
    display: none;
  }
}

.scroll-indicator-btn {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  padding: 0.25rem;
}

.scroll-indicator-label {
  position: absolute;
  right: 1.5rem;
  font-size: 0.75rem;
  font-weight: 500;
  color: #666663;
  opacity: 0;
  transition: opacity 0.2s ease;
  white-space: nowrap;
  background: rgba(255, 255, 255, 0.9);
  backdrop-filter: blur(8px);
  padding: 0.25rem 0.5rem;
  border-radius: 0.25rem;
  pointer-events: none;
}

.dark .scroll-indicator-label {
  color: #a0a0a0;
  background: rgba(25, 23, 20, 0.9);
}

.scroll-indicator-btn:hover .scroll-indicator-label {
  opacity: 1;
}

.scroll-indicator-dot {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  border: 2px solid #d4d4d4;
  background: transparent;
  transition: all 0.3s ease;
}

.dark .scroll-indicator-dot {
  border-color: #4a4a4a;
}

.scroll-indicator-dot.active {
  background: #cc785c;
  border-color: #cc785c;
  transform: scale(1.3);
}

/* Logo transitions */
.logo-scale-enter-active {
  transition: opacity 0.5s ease-out, transform 0.5s cubic-bezier(0.34, 1.56, 0.64, 1);
}

.logo-scale-leave-active {
  transition: opacity 0.3s ease-in, transform 0.3s ease-in;
}

.logo-scale-enter-from {
  opacity: 0;
  transform: scale(0.6) rotate(-8deg);
}

.logo-scale-leave-to {
  opacity: 0;
  transform: scale(1.2) rotate(8deg);
}

.logo-slide-left-enter-active,
.logo-slide-right-enter-active {
  transition: opacity 0.4s ease-out, transform 0.5s cubic-bezier(0.25, 0.46, 0.45, 0.94);
}

.logo-slide-left-leave-active,
.logo-slide-right-leave-active {
  transition: opacity 0.25s ease-in, transform 0.3s ease-in;
}

.logo-slide-left-enter-from {
  opacity: 0;
  transform: translateX(60px) scale(0.9);
}

.logo-slide-left-leave-to {
  opacity: 0;
  transform: translateX(-60px) scale(0.9);
}

.logo-slide-right-enter-from {
  opacity: 0;
  transform: translateX(-60px) scale(0.9);
}

.logo-slide-right-leave-to {
  opacity: 0;
  transform: translateX(60px) scale(0.9);
}

/* Logo container */
.logo-container {
  width: 320px;
  height: 320px;
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
}

.logo-container.home-section {
  width: 400px;
  height: 400px;
}

.logo-container > * {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
}

@media (max-width: 768px) {
  .logo-container {
    width: 240px;
    height: 240px;
  }
  .logo-container.home-section {
    width: 280px;
    height: 280px;
  }
}

/* Heartbeat animation */
.heartbeat-wrapper {
  animation: heartbeat 1.5s ease-in-out infinite;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 100%;
  height: 100%;
}

@keyframes heartbeat {
  0%, 70%, 100% { transform: scale(1); }
  14% { transform: scale(1.06); }
  28% { transform: scale(1); }
  42% { transform: scale(1.1); }
}
</style>
