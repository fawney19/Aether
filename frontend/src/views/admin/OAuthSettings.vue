<template>
  <PageContainer>
    <PageHeader
      title="OAuth 配置"
      description="配置 OAuth Providers（登录/绑定）"
    >
      <template #actions>
        <Button
          variant="outline"
          :disabled="loading"
          @click="loadAll"
        >
          刷新
        </Button>
      </template>
    </PageHeader>

    <div class="mt-6 flex gap-6">
      <!-- 左侧边栏 -->
      <div class="w-56 shrink-0 flex flex-col gap-2">
        <button
          class="flex items-center justify-center gap-1.5 w-full px-3 py-2 rounded-lg border border-dashed border-border text-sm text-muted-foreground hover:border-primary/50 hover:text-primary transition-colors"
          @click="handleClickAdd"
        >
          <Plus class="w-3.5 h-3.5" />
          添加配置
        </button>

        <div
          v-if="configuredList.length === 0 && !loading"
          class="text-sm text-muted-foreground px-2 py-4 text-center"
        >
          暂无配置
        </div>

        <div class="space-y-0.5">
          <!-- 新建临时条目 -->
          <button
            v-if="newConfigPending"
            class="w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm transition-colors"
            :class="selectedType === '__new__' ? 'bg-primary/10 text-primary' : 'text-foreground hover:bg-muted'"
            @click="selectNewConfig()"
          >
            <div
              class="w-7 h-7 rounded-md flex items-center justify-center text-xs font-semibold shrink-0"
              :class="selectedType === '__new__' ? 'bg-primary text-primary-foreground' : 'bg-muted text-muted-foreground'"
            >
              +
            </div>
            <div class="flex-1 min-w-0 text-left">
              <div class="truncate font-medium text-sm">
                新配置
              </div>
              <div class="text-[10px] text-muted-foreground">
                未保存
              </div>
            </div>
          </button>

          <button
            v-for="item in sidebarList"
            :key="item.provider_type"
            class="w-full flex items-center gap-3 px-3 py-2.5 rounded-lg text-sm transition-colors"
            :class="selectedType === item.provider_type
              ? 'bg-primary/10 text-primary'
              : 'text-foreground hover:bg-muted'"
            @click="selectProvider(item.provider_type)"
          >
            <!-- Logo / 首字母 -->
            <div
              class="w-7 h-7 rounded-md shrink-0 flex items-center justify-center text-xs font-semibold overflow-hidden relative"
              :class="selectedType === item.provider_type ? 'bg-primary text-primary-foreground' : 'bg-muted text-muted-foreground'"
            >
              {{ item.display_name.charAt(0).toUpperCase() }}
              <img
                v-if="item.provider_type === 'linuxdo'"
                src="https://cdn.linux.do/uploads/default/optimized/3X/9/d/9dd49731091ce8656243f3c2b6e5d5e5a7e3e3e3_2_32x32.png"
                class="absolute inset-0 w-full h-full object-cover"
                @error="($event.target as HTMLImageElement).remove()"
              >
            </div>
            <div class="flex-1 min-w-0 text-left">
              <div class="truncate font-medium text-sm">
                {{ item.display_name }}
              </div>
              <div class="text-[10px] text-muted-foreground">
                {{ item.configured ? (item.is_enabled ? '已启用' : '已禁用') : '未配置' }}
              </div>
            </div>
            <!-- 开关 -->
            <Switch
              v-if="item.configured"
              :model-value="item.is_enabled"
              :disabled="saving"
              @click.stop
              @update:model-value="toggleProviderEnabled(item.provider_type, $event)"
            />
            <span
              v-else
              class="w-1.5 h-1.5 rounded-full shrink-0 bg-gray-200"
            />
          </button>
        </div>
      </div>

      <!-- 右侧内容区 -->
      <div class="flex-1 min-w-0">
        <!-- 配置表单 -->
        <CardSection
          v-if="selectedType"
          :title="selectedType === '__new__' ? '新建配置' : (selectedTypeMeta?.display_name || selectedType)"
          :description="selectedType === '__new__' ? '填写后点击保存' : (configs[selectedType]?.is_enabled ? '已启用' : '已禁用')"
        >
          <template #actions>
            <div class="flex gap-2">
              <Button
                size="sm"
                variant="outline"
                :disabled="saving || testing"
                @click="handleTest"
              >
                {{ testing ? '测试中...' : '测试' }}
              </Button>
              <Button
                size="sm"
                :disabled="saving"
                @click="handleSave"
              >
                {{ saving ? '保存中...' : '保存' }}
              </Button>
            </div>
          </template>

          <div class="space-y-6">
            <!-- 新建时的 Display Name -->
            <div
              v-if="selectedType === '__new__'"
              class="grid grid-cols-1 md:grid-cols-2 gap-4"
            >
              <div>
                <Label class="block text-sm font-medium">显示名称</Label>
                <Input
                  v-model="form.new_display_name"
                  class="mt-1"
                  placeholder="例如：My OIDC Provider"
                  autocomplete="off"
                />
              </div>
              <div>
                <Label class="block text-sm font-medium">配置标识</Label>
                <Input
                  v-model="form.new_provider_type"
                  class="mt-1"
                  placeholder="custom_oidc_work"
                  autocomplete="off"
                  @blur="normalizeNewProviderType"
                />
              </div>
            </div>

            <!-- 凭证配置 -->
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <Label class="block text-sm font-medium">Client ID</Label>
                <Input
                  v-model="form.client_id"
                  class="mt-1"
                  placeholder="client_id"
                  autocomplete="off"
                />
              </div>
              <div>
                <Label class="block text-sm font-medium">Client Secret</Label>
                <Input
                  v-model="form.client_secret"
                  masked
                  class="mt-1"
                  :placeholder="hasSecret ? '已设置（留空保持不变）' : '请输入 secret'"
                />
              </div>
            </div>

            <!-- 回调地址 -->
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <Label class="block text-sm font-medium">Redirect URI（后端回调）</Label>
                <Input
                  v-model="form.redirect_uri"
                  class="mt-1"
                  placeholder="http://localhost:8084/api/oauth/xxx/callback"
                  autocomplete="off"
                />
              </div>
              <div>
                <Label class="block text-sm font-medium">前端回调页</Label>
                <Input
                  v-model="form.frontend_callback_url"
                  class="mt-1"
                  placeholder="http://localhost:5173/auth/callback"
                  autocomplete="off"
                />
              </div>
            </div>

            <!-- custom_oidc 必填端点 -->
            <div
              v-if="isSelectedCustomProvider"
              class="grid grid-cols-1 md:grid-cols-3 gap-4"
            >
              <div>
                <Label class="block text-sm font-medium">Authorization URL</Label>
                <Input
                  v-model="form.authorization_url_override"
                  class="mt-1"
                  placeholder="https://example.com/oauth/authorize"
                  autocomplete="off"
                />
              </div>
              <div>
                <Label class="block text-sm font-medium">Token URL</Label>
                <Input
                  v-model="form.token_url_override"
                  class="mt-1"
                  placeholder="https://example.com/oauth/token"
                  autocomplete="off"
                />
              </div>
              <div>
                <Label class="block text-sm font-medium">Userinfo URL</Label>
                <Input
                  v-model="form.userinfo_url_override"
                  class="mt-1"
                  placeholder="https://example.com/api/user"
                  autocomplete="off"
                />
              </div>
            </div>

            <!-- 图标 URL -->
            <div>
              <Label class="block text-sm font-medium">图标 URL</Label>
              <Input
                v-model="form.icon_url"
                class="mt-1"
                placeholder="https://example.com/icon.svg"
                autocomplete="off"
              />
              <p class="mt-1 text-xs text-muted-foreground">
                登录页显示的 Provider 图标，留空使用默认图标
              </p>
            </div>

            <!-- 高级选项（折叠） -->
            <details class="group">
              <summary class="cursor-pointer text-sm font-medium text-muted-foreground hover:text-foreground transition-colors">
                高级选项
              </summary>
              <div class="mt-4 space-y-4 pl-4 border-l-2 border-border">
                <div>
                  <Label class="block text-sm font-medium">Scopes</Label>
                  <Input
                    v-model="form.scopes_input"
                    class="mt-1"
                    :placeholder="selectedTypeMeta?.default_scopes?.join(' ') || '留空使用默认值'"
                    autocomplete="off"
                  />
                  <p class="mt-1 text-xs text-muted-foreground">
                    空格/逗号分隔；留空使用默认值
                  </p>
                </div>

                <!-- linuxdo 的可选端点覆盖 -->
                <div
                  v-if="!isSelectedCustomProvider"
                  class="grid grid-cols-1 md:grid-cols-3 gap-4"
                >
                  <div>
                    <Label class="block text-sm font-medium">Authorization URL</Label>
                    <Input
                      v-model="form.authorization_url_override"
                      class="mt-1"
                      :placeholder="selectedTypeMeta?.default_authorization_url || '默认'"
                      autocomplete="off"
                    />
                  </div>
                  <div>
                    <Label class="block text-sm font-medium">Token URL</Label>
                    <Input
                      v-model="form.token_url_override"
                      class="mt-1"
                      :placeholder="selectedTypeMeta?.default_token_url || '默认'"
                      autocomplete="off"
                    />
                  </div>
                  <div>
                    <Label class="block text-sm font-medium">Userinfo URL</Label>
                    <Input
                      v-model="form.userinfo_url_override"
                      class="mt-1"
                      :placeholder="selectedTypeMeta?.default_userinfo_url || '默认'"
                      autocomplete="off"
                    />
                  </div>
                </div>

                <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                  <div>
                    <Label class="block text-sm font-medium">Attribute Mapping</Label>
                    <Textarea
                      v-model="form.attribute_mapping_json"
                      class="mt-1 font-mono text-xs"
                      rows="3"
                      placeholder="{&quot;id&quot;: &quot;user_id&quot;, &quot;username&quot;: &quot;login&quot;}"
                    />
                  </div>
                  <div>
                    <Label class="block text-sm font-medium">
                      {{ isSelectedCustomProvider ? 'Allowed Domains / Extra Config' : 'Extra Config' }}
                    </Label>
                    <Textarea
                      v-model="form.extra_config_json"
                      class="mt-1 font-mono text-xs"
                      rows="3"
                      :placeholder="extraConfigPlaceholder"
                    />
                    <p
                      v-if="isSelectedCustomProvider"
                      class="mt-1 text-xs text-muted-foreground"
                    >
                      自定义 OIDC 必填；填写 Authorization / Token / Userinfo URL 所属域名。
                    </p>
                  </div>
                </div>
              </div>
            </details>
          </div>
          <div
            v-if="lastTestResult"
            class="mt-6 rounded-lg border border-border p-4 text-sm"
          >
            <div class="font-medium mb-2">
              测试结果
            </div>
            <div class="flex flex-wrap gap-4 text-xs">
              <div class="flex items-center gap-2">
                <span
                  class="w-2 h-2 rounded-full"
                  :class="lastTestResult.authorization_url_reachable ? 'bg-green-500' : 'bg-red-500'"
                />
                <span class="text-muted-foreground">Authorization URL</span>
              </div>
              <div class="flex items-center gap-2">
                <span
                  class="w-2 h-2 rounded-full"
                  :class="lastTestResult.token_url_reachable ? 'bg-green-500' : 'bg-red-500'"
                />
                <span class="text-muted-foreground">Token URL</span>
              </div>
              <div class="flex items-center gap-2">
                <span
                  class="w-2 h-2 rounded-full"
                  :class="lastTestResult.secret_status === 'likely_valid' ? 'bg-green-500' : lastTestResult.secret_status === 'invalid' ? 'bg-red-500' : 'bg-yellow-500'"
                />
                <span class="text-muted-foreground">Secret: {{ lastTestResult.secret_status }}</span>
              </div>
              <span
                v-if="lastTestResult.details"
                class="text-muted-foreground"
              >
                {{ lastTestResult.details }}
              </span>
            </div>
          </div>
        </CardSection>
      </div>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Plus } from 'lucide-vue-next'
