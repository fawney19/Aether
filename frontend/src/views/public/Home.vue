<template>
  <div
    ref="scrollContainer"
    class="relative h-screen overflow-y-auto overflow-x-hidden snap-y snap-mandatory scroll-smooth literary-grid literary-paper"
  >
    <!-- Fixed scroll indicator -->
    <nav class="scroll-indicator">
      <button
        v-for="(section, index) in resolvedSections"
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
    <header class="sticky top-0 z-50 border-b border-[#cc785c]/10 dark:border-[rgba(227,224,211,0.12)] bg-[#fafaf7]/90 dark:bg-[#191714]/95 backdrop-blur-xl transition-all">
      <!-- Mobile layout (< md): Logo left, buttons right -->
      <div class="h-14 sm:h-16 flex md:hidden items-center justify-between px-3 sm:px-4">
        <!-- Logo & Brand -->
        <div
          class="flex items-center gap-2 sm:gap-3 group/logo cursor-pointer shrink-0"
          @click="scrollToSection(0)"
        >
          <HeaderLogo
            size="h-7 w-7 sm:h-9 sm:w-9"
            class-name="text-[#191919] dark:text-white"
          />
          <div class="flex flex-col justify-center">
            <h1 class="text-base sm:text-lg font-bold text-[#191919] dark:text-white leading-none">
              {{ siteName }}
            </h1>
            <span class="text-[9px] sm:text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1 sm:mt-1.5 font-medium tracking-wide">{{ siteSubtitle }}</span>
          </div>
        </div>

        <!-- Right: Login + Icons -->
        <div class="flex items-center gap-2">
          <RouterLink
            v-if="authStore.isAuthenticated"
            :to="dashboardPath"
            class="min-w-[60px] text-center rounded-lg bg-[#191919] dark:bg-[#cc785c] px-3 py-1.5 text-xs font-medium text-white shadow-sm transition hover:bg-[#262625] dark:hover:bg-[#b86d52] whitespace-nowrap"
          >
            {{ t('site.home.enterDashboard') }}
          </RouterLink>
          <button
            v-else
            class="min-w-[60px] text-center rounded-lg bg-[#cc785c] px-3 py-1.5 text-xs font-medium text-white shadow-lg shadow-[#cc785c]/30 transition hover:bg-[#d4a27f] whitespace-nowrap"
            @click="showLoginDialog = true"
          >
            {{ t('site.home.login') }}
          </button>
          <ThemeModeButton size="sm" />
          <LanguageSwitcher />
          <a
            href="https://github.com/fawney19/Aether"
            target="_blank"
            rel="noopener noreferrer"
            class="flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
            :title="t('common.githubRepository')"
          >
            <GithubIcon class="h-3.5 w-3.5" />
          </a>
        </div>
      </div>

      <!-- Desktop layout (>= md): Centered nav with balanced spacing -->
      <div class="h-16 hidden md:flex items-center justify-between px-8">
        <!-- Left spacer for balance (matches right icons width) -->
        <div class="w-[76px] shrink-0" />

        <!-- Center: Logo + Nav + Login Button -->
        <div class="flex items-center">
          <!-- Logo & Brand -->
          <div
            class="flex items-center gap-3 group/logo cursor-pointer shrink-0"
            @click="scrollToSection(0)"
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
          </div>

          <!-- Navigation -->
          <nav class="flex items-center gap-2 mx-8 lg:mx-16">
            <button
              v-for="(section, index) in resolvedSections.slice(0, -1)"
              :key="index"
              class="group relative px-3 py-2 text-sm font-medium transition whitespace-nowrap"
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
            <RouterLink
              to="/guide"
              class="group relative px-3 py-2 text-sm font-medium transition whitespace-nowrap text-[#666663] dark:text-muted-foreground hover:text-[#191919] dark:hover:text-white"
            >
              {{ t('site.home.docLink') }}
            </RouterLink>
            <button
              class="group relative px-3 py-2 text-sm font-medium transition whitespace-nowrap"
              :class="currentSection === SECTIONS.FEATURES
                ? 'text-[#cc785c] dark:text-[#d4a27f]'
                : 'text-[#666663] dark:text-muted-foreground hover:text-[#191919] dark:hover:text-white'"
              @click="scrollToSection(SECTIONS.FEATURES)"
            >
              {{ resolvedSections[SECTIONS.FEATURES].name }}
              <div
                class="absolute bottom-0 left-0 right-0 h-0.5 rounded-full transition-all duration-300"
                :class="currentSection === SECTIONS.FEATURES ? 'bg-[#cc785c] dark:bg-[#d4a27f] scale-x-100' : 'bg-transparent scale-x-0'"
              />
            </button>
          </nav>

          <!-- Login/Dashboard Button -->
          <RouterLink
            v-if="authStore.isAuthenticated"
            :to="dashboardPath"
            class="min-w-[72px] text-center rounded-xl bg-[#191919] dark:bg-[#cc785c] px-4 py-2 text-sm font-medium text-white shadow-sm transition hover:bg-[#262625] dark:hover:bg-[#b86d52] whitespace-nowrap"
          >
            {{ t('site.home.enterDashboard') }}
          </RouterLink>
          <button
            v-else
            class="min-w-[72px] text-center rounded-xl bg-[#cc785c] px-4 py-2 text-sm font-medium text-white shadow-lg shadow-[#cc785c]/30 transition hover:bg-[#d4a27f] whitespace-nowrap"
            @click="showLoginDialog = true"
          >
            {{ t('site.home.login') }}
          </button>
        </div>

        <!-- Right: Theme Toggle + GitHub Icons -->
        <div class="flex items-center gap-1 shrink-0">
          <ThemeModeButton />
          <LanguageSwitcher />
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
      </div>
    </header>

    <!-- Main Content -->
    <main class="relative z-10">
      <!-- Fixed Logo Container -->
      <div class="fixed top-0 left-0 right-0 bottom-0 z-20 pointer-events-none flex items-center justify-center overflow-hidden">
        <!-- Gemini Star Cluster - positioned behind logo -->
        <Transition name="fade">
          <GeminiStarCluster
            v-if="currentSection === SECTIONS.GEMINI"
            :is-visible="sectionVisibility[SECTIONS.GEMINI] > 0.05"
            class="absolute gemini-stars"
            :class="windowWidth < 768 ? 'scale-75 opacity-60' : ''"
            :style="fixedLogoStyle"
          />
        </Transition>

        <div
          class="transform-gpu logo-container"
          :class="[currentSection === SECTIONS.HOME ? 'home-section' : '', `logo-transition-${scrollDirection}`]"
          :style="fixedLogoStyle"
        >
          <Transition :name="logoTransitionName">
            <AetherLineByLineLogo
              v-if="currentSection === SECTIONS.HOME"
              ref="aetherLogoRef"
              key="aether-logo"
              :size="homeLogoSize"
              :line-delay="50"
              :stroke-duration="1200"
              :fill-duration="1500"
              :auto-start="false"
              :loop="true"
              :loop-pause="800"
              :stroke-width="windowWidth < 768 ? 2.5 : 3.5"
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
                :size="windowWidth < 768 ? 200 : 320"
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
        class="min-h-screen snap-start flex items-center justify-center px-4 sm:px-8 md:px-16 lg:px-20 py-20"
      >
        <div class="max-w-4xl mx-auto text-center">
          <div class="h-64 sm:h-80 md:h-[26rem] w-full mb-12 sm:mb-8 md:mb-10 mt-8 sm:mt-12" />
          <h1
            class="mb-6 text-3xl sm:text-5xl md:text-7xl font-bold text-[#191919] dark:text-white leading-tight transition-all duration-700"
            :style="getTitleStyle(SECTIONS.HOME)"
          >
            {{ t('site.home.hero.titlePrefix') }} <span class="text-primary typewriter">{{ aetherText }}<span
              class="cursor"
              :class="{ 'cursor-hidden': !showCursor }"
            >_</span></span>
          </h1>
          <p
            class="mb-8 text-base sm:text-lg md:text-xl text-[#666663] dark:text-[#c9c3b4] max-w-2xl mx-auto transition-all duration-700"
            :style="getDescStyle(SECTIONS.HOME)"
          >
            {{ t('site.home.hero.subtitle') }}<br>
            {{ t('site.home.hero.subtitleLine2') }}
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
        :title="t('site.home.cli.claudeTitle')"
        :description="t('site.home.cli.claudeDescription')"
        :badge-icon="Code2"
        :badge-text="t('site.home.cli.ideIntegration')"
        badge-class="bg-[#cc785c]/10 dark:bg-[#cc785c]/20 border border-[#cc785c]/20 dark:border-[#d4a27f]/30 text-[#cc785c] dark:text-[#d4a27f]"
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
        :title="t('site.home.cli.codexTitle')"
        :description="t('site.home.cli.codexDescription')"
        :badge-icon="Terminal"
        :badge-text="t('site.home.cli.commandLine')"
        badge-class="bg-[#cc785c]/10 dark:bg-[#cc785c]/20 border border-[#cc785c]/20 dark:border-[#d4a27f]/30 text-[#cc785c] dark:text-[#d4a27f]"
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
        :title="t('site.home.cli.geminiTitle')"
        :description="t('site.home.cli.geminiDescription')"
        :badge-icon="Sparkles"
        :badge-text="t('site.home.cli.multimodalAi')"
        badge-class="bg-[#cc785c]/10 dark:bg-[#cc785c]/20 border border-[#cc785c]/20 dark:border-[#d4a27f]/30 text-[#cc785c] dark:text-[#d4a27f]"
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
      />

      <!-- Section 4: Features -->
      <section
        ref="section4"
        class="min-h-screen snap-start flex items-center justify-center px-4 sm:px-8 md:px-16 lg:px-20 py-12 md:py-20 relative overflow-hidden"
      >
        <div class="max-w-4xl mx-auto text-center relative z-10">
          <div
            class="inline-flex items-center gap-1.5 md:gap-2 rounded-full bg-[#cc785c]/10 dark:bg-[#cc785c]/20 border border-[#cc785c]/20 dark:border-[#d4a27f]/30 px-3 md:px-4 py-1.5 md:py-2 text-xs md:text-sm font-medium text-[#cc785c] dark:text-[#d4a27f] mb-4 md:mb-6 backdrop-blur-sm transition-all duration-500"
            :style="getBadgeStyle(SECTIONS.FEATURES)"
          >
            <Sparkles class="h-3.5 w-3.5 md:h-4 md:w-4" />
            {{ t('site.home.projectProgress') }}
          </div>

          <h2
            class="text-2xl md:text-5xl font-bold text-[#191919] dark:text-white mb-3 md:mb-6 transition-all duration-700"
            :style="getTitleStyle(SECTIONS.FEATURES)"
          >
            {{ t('site.home.featureProgress') }}
          </h2>

          <p
            class="text-base md:text-lg text-[#666663] dark:text-[#c9c3b4] mb-6 md:mb-12 max-w-2xl mx-auto transition-all duration-700"
            :style="getDescStyle(SECTIONS.FEATURES)"
          >
            {{ t('site.home.featureProgressDesc') }}
          </p>

          <div class="grid md:grid-cols-3 gap-3 md:gap-6">
            <div
              v-for="(feature, idx) in resolvedFeatureCards"
              :key="idx"
              class="group bg-white/90 dark:bg-[#262624]/80 backdrop-blur-sm rounded-xl md:rounded-2xl p-4 md:p-6 border transition-all duration-700"
              :class="feature.status === 'completed'
                ? 'border-[#cc785c]/20 dark:border-[#d4a27f]/20'
                : 'border-[#e5e4df] dark:border-[rgba(227,224,211,0.16)] border-dashed'"
              :style="getFeatureCardStyle(SECTIONS.FEATURES, idx)"
            >
              <div
                class="flex h-10 w-10 md:h-12 md:w-12 items-center justify-center rounded-lg md:rounded-xl mb-2 md:mb-4 mx-auto bg-[#cc785c]/8 dark:bg-[#cc785c]/12"
              >
                <component
                  :is="feature.icon"
                  class="h-5 w-5 md:h-6 md:w-6 text-[#cc785c] dark:text-[#d4a27f]"
                  :class="{ 'opacity-50': feature.status !== 'completed' }"
                />
              </div>
              <h3
                class="text-base md:text-lg font-bold mb-1 md:mb-2"
                :class="feature.status === 'completed'
                  ? 'text-[#191919] dark:text-white'
                  : 'text-[#666663] dark:text-[#a0a0a0]'"
              >
                {{ feature.title }}
              </h3>
              <p class="text-xs md:text-sm text-[#666663] dark:text-[#c9c3b4]">
                {{ feature.desc }}
              </p>
              <div
                class="mt-2 md:mt-3 inline-flex items-center gap-1.5 px-2 md:px-2.5 py-0.5 md:py-1 rounded-full text-xs font-medium border"
                :class="feature.status === 'completed'
                  ? 'bg-[#cc785c]/5 text-[#cc785c] dark:text-[#d4a27f] border-[#cc785c]/20 dark:border-[#d4a27f]/20'
                  : 'bg-transparent text-[#91918d] dark:text-[#808080] border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)]'"
              >
                {{ feature.status === 'completed' ? t('site.home.status.completed') : t('site.home.status.inProgress') }}
              </div>
            </div>
          </div>

          <div
            class="mt-6 md:mt-12 transition-all duration-700 flex items-center justify-center gap-4 relative z-30"
            :style="getButtonsStyle(SECTIONS.FEATURES)"
          >
            <RouterLink
              v-if="authStore.isAuthenticated"
              :to="dashboardPath"
              class="inline-flex items-center justify-center gap-2 rounded-xl bg-transparent border-2 border-[#cc785c] px-6 py-3 text-base font-semibold text-[#cc785c] dark:text-[#d4a27f] dark:border-[#d4a27f] transition hover:bg-[#cc785c]/10 dark:hover:bg-[#d4a27f]/10 hover:scale-105 w-[160px]"
            >
              <Rocket class="h-5 w-5" />
              {{ t('site.home.enterDashboard') }}
            </RouterLink>
            <button
              v-else
              class="inline-flex items-center justify-center gap-2 rounded-xl bg-transparent border-2 border-[#cc785c] px-6 py-3 text-base font-semibold text-[#cc785c] dark:text-[#d4a27f] dark:border-[#d4a27f] transition hover:bg-[#cc785c]/10 dark:hover:bg-[#d4a27f]/10 hover:scale-105 w-[160px]"
              @click="showLoginDialog = true"
            >
              <Rocket class="h-5 w-5" />
              {{ t('site.home.startNow') }}
            </button>
          </div>
        </div>
      </section>
    </main>

    <LoginDialog v-model="showLoginDialog" />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { RouterLink } from 'vue-router'
