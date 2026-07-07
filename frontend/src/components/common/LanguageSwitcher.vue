<template>
  <DropdownMenu>
    <DropdownMenuTrigger as-child>
      <button
        class="flex h-9 w-9 items-center justify-center rounded-lg text-muted-foreground transition hover:bg-muted/50 hover:text-foreground"
        :aria-label="t('common.language')"
        :title="t('common.language')"
        type="button"
      >
        <Languages class="h-4 w-4" />
      </button>
    </DropdownMenuTrigger>
    <DropdownMenuContent
      align="end"
      class="min-w-36"
    >
      <DropdownMenuItem
        v-for="option in options"
        :key="option.value"
        class="justify-between gap-3"
        @select="setLocale(option.value)"
      >
        <span>{{ option.label }}</span>
        <Check
          v-if="locale === option.value"
          class="h-4 w-4 text-primary"
        />
      </DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Check, Languages } from 'lucide-vue-next'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { useI18n, useLocaleOptions, type Locale } from '@/i18n'

const { t } = useI18n()
const { locale, setLocale } = useLocaleOptions()

const options = computed<Array<{ value: Locale; label: string }>>(() => [
  { value: 'zh-CN', label: t('common.chinese') },
  { value: 'en-US', label: t('common.english') },
])
</script>
