<template>
  <PageContainer>
    <PageHeader
      title="通知服务"
      description="统一管理通知项、通知模板和推送服务选择"
    />

    <div class="mt-6 space-y-6">
      <CardSection
        title="通知服务配置"
        description="选择全局推送服务，并配置邮件和第三方推送渠道"
      >
        <template #actions>
          <Button
            size="sm"
            :disabled="saving"
            @click="saveConfig"
          >
            {{ saving ? '保存中...' : '保存' }}
          </Button>
        </template>

        <div class="space-y-6">
          <div class="grid gap-4 lg:grid-cols-[minmax(0,1fr)_320px]">
            <div>
              <Label class="block text-sm font-medium">
                全局推送服务
              </Label>
              <Select v-model="config.default_channel">
                <SelectTrigger class="mt-1">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    所有可用服务
                  </SelectItem>
                  <SelectItem value="email">
                    邮件
                  </SelectItem>
                  <SelectItem value="server_chan">
                    Server 酱
                  </SelectItem>
                  <SelectItem value="bark">
                    Bark
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div class="flex items-center justify-between gap-4">
              <div>
                <Label class="text-sm font-medium">
                  启用通知服务
                </Label>
                <p class="mt-1 text-xs text-muted-foreground">
                  {{ canEnableService ? '当前策略有可用推送服务' : '当前策略没有可用推送服务' }}
                </p>
              </div>
              <Switch
                v-model="config.enabled"
                :disabled="!canEnableService"
              />
            </div>
          </div>

          <div class="grid gap-6 border-t border-border/60 pt-5 lg:grid-cols-3">
            <section class="space-y-4">
              <div class="flex items-center justify-between gap-3">
                <div>
                  <div class="flex items-center gap-2">
                    <Label class="text-sm font-medium">
                      邮件配置
                    </Label>
                    <Badge :variant="emailReady ? 'success' : 'outline'">
                      {{ emailReady ? '可用' : '未就绪' }}
                    </Badge>
                  </div>
                  <p class="mt-1 text-xs text-muted-foreground">
                    SMTP 配置在
                    <RouterLink
                      to="/admin/email"
                      class="text-primary hover:underline"
                    >
                      邮件配置
                    </RouterLink>
                    中维护
                  </p>
                </div>
                <Switch
                  v-model="config.email_enabled"
                  :disabled="!smtpConfigured"
                />
              </div>

              <div>
                <Label
                  for="notification-service-recipients"
                  class="block text-sm font-medium"
                >
                  管理员收件人
                </Label>
                <Textarea
                  id="notification-service-recipients"
                  v-model="config.email_recipients"
                  rows="4"
                  placeholder="ops@example.com&#10;admin@example.com"
                  class="mt-1"
                />
              </div>
            </section>

            <section class="space-y-4">
              <div class="flex items-center justify-between gap-3">
                <div>
                  <div class="flex items-center gap-2">
                    <Label class="text-sm font-medium">
                      Server 酱
                    </Label>
                    <Badge :variant="serverChanReady ? 'success' : 'outline'">
                      {{ serverChanReady ? '可用' : '未就绪' }}
                    </Badge>
                  </div>
                  <p class="mt-1 text-xs text-muted-foreground">
                    第三方推送服务在扩展模块中独立启用
                  </p>
                </div>
              </div>

              <RouterLink
                to="/admin/modules/server-chan"
                class="inline-flex h-11 items-center rounded-xl border border-border/60 bg-card/60 px-4 text-sm font-semibold text-foreground hover:border-primary/60 hover:bg-primary/10 hover:text-primary"
              >
                配置 Server 酱推送
              </RouterLink>
            </section>

            <section class="space-y-4">
              <div class="flex items-center justify-between gap-3">
                <div>
                  <div class="flex items-center gap-2">
                    <Label class="text-sm font-medium">
                      Bark
                    </Label>
                    <Badge :variant="barkReady ? 'success' : 'outline'">
                      {{ barkReady ? '可用' : '未就绪' }}
                    </Badge>
                  </div>
                  <p class="mt-1 text-xs text-muted-foreground">
                    通过 Bark 向 iOS 设备推送通知
                  </p>
                </div>
              </div>

              <RouterLink
                to="/admin/modules/bark"
                class="inline-flex h-11 items-center rounded-xl border border-border/60 bg-card/60 px-4 text-sm font-semibold text-foreground hover:border-primary/60 hover:bg-primary/10 hover:text-primary"
              >
                配置 Bark 推送
              </RouterLink>
            </section>
          </div>
        </div>
      </CardSection>

      <CardSection
        title="通知项"
        description="每个通知项可以继承全局服务，也可以单独指定推送服务"
      >
        <template #actions>
          <Button
            size="sm"
            variant="outline"
            @click="addItem"
          >
            <Plus class="mr-1.5 h-4 w-4" />
            添加通知项
          </Button>
        </template>

        <div class="space-y-4">
          <div
            v-for="(item, index) in config.items"
            :key="item.local_id"
            class="rounded-lg border border-border/70 p-4"
          >
            <div class="flex flex-wrap items-start justify-between gap-3">
              <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-2">
                  <Label class="text-sm font-semibold">
                    {{ item.name || item.key || '未命名通知项' }}
                  </Label>
                  <Badge
                    v-if="item.system"
                    variant="outline"
                  >
                    内置
                  </Badge>
                  <Badge :variant="isItemReady(item) ? 'success' : 'outline'">
                    {{ isItemReady(item) ? '可投递' : '未就绪' }}
                  </Badge>
                </div>
                <p class="mt-1 truncate text-xs text-muted-foreground">
                  {{ item.key }}
                </p>
              </div>
              <div class="flex items-center gap-2">
                <Switch v-model="item.enabled" />
                <Button
                  v-if="!item.system"
                  size="icon"
                  variant="ghost"
                  @click="removeItem(index)"
                >
                  <Trash2 class="h-4 w-4" />
                </Button>
              </div>
            </div>

            <div class="mt-4 grid gap-4 lg:grid-cols-2">
              <div>
                <Label class="block text-xs font-medium">
                  通知键
                </Label>
                <Input
                  v-model="item.key"
                  class="mt-1"
                  :disabled="item.system"
                  placeholder="custom_event"
                />
              </div>
              <div>
                <Label class="block text-xs font-medium">
                  名称
                </Label>
                <Input
                  v-model="item.name"
                  class="mt-1"
                  placeholder="自定义通知"
                />
              </div>
              <div>
                <Label class="block text-xs font-medium">
                  推送服务
                </Label>
                <Select v-model="item.channel">
                  <SelectTrigger class="mt-1">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="global">
                      使用全局
                    </SelectItem>
                    <SelectItem value="all">
                      所有可用服务
                    </SelectItem>
                    <SelectItem value="email">
                      邮件
                    </SelectItem>
                    <SelectItem value="server_chan">
                      Server 酱
                    </SelectItem>
                    <SelectItem value="bark">
                      Bark
                    </SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div class="flex items-center justify-between rounded-lg border border-border/70 px-4 py-3">
                <div>
                  <Label class="text-xs font-medium">
                    用户邮件
                  </Label>
                  <p class="mt-1 text-xs text-muted-foreground">
                    允许发送到用户自己的邮箱
                  </p>
                </div>
                <Switch
                  v-model="item.user_email_enabled"
                  :disabled="!smtpConfigured"
                />
              </div>
            </div>

            <div class="mt-4 grid gap-4">
              <div>
                <Label class="block text-xs font-medium">
                  标题模板
                </Label>
                <Input
                  v-model="item.title_template"
                  class="mt-1"
                  placeholder="{title}"
                />
              </div>
              <div class="grid gap-4 lg:grid-cols-2">
                <div>
                  <Label class="block text-xs font-medium">
                    Markdown 模板
                  </Label>
                  <Textarea
                    v-model="item.markdown_template"
                    rows="5"
                    class="mt-1 font-mono text-sm"
                    placeholder="{body}"
                  />
                </div>
                <div>
                  <Label class="block text-xs font-medium">
                    文本模板
                  </Label>
                  <Textarea
                    v-model="item.text_template"
                    rows="5"
                    class="mt-1 font-mono text-sm"
                    placeholder="{text_body}"
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      </CardSection>

      <CardSection
        title="测试通知"
        description="按已保存配置发送测试通知"
      >
        <div class="grid gap-3 sm:grid-cols-[minmax(0,1fr)_180px_auto]">
          <Select v-model="testItemKey">
            <SelectTrigger>
              <SelectValue placeholder="选择通知项" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="item in config.items"
                :key="item.local_id"
                :value="item.key"
              >
                {{ item.name || item.key }}
              </SelectItem>
            </SelectContent>
          </Select>
          <Select v-model="testChannel">
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="global">
                按通知项
              </SelectItem>
              <SelectItem value="all">
                所有可用服务
              </SelectItem>
              <SelectItem value="email">
                邮件
              </SelectItem>
              <SelectItem value="server_chan">
                Server 酱
              </SelectItem>
              <SelectItem value="bark">
                Bark
              </SelectItem>
            </SelectContent>
          </Select>
          <Button
            variant="outline"
            :disabled="testing || !testItemKey"
            @click="handleTest"
          >
            <Send class="mr-1.5 h-4 w-4" />
            {{ testing ? '发送中...' : '发送测试' }}
          </Button>
        </div>

        <div
          v-if="lastTestResult.length > 0"
          class="mt-4 space-y-2"
        >
          <div
            v-for="item in lastTestResult"
            :key="item.channel"
            class="flex items-center justify-between gap-4 rounded-md border border-border px-3 py-2 text-sm"
          >
            <span>{{ formatChannel(item.channel) }}</span>
            <span :class="item.success ? 'text-green-600 dark:text-green-400' : 'text-destructive'">
              {{ item.message }}
            </span>
          </div>
        </div>
      </CardSection>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { RouterLink } from 'vue-router'
