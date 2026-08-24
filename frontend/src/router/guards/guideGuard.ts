import type { RouteLocationNormalized } from 'vue-router'
import { resolveGuideRedirectPath, useSiteInfo } from '@/composables/useSiteInfo'

export async function resolveGuideRedirect(
  to: RouteLocationNormalized,
): Promise<string | null> {
  if (!to.path.startsWith('/guide')) {
    return null
  }

  const { siteInfoLoaded, refreshSiteInfo, guideMode } = useSiteInfo()
  if (!siteInfoLoaded.value) {
    await refreshSiteInfo()
  }

  return resolveGuideRedirectPath(to.path, guideMode.value)
}