import { oauthApi, type OAuthProviderAdminConfig, type OAuthProviderTestResponse, type SupportedOAuthType } from '@/api/oauth'
import { PageContainer, PageHeader, CardSection } from '@/components/layout'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Switch from '@/components/ui/switch.vue'
import Textarea from '@/components/ui/textarea.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { log } from '@/utils/logger'
import { getErrorMessage, getErrorStatus, isApiError } from '@/types/api-error'
import { summarizeOAuthConfigTest } from '@/utils/oauthConfigTest'

const { success, warning, error: showError } = useToast()
const { confirmWarning } = useConfirm()

const loading = ref(false)
const saving = ref(false)
const testing = ref(false)

const BUILTIN_OAUTH_PROVIDER_TYPES = new Set(['linuxdo'])
const CUSTOM_OIDC_TEMPLATE_TYPE = 'custom_oidc'

interface OAuthConfigForm {
  client_id: string
  client_secret: string
  authorization_url_override: string
  token_url_override: string
  userinfo_url_override: string
  scopes_input: string
  redirect_uri: string
  frontend_callback_url: string
  attribute_mapping_json: string
  extra_config_json: string
  icon_url: string
  new_provider_type: string
  new_display_name: string
}

const supportedTypes = ref<SupportedOAuthType[]>([])
const configs = ref<Record<string, OAuthProviderAdminConfig>>({})
const selectedType = ref<string>('')
const lastTestResult = ref<OAuthProviderTestResponse | null>(null)
const newConfigPending = ref(false)

