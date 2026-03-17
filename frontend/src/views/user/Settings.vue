<template>
  <div class="container mx-auto px-4 py-8">
    <h2 class="text-2xl font-bold text-foreground mb-6">
      个人设置
    </h2>

    <div class="grid grid-cols-1 lg:grid-cols-3 gap-6">
      <!-- 左侧：个人信息和密码 -->
      <div class="lg:col-span-2 space-y-6">
        <!-- 基本信息 -->
        <Card class="p-6">
          <form
            class="space-y-4"
            @submit.prevent="updateProfile"
          >
            <div class="flex items-center justify-between">
              <h3 class="text-lg font-medium text-foreground">
                基本信息
              </h3>
              <Button
                type="submit"
                :disabled="savingProfile || !hasProfileChanges"
                class="shadow-none hover:shadow-none"
              >
                {{ savingProfile ? '保存中...' : '保存' }}
              </Button>
            </div>

            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <Label for="username">用户名</Label>
                <Input
                  id="username"
                  v-model="profileForm.username"
                  class="mt-1"
                />
              </div>
              <div>
                <Label for="avatar">头像 URL</Label>
                <Input
                  id="avatar"
                  v-model="preferencesForm.avatar_url"
                  type="url"
                  class="mt-1"
                />
              </div>
            </div>

            <div>
              <Label for="bio">个人简介</Label>
              <Textarea
                id="bio"
                v-model="preferencesForm.bio"
                rows="3"
                class="mt-1"
              />
            </div>

            <!-- 邮箱字段：当系统配置了邮箱服务或用户已有邮箱时显示 -->
            <div
              v-if="emailConfigured || profileForm.email"
              class="grid grid-cols-1 md:grid-cols-2 gap-4"
            >
              <div>
                <Label for="email">邮箱</Label>
                <Input
                  id="email"
                  v-model="profileForm.email"
                  type="email"
                  class="mt-1"
                  :disabled="!emailConfigured"
                />
                <p
                  v-if="!emailConfigured && profileForm.email"
                  class="mt-1 text-xs text-muted-foreground"
                >
                  邮箱服务未配置，暂不可修改
                </p>
              </div>
            </div>
          </form>
        </Card>

        <!-- 密码设置（LDAP 用户不显示） -->
        <Card
          v-if="profile?.auth_source !== 'ldap'"
          class="p-6"
        >
          <form
            class="space-y-4"
            @submit.prevent="changePassword"
          >
            <div class="flex items-center justify-between">
              <h3 class="text-lg font-medium text-foreground">
                {{ profile?.has_password ? '修改密码' : '设置密码' }}
              </h3>
              <Button
                type="submit"
                :disabled="changingPassword || !hasPasswordChanges"
                class="shadow-none hover:shadow-none"
              >
                {{ changingPassword ? '保存中...' : '保存' }}
              </Button>
            </div>
            <div v-if="profile?.has_password">
              <Label for="old-password">当前密码</Label>
              <Input
                id="old-password"
                v-model="passwordForm.old_password"
                type="password"
                class="mt-1"
              />
            </div>
            <div>
              <Label for="new-password">{{ profile?.has_password ? '新密码' : '密码' }}</Label>
              <Input
                id="new-password"
                v-model="passwordForm.new_password"
                type="password"
                :placeholder="getPasswordPolicyPlaceholder(passwordPolicyLevel)"
                class="mt-1"
              />
              <p
                v-if="passwordError"
                class="mt-1 text-xs text-destructive"
              >
                {{ passwordError }}
              </p>
              <p
                v-else
                class="mt-1 text-xs text-muted-foreground"
              >
                {{ passwordPolicyHint }}
              </p>
            </div>
            <div>
              <Label for="confirm-password">确认{{ profile?.has_password ? '新' : '' }}密码</Label>
              <Input
                id="confirm-password"
                v-model="passwordForm.confirm_password"
                type="password"
                placeholder="再次输入密码"
                class="mt-1"
              />
              <p
                v-if="passwordForm.confirm_password && passwordForm.new_password !== passwordForm.confirm_password"
                class="mt-1 text-xs text-destructive"
              >
                两次输入的密码不一致
              </p>
            </div>
          </form>
        </Card>

        <Card class="p-6">
          <div class="flex items-center justify-between mb-4">
            <div>
              <h3 class="text-lg font-medium text-foreground">
                登录设备
              </h3>
              <p class="text-sm text-muted-foreground mt-1">
                管理当前账号在各设备上的登录状态
              </p>
            </div>
            <Button
              variant="outline"
              :disabled="sessionsLoading || otherSessionCount === 0 || sessionActionLoading === 'others'"
              @click="handleRevokeOtherSessions"
            >
              {{ sessionActionLoading === 'others' ? '处理中...' : '退出其他设备' }}
            </Button>
          </div>

          <div
            v-if="sessionsLoading"
            class="text-sm text-muted-foreground"
          >
            正在加载设备列表...
          </div>
          <div
            v-else-if="userSessions.length === 0"
            class="text-sm text-muted-foreground"
          >
            暂无登录设备记录
          </div>
          <div
            v-else
            class="space-y-3"
          >
            <div
              v-for="session in userSessions"
              :key="session.id"
              class="flex items-start justify-between gap-4 rounded-lg border border-border/60 bg-muted/20 p-4"
            >
              <div class="min-w-0">
                <div class="flex items-center gap-2 flex-wrap">
                  <template v-if="editingSessionId === session.id">
                    <Input
                      v-model="sessionLabelDraft"
                      size="sm"
                      class="h-8 w-56"
                      maxlength="120"
                      @keyup.enter="saveSessionLabel(session.id)"
                    />
                  </template>
                  <span
                    v-else
                    class="font-medium text-foreground"
                  >{{ session.device_label }}</span>
                  <Badge
                    v-if="session.is_current"
                    variant="secondary"
                  >
                    当前设备
                  </Badge>
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  {{ formatSessionMeta(session) }}
                </p>
                <p class="mt-1 text-xs text-muted-foreground">
                  最近活跃 {{ formatDate(session.last_seen_at || session.created_at) }}
                  <span v-if="session.ip_address"> · IP {{ session.ip_address }}</span>
                </p>
              </div>
              <div class="flex items-center gap-2">
                <template v-if="editingSessionId === session.id">
                  <Button
                    size="sm"
                    :disabled="sessionActionLoading === session.id || !sessionLabelDraft.trim()"
                    @click="saveSessionLabel(session.id)"
                  >
                    {{ sessionActionLoading === session.id ? '保存中...' : '保存' }}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    :disabled="sessionActionLoading === session.id"
                    @click="cancelSessionLabelEdit"
                  >
                    取消
                  </Button>
                </template>
                <template v-else>
                  <Button
                    variant="outline"
                    size="sm"
                    :disabled="sessionActionLoading !== null"
                    @click="startSessionLabelEdit(session)"
                  >
                    重命名
                  </Button>
                  <Button
                    v-if="!session.is_current"
                    variant="outline"
                    size="sm"
                    :disabled="sessionActionLoading === session.id"
                    @click="handleRevokeSession(session.id)"
                  >
                    {{ sessionActionLoading === session.id ? '处理中...' : '退出' }}
                  </Button>
                </template>
              </div>
            </div>
          </div>
        </Card>

        <!-- OAuth 绑定 -->
        <Card class="p-6">
          <h3 class="text-lg font-medium text-foreground mb-4">
            OAuth 绑定
          </h3>

          <div
            v-if="profile?.auth_source === 'ldap'"
            class="text-sm text-muted-foreground"
          >
            LDAP 用户不支持 OAuth 绑定
          </div>

          <div
            v-else-if="oauthUnavailable"
            class="text-sm text-muted-foreground"
          >
            OAuth 模块未启用或暂不可用
          </div>

          <div
            v-else
            class="space-y-4"
          >
            <!-- 合并已绑定和可绑定为卡片网格 -->
            <div
              v-if="oauthLinks.length === 0 && bindableProviders.length === 0"
              class="text-sm text-muted-foreground"
            >
              暂无可用的 OAuth Provider
            </div>
            <div
              v-else
              class="grid grid-cols-1 sm:grid-cols-2 gap-3"
            >
              <!-- 已绑定的 Provider -->
              <div
                v-for="link in oauthLinks"
                :key="link.provider_type"
                class="flex items-center justify-between gap-3 rounded-lg border border-border bg-muted/30 p-4"
              >
                <div class="flex items-center gap-3 min-w-0 flex-1">
                  <!-- eslint-disable vue/no-v-html -->
                  <div
                    class="oauth-icon shrink-0"
                    v-html="getOAuthIcon(link.provider_type)"
                  />
                  <!-- eslint-enable vue/no-v-html -->
                  <div class="min-w-0">
                    <div class="text-sm font-medium truncate">
                      {{ link.display_name }}
                    </div>
                    <div class="text-xs text-muted-foreground truncate">
                      {{ link.provider_username || link.provider_email || '已绑定' }}
                    </div>
                  </div>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  :disabled="oauthActionLoading"
                  @click="handleUnbind(link.provider_type)"
                >
                  解绑
                </Button>
              </div>

              <!-- 可绑定的 Provider -->
              <div
                v-for="p in bindableProviders"
                :key="p.provider_type"
                class="flex items-center justify-between gap-3 rounded-lg border border-dashed border-border p-4 hover:border-primary/50 transition-colors"
              >
                <div class="flex items-center gap-3 min-w-0 flex-1">
                  <!-- eslint-disable vue/no-v-html -->
                  <div
                    class="oauth-icon shrink-0"
                    v-html="getOAuthIcon(p.provider_type)"
                  />
                  <!-- eslint-enable vue/no-v-html -->
                  <div class="min-w-0">
                    <div class="text-sm font-medium truncate">
                      {{ p.display_name }}
                    </div>
                    <div class="text-xs text-muted-foreground">
                      未绑定
                    </div>
                  </div>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  :disabled="oauthActionLoading"
                  @click="handleBind(p.provider_type)"
                >
                  绑定
                </Button>
              </div>
            </div>
          </div>
        </Card>

        <!-- 偏好设置 -->
        <Card class="p-6">
          <h3 class="text-lg font-medium text-foreground mb-4">
            偏好设置
          </h3>
          <div class="space-y-4">
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
              <div>
                <Label for="theme">主题</Label>
                <Select
                  v-model="preferencesForm.theme"
                  v-model:open="themeSelectOpen"
                  @update:model-value="handleThemeChange"
                >
                  <SelectTrigger
                    id="theme"
                    class="mt-1"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="light">
                      浅色
                    </SelectItem>
                    <SelectItem value="dark">
                      深色
                    </SelectItem>
                    <SelectItem value="system">
                      跟随系统
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div>
                <Label for="language">语言</Label>
                <Select
                  v-model="preferencesForm.language"
                  v-model:open="languageSelectOpen"
                  @update:model-value="handleLanguageChange"
                >
                  <SelectTrigger
                    id="language"
                    class="mt-1"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="zh-CN">
                      简体中文
                    </SelectItem>
                    <SelectItem value="en">
                      English
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>

              <div>
                <Label for="timezone">时区</Label>
                <Input
                  id="timezone"
                  v-model="preferencesForm.timezone"
                  placeholder="Asia/Shanghai"
                  class="mt-1"
                />
              </div>
            </div>

            <div class="space-y-3">
              <h4 class="font-medium text-foreground">
                通知设置
              </h4>
              <div class="space-y-3">
                <!-- 邮件通知：仅当系统配置了邮箱服务时显示 -->
                <div
                  v-if="emailConfigured"
                  class="flex items-center justify-between py-2 border-b border-border/40 last:border-0"
                >
                  <div class="flex-1">
                    <Label
                      for="email-notifications"
                      class="text-sm font-medium cursor-pointer"
                    >
                      邮件通知
                    </Label>
                    <p class="text-xs text-muted-foreground mt-1">
                      接收系统重要通知
                    </p>
                  </div>
                  <Switch
                    id="email-notifications"
                    v-model="preferencesForm.notifications.email"
                    @update:model-value="updatePreferences"
                  />
                </div>
                <div class="flex items-center justify-between py-2 border-b border-border/40 last:border-0">
                  <div class="flex-1">
                    <Label
                      for="usage-alerts"
                      class="text-sm font-medium cursor-pointer"
                    >
                      使用提醒
                    </Label>
                    <p class="text-xs text-muted-foreground mt-1">
                      当余额接近不足时提醒
                    </p>
                  </div>
                  <Switch
                    id="usage-alerts"
                    v-model="preferencesForm.notifications.usage_alerts"
                    @update:model-value="updatePreferences"
                  />
                </div>
                <div class="flex items-center justify-between py-2">
                  <div class="flex-1">
                    <Label
                      for="announcement-notifications"
                      class="text-sm font-medium cursor-pointer"
                    >
                      公告通知
                    </Label>
                    <p class="text-xs text-muted-foreground mt-1">
                      接收系统公告
                    </p>
                  </div>
                  <Switch
                    id="announcement-notifications"
                    v-model="preferencesForm.notifications.announcements"
                    @update:model-value="updatePreferences"
                  />
                </div>
              </div>
            </div>
          </div>
        </Card>
      </div>

      <!-- 右侧：账户信息和使用量 -->
      <div class="space-y-6">
        <!-- 账户信息 -->
        <Card class="p-6">
          <h3 class="text-lg font-medium text-foreground mb-4">
            账户信息
          </h3>
          <div class="space-y-3">
            <div class="flex justify-between">
              <span class="text-muted-foreground">角色</span>
              <Badge :variant="profile?.role === 'admin' ? 'default' : 'secondary'">
                {{ profile?.role === 'admin' ? '管理员' : '普通用户' }}
              </Badge>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">账户状态</span>
              <span :class="profile?.is_active ? 'text-success' : 'text-destructive'">
                {{ profile?.is_active ? '活跃' : '停用' }}
              </span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">注册时间</span>
              <span class="text-foreground">
                {{ formatDate(profile?.created_at) }}
              </span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">最后登录</span>
              <span class="text-foreground">
                {{ profile?.last_login_at ? formatDate(profile.last_login_at) : '未记录' }}
              </span>
            </div>
          </div>
        </Card>

        <!-- 钱包状态 -->
        <Card class="p-6">
          <h3 class="text-lg font-medium text-foreground mb-4">
            钱包状态
          </h3>
          <div class="space-y-4">
            <div class="flex justify-between text-sm">
              <span class="text-muted-foreground">总余额</span>
              <span class="text-foreground">
                <template v-if="isUnlimitedBilling()">
                  无限制
                </template>
                <template v-else>
                  {{ formatCurrency(profile?.billing?.balance || 0) }}
                </template>
              </span>
            </div>
            <div class="flex justify-between text-sm">
              <span class="text-muted-foreground">充值余额</span>
              <span class="text-foreground">{{ formatCurrency(profile?.billing?.recharge_balance || 0) }}</span>
            </div>
            <div class="flex justify-between text-sm">
              <span class="text-muted-foreground">赠款余额</span>
              <span class="text-foreground">{{ formatCurrency(profile?.billing?.gift_balance || 0) }}</span>
            </div>
            <div class="flex justify-between text-sm">
              <span class="text-muted-foreground">累计消费</span>
              <span class="text-foreground">{{ formatCurrency(profile?.billing?.total_consumed || 0) }}</span>
            </div>

            <div v-if="!isUnlimitedBilling()">
              <div class="flex justify-between text-sm mb-1">
                <span class="text-muted-foreground">累计消费占比</span>
                <span class="text-foreground">{{ getBillingUsagePercentage().toFixed(1) }}%</span>
              </div>
              <div class="w-full bg-muted rounded-full h-2.5">
                <div
                  class="bg-success h-2.5 rounded-full"
                  :style="`width: ${getBillingUsagePercentage()}%`"
                />
              </div>
            </div>
          </div>
        </Card>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import { meApi, type Profile } from '@/api/me'
