import { readonly, ref, watch } from 'vue'
import apiClient from '@/api/client'

export type GuideMode = 'builtin' | 'hidden' | 'custom'
export type GuideCustomType = 'url' | 'html'

export interface SiteInfo {
  site_name: string
  site_subtitle: string
  show_github_link: boolean
  guide_mode: GuideMode
  guide_custom_type: GuideCustomType
  guide_url: string
  guide_html: string
}

const DEFAULT_SITE_INFO: SiteInfo = {
  site_name: 'Aether',
  site_subtitle: 'AI Gateway',
  show_github_link: true,
  guide_mode: 'builtin',
  guide_custom_type: 'url',
  guide_url: '',
  guide_html: '',
}

const siteName = ref('')
const siteSubtitle = ref('')
const showGithubLink = ref(false)
const guideMode = ref<GuideMode>('hidden')
const guideCustomType = ref<GuideCustomType>('url')
const guideUrl = ref('')
const guideHtml = ref('')
const loaded = ref(false)
let fetchPromise: Promise<void> | null = null

function normalizeGuideMode(value: unknown): GuideMode {
  return value === 'hidden' || value === 'custom' || value === 'builtin' ? value : 'builtin'
}

function normalizeGuideCustomType(value: unknown): GuideCustomType {
  return value === 'html' ? 'html' : 'url'
}

function isPublicHttpUrl(value: string): boolean {
  try {
    const parsed = new URL(value.trim())
    return parsed.protocol === 'http:' || parsed.protocol === 'https:'
  } catch {
    return false
  }
}

function normalizeSiteInfo(data: Partial<SiteInfo> | null | undefined): SiteInfo {
  const mode = normalizeGuideMode(data?.guide_mode)
  const customType = normalizeGuideCustomType(data?.guide_custom_type)
  const url = typeof data?.guide_url === 'string' && isPublicHttpUrl(data.guide_url) ? data.guide_url.trim() : ''
  return {
    site_name: data?.site_name?.trim() || DEFAULT_SITE_INFO.site_name,
    site_subtitle: data?.site_subtitle?.trim() || DEFAULT_SITE_INFO.site_subtitle,
    show_github_link: data?.show_github_link !== false,
    guide_mode: mode,
    guide_custom_type: customType,
    guide_url: mode === 'custom' && customType === 'url' ? url : '',
    guide_html: mode === 'custom' && customType === 'html' && typeof data?.guide_html === 'string'
      ? data.guide_html
      : '',
  }
}

function applySiteInfo(data: Partial<SiteInfo> | null | undefined): void {
  const normalized = normalizeSiteInfo(data)
  siteName.value = normalized.site_name
  siteSubtitle.value = normalized.site_subtitle
  showGithubLink.value = normalized.show_github_link
  guideMode.value = normalized.guide_mode
  guideCustomType.value = normalized.guide_custom_type
  guideUrl.value = normalized.guide_url
  guideHtml.value = normalized.guide_html
}

async function fetchSiteInfo() {
  try {
    const response = await apiClient.get<SiteInfo>('/api/public/site-info')
    applySiteInfo(response.data)
  } catch {
    if (!siteName.value || !siteSubtitle.value) {
      applySiteInfo(DEFAULT_SITE_INFO)
    }
    fetchPromise = null
  } finally {
    loaded.value = true
  }
}

async function refreshSiteInfo() {
  fetchPromise = null
  loaded.value = false
  fetchPromise = fetchSiteInfo()
  await fetchPromise
}

export function useSiteInfo() {
  if (!loaded.value && !fetchPromise) {
    fetchPromise = fetchSiteInfo()
  }
  return {
    siteName,
    siteSubtitle,
    showGithubLink,
    guideMode,
    guideCustomType,
    guideUrl,
    guideHtml,
    siteInfoLoaded: readonly(loaded),
    refreshSiteInfo,
  }
}

export function resolveGuideRedirectPath(path: string, mode: GuideMode): string | null {
  if (!path.startsWith('/guide')) {
    return null
  }
  if (mode === 'hidden') {
    return '/'
  }
  if (mode === 'custom' && path !== '/guide' && path !== '/guide/') {
    return '/guide'
  }
  return null
}

export { isPublicHttpUrl }

watch(siteName, (name) => {
  if (name) {
    document.title = name
  }
}, { immediate: true })