import { Plus, Send, Trash2 } from 'lucide-vue-next'
import {
  Badge,
  Button,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  Textarea,
} from '@/components/ui'
import { PageHeader, PageContainer, CardSection } from '@/components/layout'
import { adminApi } from '@/api/admin'
import { modulesApi, type ModuleStatus } from '@/api/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

type DeliveryChannel = 'global' | 'all' | 'email' | 'server_chan' | 'bark'

interface NotificationItem {
  local_id: string
  key: string
  name: string
  enabled: boolean
  channel: DeliveryChannel
  title_template: string
  markdown_template: string
  text_template: string
  user_email_enabled: boolean
  system: boolean
}

interface NotificationConfig {
  enabled: boolean
  email_enabled: boolean
  email_recipients: string
  default_channel: Exclude<DeliveryChannel, 'global'>
  items: NotificationItem[]
}

const CONFIG_KEYS = {
  enabled: 'module.important_notification.enabled',
  email_enabled: 'module.important_notification.email_enabled',
  email_recipients: 'module.important_notification.email_recipients',
  default_channel: 'module.important_notification.default_channel',
  items: 'module.important_notification.items',
  server_chan_send_key: 'module.server_chan_push.send_key',
  bark_device_key: 'module.bark_push.device_key',
} as const

const DEFAULT_ITEMS: NotificationItem[] = [
  {
    local_id: 'provider_quota_alert',
    key: 'provider_quota_alert',
    name: '号池额度不足',
    enabled: true,
    channel: 'global',
    title_template: '',
    markdown_template: '',
    text_template: '',
    user_email_enabled: false,
    system: true,
  },
  {
    local_id: 'provider_pool_abnormal',
    key: 'provider_pool_abnormal',
    name: '号池异常',
    enabled: true,
    channel: 'global',
    title_template: '号池异常：{provider_name}',
    markdown_template: '号池 `{provider_name}` 出现异常，请检查服务状态。',
    text_template: '号池 {provider_name} 出现异常，请检查服务状态。',
    user_email_enabled: false,
    system: true,
  },
  {
    local_id: 'user_balance_low',
    key: 'user_balance_low',
    name: '用户余额不足',
    enabled: true,
    channel: 'email',
    title_template: '余额不足提醒',
    markdown_template: '你的账户余额已低于提醒阈值，请及时处理。',
    text_template: '你的账户余额已低于提醒阈值，请及时处理。',
    user_email_enabled: true,
    system: true,
  },
]