import { type UserSession, formatSessionMeta } from '@/types/session'
import { authApi } from '@/api/auth'
import { oauthApi, type OAuthLinkInfo, type OAuthProviderInfo } from '@/api/oauth'
import { getClientDeviceId } from '@/utils/deviceId'
import { getOAuthIcon } from '@/utils/oauth-icons'
import { useDarkMode, type ThemeMode } from '@/composables/useDarkMode'
import {
  getPasswordPolicyHint,
  getPasswordPolicyPlaceholder,
  normalizePasswordPolicyLevel,
  validatePasswordByPolicy,
  type PasswordPolicyLevel,
} from '@/utils/passwordPolicy'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Textarea from '@/components/ui/textarea.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import Switch from '@/components/ui/switch.vue'
import { useToast } from '@/composables/useToast'
import { formatCurrency } from '@/utils/format'
import { getApiUrl } from '@/utils/url'
import { log } from '@/utils/logger'
import { getErrorMessage, getErrorStatus } from '@/types/api-error'

const authStore = useAuthStore()
const route = useRoute()
const router = useRouter()
const { success, error: showError } = useToast()
const { setThemeMode } = useDarkMode()

const profile = ref<Profile | null>(null)
const userSessions = ref<UserSession[]>([])

const profileForm = ref({
  email: '',
  username: ''
})

