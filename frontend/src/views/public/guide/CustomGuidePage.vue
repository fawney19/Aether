<template>
  <div class="min-h-screen bg-[#fafaf7] dark:bg-[#191714] text-[#191919] dark:text-white">
    <header class="sticky top-0 z-40 border-b border-[#cc785c]/10 dark:border-[rgba(227,224,211,0.12)] bg-[#fafaf7]/90 dark:bg-[#191714]/95 backdrop-blur-xl">
      <div class="h-14 sm:h-16 flex items-center justify-between px-4 sm:px-8">
        <RouterLink
          to="/"
          class="flex items-center gap-3 group"
        >
          <HeaderLogo
            size="h-8 w-8 sm:h-9 sm:w-9"
            class-name="text-[#191919] dark:text-white"
          />
          <div class="flex flex-col justify-center">
            <h1 class="text-base sm:text-lg font-bold leading-none">
              {{ siteName }}
            </h1>
            <span class="text-[10px] text-[#91918d] dark:text-muted-foreground leading-none mt-1.5 font-medium tracking-wide">
              {{ siteSubtitle }}
            </span>
          </div>
        </RouterLink>

        <div class="flex items-center gap-1">
          <a
            v-if="isUrlMode && guideUrl"
            :href="guideUrl"
            target="_blank"
            rel="noopener noreferrer"
            class="hidden sm:inline-flex h-9 items-center rounded-lg px-3 text-sm text-muted-foreground hover:text-foreground hover:bg-muted/50 transition"
          >
            新窗口打开
          </a>
          <ThemeModeButton />
          <LanguageSwitcher />
          <a
            v-if="showGithubLink"
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

    <main class="h-[calc(100vh-4rem)]">
      <iframe
        v-if="isUrlMode && guideUrl"
        :src="guideUrl"
        class="h-full w-full border-0 bg-white"
        sandbox="allow-scripts allow-same-origin allow-popups allow-popups-to-escape-sandbox allow-forms"
        referrerpolicy="no-referrer"
        title="Guide"
      />
      <iframe
        v-else-if="!isUrlMode && guideHtml.trim()"
        :srcdoc="guideHtml"
        class="h-full w-full border-0 bg-white"
        sandbox="allow-scripts allow-popups allow-forms"
        title="Guide"
      />
      <div
        v-else
        class="h-full flex items-center justify-center px-6 text-center text-sm text-muted-foreground"
      >
        自定义文档尚未配置完成。请到系统设置填写文档链接或 HTML 内容。
      </div>
    </main>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { RouterLink } from 'vue-router'
import HeaderLogo from '@/components/HeaderLogo.vue'
import LanguageSwitcher from '@/components/common/LanguageSwitcher.vue'
import ThemeModeButton from '@/components/common/ThemeModeButton.vue'
import GithubIcon from '@/components/icons/GithubIcon.vue'
import { useSiteInfo } from '@/composables/useSiteInfo'
import { useI18n } from '@/i18n'

const { t } = useI18n()
const {
  siteName,
  siteSubtitle,
  showGithubLink,
  guideCustomType,
  guideUrl,
  guideHtml,
} = useSiteInfo()

const isUrlMode = computed(() => guideCustomType.value === 'url')
</script>
