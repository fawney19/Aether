import type { App, InjectionKey, Ref } from 'vue'
import { computed, inject, readonly, ref } from 'vue'
import {
  defaultLocale,
  messages,
  supportedLocales,
  translateLegacyText,
  type Locale,
  type MessageKey,
} from './messages'
import { installLegacyDomTranslator } from './dom-translator'

type Params = Record<string, string | number>

interface I18nContext {
  locale: Ref<Locale>
  setLocale: (locale: Locale) => void
  t: (key: MessageKey, params?: Params) => string
  legacyT: (value: string) => string
}

const STORAGE_KEY = 'aether_locale'
const i18nKey: InjectionKey<I18nContext> = Symbol('aether-i18n')
const locale = ref<Locale>(readInitialLocale())

function isLocale(value: string | null | undefined): value is Locale {
  return !!value && supportedLocales.includes(value as Locale)
}

function readInitialLocale(): Locale {
  if (typeof window === 'undefined') return defaultLocale

  const stored = localStorage.getItem(STORAGE_KEY)
  if (isLocale(stored)) return stored

  const preferred = navigator.languages?.find(language => {
    const normalized = normalizeLocale(language)
    return isLocale(normalized)
  })
  const normalizedPreferred = normalizeLocale(preferred)
  return isLocale(normalizedPreferred) ? normalizedPreferred : defaultLocale
}

function normalizeLocale(value: string | undefined): string | undefined {
  if (!value) return undefined
  const lower = value.toLowerCase()
  if (lower.startsWith('zh')) return 'zh-CN'
  if (lower.startsWith('en')) return 'en-US'
  return value
}

function setLocale(nextLocale: Locale): void {
  locale.value = nextLocale
  if (typeof document !== 'undefined') {
    document.documentElement.lang = nextLocale
  }
  if (typeof window !== 'undefined') {
    localStorage.setItem(STORAGE_KEY, nextLocale)
  }
}

function formatMessage(template: string, params?: Params): string {
  if (!params) return template
  return template.replace(/\{(\w+)\}/g, (_, key: string) => String(params[key] ?? `{${key}}`))
}

function t(key: MessageKey, params?: Params): string {
  const bundle = messages[locale.value] ?? messages[defaultLocale]
  const template = bundle[key] ?? messages[defaultLocale][key] ?? key
  return formatMessage(template, params)
}

function legacyT(value: string): string {
  return translateLegacyText(value, locale.value)
}

const context: I18nContext = {
  locale,
  setLocale,
  t,
  legacyT,
}

export function createI18n() {
  return {
    install(app: App) {
      app.provide(i18nKey, context)
      app.config.globalProperties.$t = t
      app.config.globalProperties.$legacyT = legacyT
      setLocale(locale.value)
      installLegacyDomTranslator(locale)
    }
  }
}

export function useI18n() {
  return inject(i18nKey, context)
}

export function setI18nLocale(locale: Locale): void {
  setLocale(locale)
}

export function getI18nLocale(): Locale {
  return locale.value
}

export function useLocaleOptions() {
  const { locale: currentLocale, setLocale: applyLocale } = useI18n()
  const currentLocaleLabel = computed(() => {
    return currentLocale.value === 'zh-CN' ? t('common.chinese') : t('common.english')
  })

  return {
    locale: readonly(currentLocale),
    supportedLocales,
    currentLocaleLabel,
    setLocale: applyLocale,
  }
}

export type { Locale, MessageKey }