const { success, error } = useToast()

const saving = ref(false)
const testing = ref(false)
const smtpConfigured = ref(false)
const serverChanKeyIsSet = ref(false)
const serverChanStatus = ref<ModuleStatus | null>(null)
const barkKeyIsSet = ref(false)
const barkStatus = ref<ModuleStatus | null>(null)
const testItemKey = ref('provider_quota_alert')
const testChannel = ref<DeliveryChannel>('global')
const lastTestResult = ref<Array<{ channel: string; success: boolean; message: string }>>([])

const config = ref<NotificationConfig>({
  enabled: false,
  email_enabled: false,
  email_recipients: '',
  default_channel: 'all',
  items: cloneDefaultItems(),
})

const emailReady = computed(() => {
  return config.value.email_enabled && smtpConfigured.value && config.value.email_recipients.trim() !== ''
})

const serverChanReady = computed(() => {
  return serverChanStatus.value?.enabled === true && serverChanKeyIsSet.value
})

const barkReady = computed(() => {
  return barkStatus.value?.enabled === true && barkKeyIsSet.value
})

const canEnableService = computed(() => {
  if (deliveryReady(config.value.default_channel)) return true
  return config.value.items.some(item => item.enabled && isItemReady(item))
})

onMounted(() => {
  loadConfig()
})

