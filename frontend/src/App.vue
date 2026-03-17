<template>
  <RouterView />
  <ToastContainer />
  <ConfirmContainer />
</template>

<script setup lang="ts">
import { onMounted, onErrorCaptured, onUnmounted } from 'vue'
import { useAuthStore } from '@/stores/auth'
import ToastContainer from '@/components/ToastContainer.vue'
import ConfirmContainer from '@/components/ConfirmContainer.vue'
import apiClient, { AUTH_STATE_CHANGE_EVENT } from '@/api/client'
import { NETWORK_CONFIG, AUTH_CONFIG } from '@/config/constants'
import router from '@/router'
import { hasAuthIdentityChanged } from '@/utils/authToken'
import { log } from '@/utils/logger'

const authStore = useAuthStore()

// 立即检查token,如果存在就设置到store中
const storedToken = apiClient.getToken()
if (storedToken) {
  authStore.token = storedToken
}

// 全局错误处理器 - 只处理特定错误,避免完全吞掉所有错误
onErrorCaptured((error: Error) => {
  log.error('Error captured in component', error)
  // 对于非关键错误,不阻止传播
  return true
})

// 统一的模块加载错误处理
let moduleLoadFailureCount = 0

if (typeof window !== 'undefined') {
  // 处理未捕获的 Promise 拒绝
  window.addEventListener('unhandledrejection', (event) => {
    const error = event.reason

    // 只处理模块加载失败的情况
    if (error?.message?.includes('Failed to fetch dynamically imported module')) {
      event.preventDefault() // 阻止控制台显示这个特定错误

      if (moduleLoadFailureCount < NETWORK_CONFIG.MODULE_LOAD_RETRY_LIMIT) {
        moduleLoadFailureCount++
        log.info(`模块加载失败,尝试刷新页面 (${moduleLoadFailureCount}/${NETWORK_CONFIG.MODULE_LOAD_RETRY_LIMIT})`)
        window.location.reload()
      } else {
        // 超过最大重试次数,显示友好提示
        alert('页面加载失败,请手动刷新浏览器。如问题持续,请清除浏览器缓存后重试。')
      }
      return
    }

    // 其他 Promise 错误记录日志
    log.error('Unhandled promise rejection', event.reason)
  })

  // 处理全局错误
  window.addEventListener('error', (event) => {
    // 过滤掉常见的无害警告
    const harmlessWarnings = [
      'ResizeObserver loop completed with undelivered notifications',
      'ResizeObserver loop limit exceeded'
    ]

    const isHarmless = harmlessWarnings.some(warning =>
      event.message?.includes(warning)
    )

    if (isHarmless) {
      event.preventDefault()
      return
    }

    // 记录其他错误
    if (event.error) {
      log.error('Global error', event.error)
    } else {
      log.warn('Global error event', {
        message: event.message,
        filename: event.filename,
        lineno: event.lineno,
        colno: event.colno
      })
    }
  })
}

async function syncExternalAuthState(nextToken: string | null): Promise<void> {
  const previousToken = authStore.token
  const previousUser = authStore.user
    ? {
        id: authStore.user.id,
        role: authStore.user.role,
      }
    : null

  authStore.syncToken()

  if (!nextToken) {
    if (previousToken || previousUser) {
      authStore.applyExternalLogout()
      await router.replace('/')
    }
    return
  }

  const identityChanged = hasAuthIdentityChanged(previousToken, nextToken, previousUser)
  if (!identityChanged && previousUser) {
    return
  }

  const user = await authStore.fetchCurrentUser()
  if (!user) {
    return
  }

  if (router.currentRoute.value.path.startsWith('/admin') && user.role !== 'admin') {
    await router.replace('/dashboard')
  }
}

function handleAuthStorageChange(event: StorageEvent): void {
  if (event.key !== 'access_token') {
    return
  }

  syncExternalAuthState(event.newValue).catch((err) => log.error('syncExternalAuthState failed', err))
}

function handleLocalAuthStateChange(event: Event): void {
  const authEvent = event as CustomEvent<{ token: string | null }>
  syncExternalAuthState(authEvent.detail?.token ?? apiClient.getToken()).catch((err) => log.error('syncExternalAuthState failed', err))
}

onMounted(async () => {
  if (typeof window !== 'undefined') {
    window.addEventListener('storage', handleAuthStorageChange)
    window.addEventListener(AUTH_STATE_CHANGE_EVENT, handleLocalAuthStateChange as (event: Event) => void)
  }

  // 延迟检查认证状态,让页面先加载
  setTimeout(async () => {
    try {
      await authStore.checkAuth()
    } catch (error) {
      // 即使checkAuth失败,也不要做任何会导致退出的操作
      log.warn('Auth check failed, but keeping session', error)
    }
  }, AUTH_CONFIG.TOKEN_REFRESH_INTERVAL)
})

onUnmounted(() => {
  if (typeof window !== 'undefined') {
    window.removeEventListener('storage', handleAuthStorageChange)
    window.removeEventListener(
      AUTH_STATE_CHANGE_EVENT,
      handleLocalAuthStateChange as (event: Event) => void,
    )
  }
})
</script>