import {
  ChevronDown,
  Code2,
  Rocket,
  Sparkles,
  Terminal
} from 'lucide-vue-next'
import GithubIcon from '@/components/icons/GithubIcon.vue'
import { useAuthStore } from '@/stores/auth'
import { useDarkMode } from '@/composables/useDarkMode'
import { useClipboard } from '@/composables/useClipboard'
import { useSiteInfo } from '@/composables/useSiteInfo'
import LanguageSwitcher from '@/components/common/LanguageSwitcher.vue'
import ThemeModeButton from '@/components/common/ThemeModeButton.vue'
import LoginDialog from '@/features/auth/components/LoginDialog.vue'
import RippleLogo from '@/components/RippleLogo.vue'
import HeaderLogo from '@/components/HeaderLogo.vue'
import AetherLineByLineLogo from '@/components/AetherLineByLineLogo.vue'
import GeminiStarCluster from '@/components/GeminiStarCluster.vue'
import CliSection from './CliSection.vue'
import { platformPresets, getInstallCommand } from '@/config/platform-presets'
import {
  SECTIONS,
  sections,
  featureCards,
  useCliConfigs,
  getLogoType,
  getLogoClass
} from './home-config'
import {
  useSectionAnimations,
  useLogoPosition,
  useLogoTransition
} from './useSectionAnimations'
import { useI18n } from '@/i18n'