const passwordForm = ref({
  old_password: '',
  new_password: '',
  confirm_password: ''
})

const preferencesForm = ref({
  avatar_url: '',
  bio: '',
  theme: 'light',
  language: 'zh-CN',
  timezone: 'Asia/Shanghai',
  notifications: {
    email: true,
    usage_alerts: true,
    announcements: true
  }
})

const savingProfile = ref(false)
const changingPassword = ref(false)
const sessionsLoading = ref(false)
const sessionActionLoading = ref<string | null>(null)
const editingSessionId = ref<string | null>(null)
const sessionLabelDraft = ref('')
const passwordPolicyLevel = ref<PasswordPolicyLevel>('weak')
const themeSelectOpen = ref(false)
const languageSelectOpen = ref(false)

const oauthUnavailable = ref(false)
const oauthActionLoading = ref(false)
const oauthLinks = ref<OAuthLinkInfo[]>([])
const bindableProviders = ref<OAuthProviderInfo[]>([])
const emailConfigured = ref(false) // 系统是否配置了邮箱服务

// 原始值，用于检测是否有修改
const originalProfileForm = ref({ email: '', username: '' })
const originalPreferencesForm = ref({ avatar_url: '', bio: '' })

// 检测基本信息是否有修改
const hasProfileChanges = computed(() => {
  return (
    profileForm.value.username !== originalProfileForm.value.username ||
    profileForm.value.email !== originalProfileForm.value.email ||
    preferencesForm.value.avatar_url !== originalPreferencesForm.value.avatar_url ||
    preferencesForm.value.bio !== originalPreferencesForm.value.bio
  )
})