const form = ref<OAuthConfigForm>({
  client_id: '',
  client_secret: '',
  authorization_url_override: '',
  token_url_override: '',
  userinfo_url_override: '',
  scopes_input: '',
  redirect_uri: '',
  frontend_callback_url: '',
  attribute_mapping_json: '',
  extra_config_json: '',
  icon_url: '',
  new_provider_type: '',
  new_display_name: '',
})

const newConfigForm = ref<OAuthConfigForm | null>(null)

const configuredList = computed(() => Object.values(configs.value))

const customOidcTemplate = computed(() =>
  supportedTypes.value.find((type) => type.provider_type === CUSTOM_OIDC_TEMPLATE_TYPE)
)

function isBuiltinProviderType(providerType: string): boolean {
  return BUILTIN_OAUTH_PROVIDER_TYPES.has(providerType)
}

function isCustomProviderType(providerType: string): boolean {
  return !!providerType && !isBuiltinProviderType(providerType)
}

const sidebarList = computed(() => {
  const builtins = supportedTypes.value
    .filter((t) => isBuiltinProviderType(t.provider_type))
    .map((t) => ({
      ...t,
      ...(configs.value[t.provider_type] || {}),
      configured: !!configs.value[t.provider_type],
      is_enabled: configs.value[t.provider_type]?.is_enabled ?? false,
    }))
  const customTemplate = customOidcTemplate.value
  const customs = Object.values(configs.value)
    .filter((config) => isCustomProviderType(config.provider_type))
    .map((config) => ({
      ...(customTemplate || {
        provider_type: config.provider_type,
        display_name: config.display_name,
        default_authorization_url: '',
        default_token_url: '',
        default_userinfo_url: '',
        default_scopes: ['openid', 'profile', 'email'],
      }),
      ...config,
      configured: true,
      is_enabled: config.is_enabled,
    }))
  return [...builtins, ...customs]
})