async function loadConfig() {
  try {
    const [
      moduleStatus,
      emailEnabled,
      recipients,
      defaultChannel,
      items,
      serverChanModuleStatus,
      serverChanKey,
      barkModuleStatus,
      barkDeviceKey,
      smtpHost,
      smtpFromEmail,
    ] = await Promise.all([
      modulesApi.getStatus('important_notification'),
      adminApi.getSystemConfig(CONFIG_KEYS.email_enabled),
      adminApi.getSystemConfig(CONFIG_KEYS.email_recipients),
      adminApi.getSystemConfig(CONFIG_KEYS.default_channel),
      adminApi.getSystemConfig(CONFIG_KEYS.items),
      modulesApi.getStatus('server_chan_push'),
      adminApi.getSystemConfig(CONFIG_KEYS.server_chan_send_key),
      modulesApi.getStatus('bark_push'),
      adminApi.getSystemConfig(CONFIG_KEYS.bark_device_key),
      adminApi.getSystemConfig('smtp_host'),
      adminApi.getSystemConfig('smtp_from_email'),
    ])

    config.value.enabled = moduleStatus.enabled === true
    config.value.email_enabled = emailEnabled.value === true
    config.value.email_recipients = normalizeRecipients(recipients.value)
    config.value.default_channel = normalizeDefaultChannel(defaultChannel.value)
    config.value.items = normalizeItems(items.value)
    serverChanStatus.value = serverChanModuleStatus
    serverChanKeyIsSet.value = serverChanKey.is_set === true
    barkStatus.value = barkModuleStatus
    barkKeyIsSet.value = barkDeviceKey.is_set === true
    smtpConfigured.value = isNonEmptyString(smtpHost.value) && isNonEmptyString(smtpFromEmail.value)
    if (!config.value.items.some(item => item.key === testItemKey.value)) {
      testItemKey.value = config.value.items[0]?.key || ''
    }
  } catch (err) {
    error(parseApiError(err, '加载通知服务配置失败'))
    log.error('加载通知服务配置失败:', err)
  }
}

async function saveConfig() {
  saving.value = true
  try {
    if (!canEnableService.value) {
      config.value.enabled = false
    }
    await Promise.all([
      adminApi.updateSystemConfig(CONFIG_KEYS.email_enabled, config.value.email_enabled, '通知服务邮件推送开关'),
      adminApi.updateSystemConfig(CONFIG_KEYS.email_recipients, config.value.email_recipients, '通知服务管理员收件人'),
      adminApi.updateSystemConfig(CONFIG_KEYS.default_channel, config.value.default_channel, '通知服务全局推送服务'),
      adminApi.updateSystemConfig(CONFIG_KEYS.items, serializeItems(), '通知服务通知项和模板'),
    ])
    await adminApi.updateSystemConfig(CONFIG_KEYS.enabled, config.value.enabled, '通知服务总开关')
    success('通知服务配置已保存')
  } catch (err) {
    error(parseApiError(err, '保存通知服务配置失败'))
    log.error('保存通知服务配置失败:', err)
  } finally {
    saving.value = false
  }
}

async function handleTest() {
  testing.value = true
  try {
    const result = await adminApi.testImportantNotification({
      item_key: testItemKey.value,
      channel: testChannel.value === 'global' ? undefined : testChannel.value,
    })
    lastTestResult.value = result.channels || []
    if (result.success) {
      success(result.message || '测试通知已发送')
    } else {
      error(result.message || '测试通知发送失败')
    }
  } catch (err) {
    error(parseApiError(err, '测试通知发送失败'))
    log.error('测试通知服务失败:', err)
  } finally {
    testing.value = false
  }
}