const passwordPolicyHint = computed(() => getPasswordPolicyHint(passwordPolicyLevel.value))
const passwordError = computed(() =>
  validatePasswordByPolicy(passwordForm.value.new_password, passwordPolicyLevel.value)
)

// 检测密码表单是否有内容
const hasPasswordChanges = computed(() => {
  const hasPassword = profile.value?.has_password
  if (hasPassword) {
    // 已有密码：需要填写旧密码和新密码
    return !!(passwordForm.value.old_password && passwordForm.value.new_password && passwordForm.value.confirm_password)
  } else {
    // 设置密码：只需要填写新密码
    return !!(passwordForm.value.new_password && passwordForm.value.confirm_password)
  }
})

const otherSessionCount = computed(() => userSessions.value.filter((session) => !session.is_current).length)

function handleThemeChange(value: string) {
  preferencesForm.value.theme = value
  themeSelectOpen.value = false
  updatePreferences()

  // 使用 useDarkMode 统一切换主题
  setThemeMode(value as ThemeMode)
}

function handleLanguageChange(value: string) {
  preferencesForm.value.language = value
  languageSelectOpen.value = false
  updatePreferences()
}

onMounted(async () => {
  await loadProfile()
  await Promise.all([
    loadPreferences(),
    loadSessions(),
    loadOAuthBindings(),
    loadEmailConfigured(),
  ])
})

