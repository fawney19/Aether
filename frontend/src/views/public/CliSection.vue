<template>
  <section
    ref="sectionRef"
    class="min-h-screen snap-start flex items-center justify-center px-4 sm:px-8 md:px-16 lg:px-20 py-20"
  >
    <div class="max-w-7xl mx-auto grid md:grid-cols-2 gap-12 items-center w-full">
      <!-- Content column -->
      <div :class="contentOrder">
        <!-- Badge -->
        <div
          class="inline-flex items-center gap-2 rounded-full px-3 py-1 text-xs font-medium mb-4 transition-all duration-500"
          :class="badgeClass"
          :style="badgeStyle"
        >
          <component
            :is="badgeIcon"
            class="h-3 w-3"
          />
          {{ badgeText }}
        </div>

        <!-- Title -->
        <h2
          class="text-4xl md:text-5xl font-bold text-[#191919] dark:text-white mb-6 transition-all duration-700"
          :style="titleStyle"
        >
          {{ title }}
        </h2>

        <!-- Description -->
        <p
          class="text-lg text-[#666663] dark:text-[#c9c3b4] mb-4 transition-all duration-700"
          :style="descStyle"
        >
          {{ description }}
        </p>

        <!-- Install command -->
        <div
          class="mb-4 transition-all duration-700 relative z-10"
          :style="cardStyleFn(0)"
        >
          <div
            class="flex flex-wrap items-center gap-3 px-4 py-3"
            :class="[panelClasses.commandPanel]"
          >
            <PlatformSelect
              :model-value="platformValue"
              :options="platformOptions"
              class="shrink-0"
              @update:model-value="$emit('update:platformValue', $event)"
            />
            <div class="flex-1 min-w-[180px]">
              <CodeHighlight
                :code="installCommand"
                language="bash"
                dense
              />
            </div>
            <button
              :class="panelClasses.iconButtonSmall"
              :title="t('site.home.copyConfig')"
              @click="$emit('copy', installCommand)"
            >
              <Copy class="h-3.5 w-3.5" />
            </button>
          </div>
        </div>

        <!-- Config files -->
        <div
          v-for="(config, idx) in configFiles"
          :key="config.path"
          class="transition-all duration-700"
          :class="idx < configFiles.length - 1 ? 'mb-3' : ''"
          :style="cardStyleFn(idx + 1)"
        >
          <div
            class="overflow-hidden"
            :class="[panelClasses.configPanel]"
          >
            <div :class="panelClasses.panelHeader">
              <div class="flex items-center justify-between">
                <span class="text-xs font-medium text-[#666663] dark:text-[#c9c3b4]">
                  {{ config.path }}
                </span>
                <button
                  :class="panelClasses.iconButtonSmall"
                  :title="t('site.home.copyConfig')"
                  @click="$emit('copy', config.content)"
                >
                  <Copy class="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
            <div :class="panelClasses.codeBody">
              <div class="config-code-wrapper">
                <CodeHighlight
                  :code="config.content"
                  :language="config.language"
                />
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- Logo placeholder column - hidden on mobile since logo is fixed positioned -->
      <div
        :class="logoOrder"
        class="hidden md:flex items-center justify-center h-full min-h-[300px] relative"
      >
        <slot name="logo" />
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { ref, computed, type CSSProperties, type Component } from 'vue'
import { Copy } from 'lucide-vue-next'
import PlatformSelect from '@/components/PlatformSelect.vue'
import CodeHighlight from '@/components/CodeHighlight.vue'
import { panelClasses } from './home-config'
import type { PlatformOption } from '@/config/platform-presets'
import { useI18n } from '@/i18n'

const props = withDefaults(defineProps<Props>(), {
  contentPosition: 'left'
})
defineEmits<{
  copy: [text: string]
  'update:platformValue': [value: string]
}>()
const { t } = useI18n()
// Expose section element for parent scroll tracking
const sectionRef = ref<HTMLElement | null>(null)
defineExpose({ sectionEl: sectionRef })

interface ConfigFile {
  path: string
  content: string
  language: string
}

interface Props {
  title: string
  description: string
  badgeIcon: Component
  badgeText: string
  badgeClass: string
  platformValue: string
  platformOptions: PlatformOption[]
  installCommand: string
  configFiles: ConfigFile[]
  // Style props
  badgeStyle: CSSProperties
  titleStyle: CSSProperties
  descStyle: CSSProperties
  cardStyleFn: (cardIndex: number) => CSSProperties
  // Layout: 'left' means content on left, 'right' means content on right
  contentPosition?: 'left' | 'right'
}

const contentOrder = computed(() =>
  props.contentPosition === 'right' ? 'md:order-2' : ''
)

const logoOrder = computed(() =>
  props.contentPosition === 'right' ? 'md:order-1' : ''
)
</script>

<style scoped>
.config-code-wrapper :deep(.code-highlight pre) {
  border: none;
  border-radius: 0;
  margin: 0;
  background-color: transparent !important;
  padding: 1rem 1.2rem !important;
}

/* Header separator line */
.panel-header {
  border-bottom: 1px solid var(--color-border);
}
</style>
