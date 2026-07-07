<template>
  <button
    type="button"
    :class="buttonClass"
    :title="title"
    :aria-label="title"
    @click="toggleDarkMode"
  >
    <SunMoon
      v-if="themeMode === 'system'"
      :class="iconClass"
    />
    <SunMedium
      v-else-if="themeMode === 'light'"
      :class="iconClass"
    />
    <Moon
      v-else
      :class="iconClass"
    />
  </button>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Moon, SunMedium, SunMoon } from 'lucide-vue-next'
import { useDarkMode } from '@/composables/useDarkMode'
import { useI18n } from '@/i18n'

const props = withDefaults(defineProps<{
  size?: 'sm' | 'md'
  class?: string
}>(), {
  size: 'md',
  class: '',
})

const { themeMode, toggleDarkMode } = useDarkMode()
const { t } = useI18n()

const title = computed(() => {
  if (themeMode.value === 'system') return t('theme.system')
  if (themeMode.value === 'dark') return t('theme.dark')
  return t('theme.light')
})

const buttonClass = computed(() => [
  'flex items-center justify-center rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted/50 transition',
  props.size === 'sm' ? 'h-8 w-8' : 'h-9 w-9',
  props.class,
].filter(Boolean).join(' '))

const iconClass = computed(() => props.size === 'sm' ? 'h-3.5 w-3.5' : 'h-4 w-4')
</script>