const authStore = useAuthStore()
const { isDark } = useDarkMode()
const { copyToClipboard } = useClipboard()
const { siteName, siteSubtitle } = useSiteInfo()
const { t } = useI18n()

const dashboardPath = computed(() =>
  authStore.canAccessAdmin ? '/admin/dashboard' : '/dashboard'
)
const baseUrl = computed(() => window.location.origin)

const resolvedSections = computed(() => sections.map(section => ({
  ...section,
  name: t(section.nameKey)
})))

const resolvedFeatureCards = computed(() => featureCards.map(card => ({
  ...card,
  title: t(card.titleKey),
  desc: t(card.descKey),
})))

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

// Responsive logo size - matches .logo-container.home-section CSS
const homeLogoSize = computed(() => windowWidth.value < 768 ? 280 : 400)
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

// Typewriter effect for site name
const aetherText = ref('')
const showCursor = ref(true)
const typewriterFullText = computed(() => siteName.value)
let typewriterTimer: ReturnType<typeof setTimeout> | null = null
const hasTypewriterStarted = ref(false)

const startTypewriter = () => {
  if (hasTypewriterStarted.value) return
  hasTypewriterStarted.value = true
  aetherText.value = ''
  showCursor.value = true

  const typeSpeed = 200
  const deleteSpeed = 120
  const pauseAfterType = 3500
  const pauseAfterDelete = 1000

  const typeLoop = () => {
    let index = 0
    const fullText = typewriterFullText.value

    // Type phase
    const typeNextChar = () => {
      if (index < fullText.length) {
        aetherText.value = fullText.slice(0, index + 1)
        index++
        typewriterTimer = setTimeout(typeNextChar, typeSpeed)
      } else {
        // Pause then start deleting
        typewriterTimer = setTimeout(deleteChars, pauseAfterType)
      }
    }

    // Delete phase
    const deleteChars = () => {
      if (aetherText.value.length > 0) {
        aetherText.value = aetherText.value.slice(0, -1)
        typewriterTimer = setTimeout(deleteChars, deleteSpeed)
      } else {
        // Pause then restart typing
        typewriterTimer = setTimeout(typeLoop, pauseAfterDelete)
      }
    }

    typeNextChar()
  }
  
  // Start typing after a short delay
  typewriterTimer = setTimeout(typeLoop, 400)
}

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
      startTypewriter()
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
      startTypewriter()
    }
  }, 300)
})

onUnmounted(() => {
  scrollContainer.value?.removeEventListener('scroll', handleScroll)
  window.removeEventListener('resize', handleResize)
  if (scrollEndTimer) clearTimeout(scrollEndTimer)
  if (typewriterTimer) clearTimeout(typewriterTimer)
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

/* Gemini star cluster positioning */
.gemini-stars {
  z-index: -1;
}

/* Fade transition */
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.6s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}

/* Typewriter cursor */
.typewriter {
  display: inline;
}

.typewriter .cursor {
  font-weight: 400;
  opacity: 1;
  animation: cursor-blink 1s ease-in-out infinite;
  margin-left: 1px;
}

.typewriter .cursor.cursor-hidden {
  opacity: 0;
  animation: none;
}

@keyframes cursor-blink {
  0%, 45% { opacity: 1; }
  50%, 100% { opacity: 0; }
}
</style>