const hasSecret = computed(() => !!configs.value[selectedType.value]?.has_secret)
const selectedTypeMeta = computed(() => {
  if (selectedType.value === '__new__') {
    return customOidcTemplate.value
  }
  const builtin = supportedTypes.value.find((t) => t.provider_type === selectedType.value)
  if (builtin) {
    return builtin
  }
  const config = configs.value[selectedType.value]
  if (!config) {
    return undefined
  }
  return {
    ...(customOidcTemplate.value || {
      default_authorization_url: '',
      default_token_url: '',
      default_userinfo_url: '',
      default_scopes: ['openid', 'profile', 'email'],
    }),
    provider_type: config.provider_type,
    display_name: config.display_name,
  }
})
const isSelectedCustomProvider = computed(() =>
  selectedType.value === '__new__' || isCustomProviderType(selectedType.value)
)
const extraConfigPlaceholder = computed(() =>
  isSelectedCustomProvider.value
    ? '{\n  "allowed_domains": ["example.com"]\n}'
    : '{}'
)

function selectProvider(providerType: string) {
  if (selectedType.value !== providerType) {
    if (selectedType.value === '__new__') {
      newConfigForm.value = { ...form.value }
    }
    selectedType.value = providerType
    syncFormFromSelected()
  }
}

function selectNewConfig() {
  if (selectedType.value !== '__new__') {
    selectedType.value = '__new__'
    if (newConfigForm.value) {
      form.value = { ...newConfigForm.value }
    }
    lastTestResult.value = null
  }
}

function normalizeProviderType(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, '_')
    .replace(/_+/g, '_')
    .replace(/^[-_]+|[-_]+$/g, '')
}

function isAllowedNewCustomProviderType(providerType: string): boolean {
  return providerType === CUSTOM_OIDC_TEMPLATE_TYPE
    || providerType.startsWith('custom_oidc_')
    || providerType.startsWith('custom_')
    || providerType.startsWith('oidc_')
}

