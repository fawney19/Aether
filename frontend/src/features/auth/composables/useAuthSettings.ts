import { computed, ref } from 'vue'
import { authApi } from '@/api/auth'
import { oauthApi, type OAuthProviderInfo } from '@/api/oauth'

const allowRegistration = ref(false)
const requireEmailVerification = ref(false)
const emailConfigured = ref(false)

const localEnabled = ref(true)
const ldapEnabled = ref(false)
const ldapExclusive = ref(false)

const oauthProviders = ref<OAuthProviderInfo[]>([])

const loaded = ref(false)
let fetchPromise: Promise<void> | null = null

function applyDefaultSettings() {
  allowRegistration.value = false
  requireEmailVerification.value = false
  emailConfigured.value = false

  localEnabled.value = true
  ldapEnabled.value = false
  ldapExclusive.value = false

  oauthProviders.value = []
}

async function fetchSettings() {
  try {
    const [regSettings, authSettings, providers] = await Promise.all([
      authApi.getRegistrationSettings(),
      authApi.getAuthSettings(),
      oauthApi.getProviders().catch(() => []),
    ])

    allowRegistration.value = !!regSettings.enable_registration
    requireEmailVerification.value = !!regSettings.require_email_verification
    emailConfigured.value = !!regSettings.email_configured

    localEnabled.value = !!authSettings.local_enabled
    ldapEnabled.value = !!authSettings.ldap_enabled
    ldapExclusive.value = !!authSettings.ldap_exclusive

    oauthProviders.value = providers
  } catch {
    applyDefaultSettings()
  }
}

async function loadSettings(options?: { force?: boolean }) {
  const force = options?.force === true

  if (loaded.value && !force) {
    return
  }

  if (fetchPromise) {
    return fetchPromise
  }

  fetchPromise = (async () => {
    await fetchSettings()
    loaded.value = true
    fetchPromise = null
  })()

  return fetchPromise
}

const showAuthTypeTabs = computed(() => {
  return localEnabled.value && ldapEnabled.value && !ldapExclusive.value
})

const resolvedAuthType = computed<'local' | 'ldap'>(() => {
  if (ldapExclusive.value) return 'ldap'
  if (!localEnabled.value && ldapEnabled.value) return 'ldap'
  return 'local'
})

export function useAuthSettings() {
  return {
    allowRegistration,
    requireEmailVerification,
    emailConfigured,
    localEnabled,
    ldapEnabled,
    ldapExclusive,
    oauthProviders,
    loaded,
    loadSettings,
    showAuthTypeTabs,
    resolvedAuthType,
  }
}