async function loadEmailConfigured() {
  try {
    const settings = await authApi.getRegistrationSettings()
    emailConfigured.value = !!settings.email_configured
    passwordPolicyLevel.value = normalizePasswordPolicyLevel(settings.password_policy_level)
  } catch {
    emailConfigured.value = false
    passwordPolicyLevel.value = 'weak'
  }
}

async function loadProfile() {
  try {
    profile.value = await meApi.getProfile()
    profileForm.value = {
      email: profile.value.email || '',
      username: profile.value.username
    }
    // 保存原始值
    originalProfileForm.value = { ...profileForm.value }
  } catch (error) {
    log.error('加载个人信息失败:', error)
    showError('加载个人信息失败')
  }
}

async function loadSessions() {
  sessionsLoading.value = true
  try {
    userSessions.value = await meApi.listSessions()
    if (editingSessionId.value) {
      const currentEditing = userSessions.value.find((session) => session.id === editingSessionId.value)
      if (!currentEditing) {
        cancelSessionLabelEdit()
      }
    }
  } catch (error) {
    log.error('加载登录设备失败:', error)
  } finally {
    sessionsLoading.value = false
  }
}

async function loadOAuthBindings() {
  oauthUnavailable.value = false
  oauthLinks.value = []
  bindableProviders.value = []

  // profile 加载失败时跳过
  if (!profile.value) {
    oauthUnavailable.value = true
    return
  }

  // LDAP 用户不支持绑定
  if (profile.value.auth_source === 'ldap') {
    return
  }

  try {
    const [links, providers] = await Promise.all([
      oauthApi.getMyLinks(),
      oauthApi.getBindableProviders(),
    ])
    oauthLinks.value = links
    bindableProviders.value = providers
  } catch (err: unknown) {
    if (getErrorStatus(err) === 503) {
      oauthUnavailable.value = true
      return
    }
    log.error('加载 OAuth 绑定信息失败:', err)
    oauthUnavailable.value = true
  }
}