function addItem() {
  const suffix = Date.now().toString(36)
  const key = `custom_${suffix}`
  config.value.items.push({
    local_id: key,
    key,
    name: '自定义通知',
    enabled: true,
    channel: 'global',
    title_template: '',
    markdown_template: '',
    text_template: '',
    user_email_enabled: false,
    system: false,
  })
  testItemKey.value = key
}

function removeItem(index: number) {
  const [removed] = config.value.items.splice(index, 1)
  if (removed?.key === testItemKey.value) {
    testItemKey.value = config.value.items[0]?.key || ''
  }
}

function isItemReady(item: NotificationItem): boolean {
  if (!item.enabled) return false
  return deliveryReady(resolveItemChannel(item))
}

function deliveryReady(channel: Exclude<DeliveryChannel, 'global'>): boolean {
  if (channel === 'all') return emailReady.value || serverChanReady.value || barkReady.value
  if (channel === 'email') return emailReady.value
  if (channel === 'server_chan') return serverChanReady.value
  if (channel === 'bark') return barkReady.value
  return false
}

function resolveItemChannel(item: NotificationItem): Exclude<DeliveryChannel, 'global'> {
  return item.channel === 'global' ? config.value.default_channel : item.channel
}

function serializeItems() {
  return config.value.items.map(item => ({
    key: normalizeItemKey(item.key),
    name: item.name.trim() || normalizeItemKey(item.key),
    enabled: item.enabled,
    channel: item.channel,
    title_template: item.title_template.trim(),
    markdown_template: item.markdown_template.trim(),
    text_template: item.text_template.trim(),
    user_email_enabled: item.user_email_enabled,
    system: item.system,
  }))
}

function normalizeItems(value: unknown): NotificationItem[] {
  if (!Array.isArray(value)) return cloneDefaultItems()
  const items = value
    .map((item, index) => normalizeItem(item, index))
    .filter((item): item is NotificationItem => item !== null)
  return items.length > 0 ? items : cloneDefaultItems()
}

function normalizeItem(value: unknown, index: number): NotificationItem | null {
  if (!value || typeof value !== 'object') return null
  const raw = value as Record<string, unknown>
  const key = normalizeItemKey(raw.key)
  if (!key) return null
  return {
    local_id: `${key}_${index}`,
    key,
    name: typeof raw.name === 'string' && raw.name.trim() ? raw.name.trim() : key,
    enabled: raw.enabled !== false,
    channel: normalizeItemChannel(raw.channel),
    title_template: typeof raw.title_template === 'string' ? raw.title_template : '',
    markdown_template: typeof raw.markdown_template === 'string' ? raw.markdown_template : '',
    text_template: typeof raw.text_template === 'string' ? raw.text_template : '',
    user_email_enabled: raw.user_email_enabled === true,
    system: raw.system === true,
  }
}

function normalizeItemKey(value: unknown): string {
  if (typeof value !== 'string') return ''
  return value.trim().replace(/[^A-Za-z0-9_.:-]/g, '_').slice(0, 64)
}

function normalizeItemChannel(value: unknown): DeliveryChannel {
  if (value === 'all' || value === 'email' || value === 'server_chan' || value === 'bark') return value
  return 'global'
}

function normalizeDefaultChannel(value: unknown): Exclude<DeliveryChannel, 'global'> {
  if (value === 'email' || value === 'server_chan' || value === 'bark') return value
  return 'all'
}

function cloneDefaultItems(): NotificationItem[] {
  return DEFAULT_ITEMS.map(item => ({ ...item }))
}

function isNonEmptyString(value: unknown): boolean {
  return typeof value === 'string' && value.trim() !== ''
}

function normalizeRecipients(value: unknown): string {
  if (Array.isArray(value)) {
    return value
      .map(item => String(item).trim())
      .filter(Boolean)
      .join('\n')
  }
  return typeof value === 'string' ? value : ''
}

function formatChannel(channel: string): string {
  if (channel === 'email') return '邮件'
  if (channel === 'server_chan') return 'Server 酱'
  if (channel === 'bark') return 'Bark'
  if (channel === 'user_email') return '用户邮件'
  if (channel === 'module') return '模块'
  if (channel === 'item') return '通知项'
  if (channel === 'none') return '无可用服务'
  return channel
}
</script>
