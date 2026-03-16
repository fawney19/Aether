<template>
  <div class="min-h-screen flex items-center justify-center px-6">
    <Card class="w-full max-w-md p-6 space-y-2">
      <h1 class="text-lg font-semibold text-foreground">
        正在处理认证...
      </h1>
      <p class="text-sm text-muted-foreground">
        {{ hint }}
      </p>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import Card from '@/components/ui/card.vue'
import apiClient from '@/api/client'
import { useAuthStore } from '@/stores/auth'
import { useToast } from '@/composables/useToast'

const route = useRoute()
const router = useRouter()
const authStore = useAuthStore()
const { success, error: showError } = useToast()

const hint = ref('请稍候...')

function consumeRedirectPath(): string | null {
  const redirectPath = sessionStorage.getItem('redirectPath')
  if (redirectPath) {
    sessionStorage.removeItem('redirectPath')
    return redirectPath
  }
  return null
}

function clearUrlState() {
  // 清理 fragment，避免刷新时重复处理
  // 同时清理 query（oauth_bound / error_code / error_detail）
  const newUrl = window.location.pathname
  window.history.replaceState({}, document.title, newUrl)
}

function errorMessageFromCode(code: string): string {
  const map: Record<string, string> = {
    authorization_denied: '你已取消授权',
    provider_disabled: '该 OAuth Provider 已被禁用',
    provider_unavailable: 'OAuth Provider 不可用',
    invalid_callback: '回调参数无效',
    invalid_state: '登录状态已失效，请重试',
    token_exchange_failed: '令牌兑换失败',
    userinfo_fetch_failed: '获取用户信息失败',
    email_exists_local: '该邮箱已存在，请先登录后再绑定 OAuth',
    email_is_ldap: '该邮箱属于 LDAP 账号，请使用 LDAP 登录',
    email_is_oauth: '该邮箱已关联其他 OAuth 账号，请使用原账号登录',
    registration_disabled: '系统未开放注册，无法创建新账号',
    oauth_already_bound: '该第三方账号已被其他用户绑定',
    already_bound_provider: '你已绑定该 Provider',
    last_oauth_binding: '解绑失败：至少需要保留一个 OAuth 绑定',
    last_login_method: '解绑失败：解绑后将无法登录',
    ldap_no_oauth: 'LDAP 用户不支持 OAuth 绑定',
  }
  return map[code] || '认证失败，请重试'
}

onMounted(async () => {
  // 1) 绑定成功提示
  const oauthBound = route.query.oauth_bound
  if (typeof oauthBound === 'string' && oauthBound) {
    success(`已绑定 ${oauthBound}`)
    clearUrlState()
    const redirectPath = consumeRedirectPath()
    await router.replace(redirectPath || '/dashboard/settings')
    return
  }

  // 2) 错误提示
  const errorCode = route.query.error_code
  if (typeof errorCode === 'string' && errorCode) {
    showError(errorMessageFromCode(errorCode))
    clearUrlState()
    const redirectPath = consumeRedirectPath()
    await router.replace(redirectPath || '/')
    return
  }

  // 3) 登录成功：解析 fragment token
  const hash = window.location.hash.startsWith('#') ? window.location.hash.slice(1) : window.location.hash
  const params = new URLSearchParams(hash)
  const accessToken = params.get('access_token')

  clearUrlState()

  if (!accessToken) {
    showError('未获取到访问令牌')
    await router.replace('/')
    return
  }

  hint.value = '正在写入登录态...'
  apiClient.setToken(accessToken)

  authStore.syncToken()

  hint.value = '正在获取用户信息...'
  await authStore.fetchCurrentUser()

  success('登录成功')

  const redirectPath = consumeRedirectPath()
  const target = redirectPath || (authStore.user?.role === 'admin' ? '/admin/dashboard' : '/dashboard')
  await router.replace(target)
})
</script>