function handleBind(providerType: string) {
  // 保存返回路径（OAuth callback 会读取）
  sessionStorage.setItem('redirectPath', route.fullPath)

  // 先获取一次性绑定令牌，再在新标签页打开（避免在 URL 中暴露 access_token）
  oauthActionLoading.value = true
  oauthApi.createBindToken(providerType)
    .then((bindToken) => {
      // getApiUrl 可能返回相对路径，需要拼接完整 URL
      const basePath = getApiUrl(`/api/user/oauth/${providerType}/bind`)
      const bindUrl = basePath.startsWith('http')
        ? new URL(basePath)
        : new URL(basePath, window.location.origin)
      bindUrl.searchParams.set('bind_token', bindToken)
      bindUrl.searchParams.set('client_device_id', getClientDeviceId())

      // 新标签页打开 OAuth 流程
      const newTab = window.open(bindUrl.toString(), '_blank')

      // 监听标签页关闭，刷新绑定状态
      if (newTab) {
        const MAX_WAIT_MS = 10 * 60 * 1000 // 10 分钟超时
        const startTime = Date.now()
        const checkClosed = setInterval(() => {
          if (newTab.closed || Date.now() - startTime > MAX_WAIT_MS) {
            clearInterval(checkClosed)
            oauthActionLoading.value = false
            loadOAuthBindings()
          }
        }, 500)
      } else {
        // 被浏览器阻止，回退到当前页面跳转
        oauthActionLoading.value = false
        window.location.href = bindUrl.toString()
      }
    })
    .catch((err) => {
      oauthActionLoading.value = false
      showError(getErrorMessage(err, '获取绑定令牌失败'))
    })
}