function generateUniqueCustomProviderType(base = CUSTOM_OIDC_TEMPLATE_TYPE): string {
  const rawBase = normalizeProviderType(base) || CUSTOM_OIDC_TEMPLATE_TYPE
  const normalizedBase = isAllowedNewCustomProviderType(rawBase)
    ? rawBase
    : `custom_${rawBase}`
  const used = new Set(Object.keys(configs.value))
  if (!used.has(normalizedBase)) {
    return normalizedBase
  }
  let index = 2
  while (used.has(`${normalizedBase}_${index}`)) {
    index += 1
  }
  return `${normalizedBase}_${index}`
}

function ensureNewProviderType(): string {
  const normalized = normalizeProviderType(form.value.new_provider_type)
  const providerType = normalized && isAllowedNewCustomProviderType(normalized) && !configs.value[normalized]
    ? normalized
    : generateUniqueCustomProviderType(normalized || CUSTOM_OIDC_TEMPLATE_TYPE)
  form.value.new_provider_type = providerType
  return providerType
}

function normalizeNewProviderType() {
  const providerType = ensureNewProviderType()
  const redirectUri = form.value.redirect_uri.trim()
  if (!redirectUri || /\/api\/oauth\/[^/]+\/callback\/?$/.test(redirectUri)) {
    form.value.redirect_uri = defaultRedirectUri(providerType)
  }
  newConfigForm.value = { ...form.value }
}

function handleClickAdd() {
  const providerType = generateUniqueCustomProviderType()
  selectedType.value = '__new__'
  newConfigPending.value = true
  form.value = {
    client_id: '',
    client_secret: '',
    authorization_url_override: '',
    token_url_override: '',
    userinfo_url_override: '',
    scopes_input: 'openid profile email',
    redirect_uri: defaultRedirectUri(providerType),
    frontend_callback_url: defaultFrontendCallbackUrl(),
    attribute_mapping_json: '',
    extra_config_json: '',
    icon_url: '',
    new_provider_type: providerType,
    new_display_name: '',
  }
  newConfigForm.value = { ...form.value }
  lastTestResult.value = null
}

function defaultRedirectUri(providerType: string): string {
  return new URL(`/api/oauth/${providerType}/callback`, window.location.origin).toString()
}

function defaultFrontendCallbackUrl(): string {
  return new URL('/auth/callback', window.location.origin).toString()
}

function parseScopes(input: string): string[] | null {
  const raw = input.trim()
  if (!raw) return null
  const parts = raw.split(/[,\s]+/).map((s) => s.trim()).filter(Boolean)
  return parts.length ? parts : null
}

function parseJsonOrNull(input: string): Record<string, unknown> | null {
  const raw = input.trim()
  if (!raw) return null
  return JSON.parse(raw)
}


function syncFormFromSelected() {
  lastTestResult.value = null
  const cfg = configs.value[selectedType.value]

  form.value = {
    client_id: cfg?.client_id || '',
    client_secret: '',
    authorization_url_override: cfg?.authorization_url_override || '',
    token_url_override: cfg?.token_url_override || '',
    userinfo_url_override: cfg?.userinfo_url_override || '',
    scopes_input: (cfg?.scopes || []).join(' '),
    redirect_uri: cfg?.redirect_uri || defaultRedirectUri(selectedType.value),
    frontend_callback_url: cfg?.frontend_callback_url || defaultFrontendCallbackUrl(),
    attribute_mapping_json: cfg?.attribute_mapping ? JSON.stringify(cfg.attribute_mapping, null, 2) : '',
    extra_config_json: cfg?.extra_config ? JSON.stringify(cfg.extra_config, null, 2) : '',
    icon_url: cfg?.icon_url || '',
    new_provider_type: '',
    new_display_name: '',
  }
}

