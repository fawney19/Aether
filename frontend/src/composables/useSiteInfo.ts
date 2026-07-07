import { readonly, ref, watch } from 'vue'
import apiClient from '@/api/client'

interface SiteInfo {
  site_name: string
  site_subtitle: string
}

const DEFAULT_SITE_INFO: SiteInfo = {
  site_name: 'Aether',
  site_subtitle: 'AI Gateway',
}

// 模块级缓存，所有组件共享同一份数据
const siteName = ref('')
const siteSubtitle = ref('')
const loaded = ref(false)
let fetchPromise: Promise<void> | null = null

function normalizeSiteInfo(data: Partial<SiteInfo> | null | undefined): SiteInfo {
  return {
    site_name: data?.site_name?.trim() || DEFAULT_SITE_INFO.site_name,
    site_subtitle: data?.site_subtitle?.trim() || DEFAULT_SITE_INFO.site_subtitle,
  }
}

function applySiteInfo(data: Partial<SiteInfo> | null | undefined): void {
  const normalized = normalizeSiteInfo(data)
  siteName.value = normalized.site_name
  siteSubtitle.value = normalized.site_subtitle
}

async function fetchSiteInfo() {
  try {
    const response = await apiClient.get<SiteInfo>('/api/public/site-info')
    applySiteInfo(response.data)
  } catch {
    // 加载失败时才使用 upstream 默认站点信息，避免配置加载前闪出默认品牌文案
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
  return { siteName, siteSubtitle, siteInfoLoaded: readonly(loaded), refreshSiteInfo }
}

// 站点名称变化时同步更新 document.title
watch(siteName, (name) => {
  if (name) {
    document.title = name
  }
}, { immediate: true })