async function handleUnbind(providerType: string) {
  oauthActionLoading.value = true
  try {
    await oauthApi.unbind(providerType)
    success('解绑成功')
    await loadOAuthBindings()
  } catch (err) {
    showError(getErrorMessage(err, '解绑失败'))
  } finally {
    oauthActionLoading.value = false
  }
}

async function loadPreferences() {
  try {
    const prefs = await meApi.getPreferences()

    // 主题以本地 localStorage 为准（useDarkMode 在应用启动时已初始化）
    // 这样可以避免刷新页面时主题被服务端旧值覆盖
    const { themeMode: currentThemeMode } = useDarkMode()
    const localTheme = currentThemeMode.value

    preferencesForm.value = {
      avatar_url: prefs.avatar_url || '',
      bio: prefs.bio || '',
      theme: localTheme,  // 使用本地主题，而非服务端返回值
      language: prefs.language || 'zh-CN',
      timezone: prefs.timezone || 'Asia/Shanghai',
      notifications: {
        email: prefs.notifications?.email ?? true,
        usage_alerts: prefs.notifications?.usage_alerts ?? true,
        announcements: prefs.notifications?.announcements ?? true
      }
    }

    // 保存原始值
    originalPreferencesForm.value = {
      avatar_url: preferencesForm.value.avatar_url,
      bio: preferencesForm.value.bio
    }

    // 如果本地主题和服务端不一致，同步到服务端（静默更新，不提示用户）
    const serverTheme = prefs.theme || 'light'
    if (localTheme !== serverTheme) {
      meApi.updatePreferences({ theme: localTheme }).catch(() => {
        // 静默失败，不影响用户体验
      })
    }
  } catch (error) {
    log.error('加载偏好设置失败:', error)
  }
}

async function updateProfile() {
  savingProfile.value = true
  try {
    await meApi.updateProfile(profileForm.value)

    // 同时更新偏好设置中的 avatar_url 和 bio
    await meApi.updatePreferences({
      avatar_url: preferencesForm.value.avatar_url || undefined,
      bio: preferencesForm.value.bio || undefined,
      theme: preferencesForm.value.theme,
      language: preferencesForm.value.language,
      timezone: preferencesForm.value.timezone || undefined,
      notifications: {
        email: preferencesForm.value.notifications.email,
        usage_alerts: preferencesForm.value.notifications.usage_alerts,
        announcements: preferencesForm.value.notifications.announcements
      }
    })

    // 更新原始值
    originalProfileForm.value = { ...profileForm.value }
    originalPreferencesForm.value = {
      avatar_url: preferencesForm.value.avatar_url,
      bio: preferencesForm.value.bio
    }

    success('个人信息已更新')
    authStore.fetchCurrentUser()
  } catch (err) {
    log.error('更新个人信息失败:', err)
    showError(getErrorMessage(err), '更新个人信息失败')
  } finally {
    savingProfile.value = false
  }
}