async function toggleProviderEnabled(providerType: string, enabled: boolean, force = false) {
  const cfg = configs.value[providerType]
  if (!cfg) {
    showError('请先保存配置后再启用')
    return
  }

  saving.value = true
  try {
    const payload = {
      display_name: cfg.display_name,
      client_id: cfg.client_id,
      authorization_url_override: cfg.authorization_url_override || null,
      token_url_override: cfg.token_url_override || null,
      userinfo_url_override: cfg.userinfo_url_override || null,
      scopes: cfg.scopes || null,
      redirect_uri: cfg.redirect_uri,
      frontend_callback_url: cfg.frontend_callback_url,
      attribute_mapping: cfg.attribute_mapping || null,
      extra_config: cfg.extra_config || null,
      icon_url: cfg.icon_url || null,
      is_enabled: enabled,
      force,
    }
    await oauthApi.admin.upsertProviderConfig(providerType, payload)
    success(enabled ? '已启用' : '已禁用')
    await loadAll()
  } catch (err: unknown) {
    // 检查是否是需要确认的冲突错误
    if (isApiError(err) && getErrorStatus(err) === 409) {
      const errorData = err.response?.data?.error
      if (errorData?.type === 'confirmation_required') {
        const affectedCount = errorData.details?.affected_count ?? 0
        const confirmed = await confirmWarning(
          `禁用该 Provider 会导致 ${affectedCount} 个用户无法登录，是否继续？`,
          '确认禁用'
        )
        if (confirmed) {
          await toggleProviderEnabled(providerType, enabled, true)
        }
        return
      }
    }
    showError(getErrorMessage(err, '操作失败'))
  } finally {
    saving.value = false
  }
}

async function loadAll() {
  loading.value = true
  try {
    const [types, list] = await Promise.all([
      oauthApi.admin.getSupportedTypes(),
      oauthApi.admin.listProviderConfigs(),
    ])
    supportedTypes.value = types
    configs.value = Object.fromEntries(list.map((c) => [c.provider_type, c]))
    newConfigPending.value = false

    if (!selectedType.value || selectedType.value === '__new__') {
      const first = list[0]?.provider_type || types[0]?.provider_type
      if (first) {
        selectedType.value = first
        syncFormFromSelected()
      }
    }
  } catch (err: unknown) {
    log.error('加载 OAuth 配置失败:', err)
    showError(getErrorMessage(err, '加载失败'))
  } finally {
    loading.value = false
  }
}

async function handleSave() {
  if (!selectedType.value) return
  saving.value = true
  lastTestResult.value = null
  try {
    const isNew = selectedType.value === '__new__'
    const providerType = isNew ? ensureNewProviderType() : selectedType.value
    const existingConfig = configs.value[providerType]
    const payload = {
      display_name: isNew ? (form.value.new_display_name.trim() || 'Custom OIDC') : (configs.value[providerType]?.display_name || supportedTypes.value.find((t) => t.provider_type === providerType)?.display_name || providerType),
      client_id: form.value.client_id.trim(),
      client_secret: form.value.client_secret.trim() || undefined,
      authorization_url_override: form.value.authorization_url_override.trim() || null,
      token_url_override: form.value.token_url_override.trim() || null,
      userinfo_url_override: form.value.userinfo_url_override.trim() || null,
      scopes: parseScopes(form.value.scopes_input),
      redirect_uri: form.value.redirect_uri.trim(),
      frontend_callback_url: form.value.frontend_callback_url.trim(),
      attribute_mapping: parseJsonOrNull(form.value.attribute_mapping_json),
      extra_config: parseJsonOrNull(form.value.extra_config_json),
      icon_url: form.value.icon_url.trim() || null,
      is_enabled: existingConfig?.is_enabled || false,
    }

    await oauthApi.admin.upsertProviderConfig(providerType, payload)
    success('保存成功')
    const savedType = providerType
    await loadAll()
    selectedType.value = savedType
    syncFormFromSelected()
  } catch (err: unknown) {
    showError(getErrorMessage(err, '保存失败'))
  } finally {
    saving.value = false
    form.value.client_secret = ''
  }
}

async function handleTest() {
  if (!selectedType.value) return
  testing.value = true
  try {
    const providerType = selectedType.value === '__new__' ? ensureNewProviderType() : selectedType.value
    const testPayload = {
      client_id: form.value.client_id.trim(),
      client_secret: form.value.client_secret.trim() || undefined,
      authorization_url_override: form.value.authorization_url_override.trim() || null,
      token_url_override: form.value.token_url_override.trim() || null,
      redirect_uri: form.value.redirect_uri.trim(),
    }
    const result = await oauthApi.admin.testProviderConfig(providerType, testPayload)
    lastTestResult.value = result
    const summary = summarizeOAuthConfigTest(result)
    if (summary.severity === 'success') {
      success(summary.message)
    } else if (summary.severity === 'warning') {
      warning(summary.message)
    } else {
      showError(summary.message)
    }
  } catch (err: unknown) {
    showError(getErrorMessage(err, '测试失败'))
  } finally {
    testing.value = false
  }
}

onMounted(async () => {
  await loadAll()
})
</script>
