<template>
  <div class="min-h-screen flex items-center justify-center px-6">
    <Card class="w-full max-w-md p-6 space-y-2">
      <h1 class="text-lg font-semibold text-foreground">
        {{ t('site.auth.processing') }}
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
import { useI18n } from '@/i18n'

const route = useRoute()
const router = useRouter()
const authStore = useAuthStore()
const { success, error: showError } = useToast()
const { t } = useI18n()

const hint = ref(t('site.auth.waiting'))

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
    authorization_denied: t('site.auth.cancelled'),
    provider_disabled: t('site.auth.providerDisabled'),
    provider_unavailable: t('site.auth.providerUnavailable'),
    invalid_callback: t('site.auth.invalidCallback'),
    invalid_state: t('site.auth.invalidState'),
    token_exchange_failed: t('site.auth.tokenExchangeFailed'),
    userinfo_fetch_failed: t('site.auth.userinfoFetchFailed'),
    email_exists_local: t('site.auth.emailExistsLocal'),
    email_is_ldap: t('site.auth.emailIsLdap'),
    email_is_oauth: t('site.auth.emailIsOauth'),
    registration_disabled: t('site.auth.registrationDisabled'),
    oauth_already_bound: t('site.auth.oauthAlreadyBound'),
    already_bound_provider: t('site.auth.alreadyBoundProvider'),
    last_oauth_binding: t('site.auth.lastOauthBinding'),
    last_login_method: t('site.auth.lastLoginMethod'),
    ldap_no_oauth: t('site.auth.ldapNoOauth'),
  }
  return map[code] || t('site.auth.failed')
}

onMounted(async () => {
  // 1) 绑定成功提示
  const oauthBound = route.query.oauth_bound
  if (typeof oauthBound === 'string' && oauthBound) {
    success(t('site.auth.bound', { provider: oauthBound }))
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
    showError(t('site.auth.noToken'))
    await router.replace('/')
    return
  }

  hint.value = t('site.auth.writing')
  apiClient.setToken(accessToken)

  authStore.syncToken()

  hint.value = t('site.auth.fetchingUser')
  await authStore.fetchCurrentUser()

  success(t('site.auth.success'))

  const redirectPath = consumeRedirectPath()
  const target = redirectPath || (authStore.canAccessAdmin ? '/admin/dashboard' : '/dashboard')
  await router.replace(target)
})
</script>