async function changePassword() {
  if (passwordForm.value.new_password !== passwordForm.value.confirm_password) {
    showError('两次输入的密码不一致', '密码错误')
    return
  }

  if (passwordError.value) {
    showError(passwordError.value, '密码错误')
    return
  }

  const isSettingPassword = !profile.value?.has_password
  changingPassword.value = true
  try {
    await meApi.changePassword({
      old_password: isSettingPassword ? undefined : passwordForm.value.old_password,
      new_password: passwordForm.value.new_password
    })
    success(isSettingPassword ? '密码设置成功，请重新登录' : '密码修改成功，请重新登录')
    await authStore.logout()
    await router.replace('/')
  } catch (err) {
    log.error('修改密码失败:', err)
    const title = isSettingPassword ? '密码设置失败' : '密码修改失败'
    const defaultMsg = isSettingPassword ? '请稍后重试' : '请检查当前密码是否正确'
    showError(getErrorMessage(err, defaultMsg), title)
  } finally {
    changingPassword.value = false
  }
}

function startSessionLabelEdit(session: UserSession) {
  editingSessionId.value = session.id
  sessionLabelDraft.value = session.device_label
}

function cancelSessionLabelEdit() {
  editingSessionId.value = null
  sessionLabelDraft.value = ''
}

async function saveSessionLabel(sessionId: string) {
  const nextLabel = sessionLabelDraft.value.trim()
  if (!nextLabel) {
    showError('设备名称不能为空')
    return
  }

  sessionActionLoading.value = sessionId
  try {
    const updated = await meApi.updateSessionLabel(sessionId, nextLabel)
    userSessions.value = userSessions.value.map((session) =>
      session.id === sessionId ? updated : session
    )
    cancelSessionLabelEdit()
    success('设备名称已更新')
  } catch (error) {
    log.error('更新设备名称失败:', error)
    showError(getErrorMessage(error, '更新设备名称失败'))
  } finally {
    sessionActionLoading.value = null
  }
}

async function handleRevokeSession(sessionId: string) {
  sessionActionLoading.value = sessionId
  try {
    await meApi.revokeSession(sessionId)
    if (editingSessionId.value === sessionId) {
      cancelSessionLabelEdit()
    }
    success('设备已退出登录')
    await loadSessions()
  } catch (error) {
    log.error('退出设备失败:', error)
    showError(getErrorMessage(error, '退出设备失败'))
  } finally {
    sessionActionLoading.value = null
  }
}

async function handleRevokeOtherSessions() {
  sessionActionLoading.value = 'others'
  try {
    const result = await meApi.revokeOtherSessions()
    success(result.revoked_count > 0 ? `已退出 ${result.revoked_count} 个其他设备` : '没有其他在线设备')
    await loadSessions()
  } catch (error) {
    log.error('退出其他设备失败:', error)
    showError(getErrorMessage(error, '退出其他设备失败'))
  } finally {
    sessionActionLoading.value = null
  }
}

async function updatePreferences() {
  try {
    await meApi.updatePreferences({
      avatar_url: preferencesForm.value.avatar_url || undefined,
      bio: preferencesForm.value.bio || undefined,
      theme: preferencesForm.value.theme,
      language: preferencesForm.value.language,
      timezone: preferencesForm.value.timezone || undefined,
      notifications: {
        email: preferencesForm.value.notifications.email,
        usage_alerts: preferencesForm.value.notifications.usage_alerts,
        announcements: preferencesForm.value.notifications.announcements
      }
    })
    success('设置已保存')
  } catch (error) {
    log.error('更新偏好设置失败:', error)
    showError('保存设置失败')
  }
}

function getBillingUsagePercentage(): number {
  const billing = profile.value?.billing
  if (!billing) return 0
  const consumed = billing.total_consumed || 0
  const denominator = consumed + (billing.balance || 0)
  if (denominator <= 0) return 0
  return Math.min(100, (consumed / denominator) * 100)
}

function isUnlimitedBilling(): boolean {
  return profile.value?.billing?.unlimited === true
}

function formatDate(dateString?: string): string {
  if (!dateString) return '未知'
  return new Date(dateString).toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
}
</script>

<style scoped>
.oauth-icon {
  width: 24px;
  height: 24px;
}

.oauth-icon :deep(svg) {
  width: 100%;
  height: 100%;
}
</style>
