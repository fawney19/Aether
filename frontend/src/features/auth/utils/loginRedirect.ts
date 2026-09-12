import { isNavigationFailure, NavigationFailureType, type Router } from 'vue-router'
import { safeInternalNavigationPath } from '@/utils/navigationSecurity'

export type LoginNavigationResult = 'router' | 'already-there' | 'document'

type DocumentNavigate = (targetPath: string) => void

function defaultDocumentNavigate(targetPath: string) {
  window.location.assign(targetPath)
}

export async function navigateAfterLogin(
  router: Router,
  targetPath: string,
  documentNavigate: DocumentNavigate = defaultDocumentNavigate,
): Promise<LoginNavigationResult> {
  const safeTargetPath = safeInternalNavigationPath(targetPath) ?? '/'
  try {
    const navigationFailure = await router.push(safeTargetPath)

    if (isNavigationFailure(navigationFailure, NavigationFailureType.duplicated)) {
      return 'already-there'
    }

    if (navigationFailure) {
      documentNavigate(safeTargetPath)
      return 'document'
    }

    return 'router'
  } catch {
    documentNavigate(safeTargetPath)
    return 'document'
  }
}
