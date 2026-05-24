<template>
  <PageContainer>
    <PageHeader
      title="Bark 推送"
      description="第三方推送服务，用于通知服务的 Bark 渠道"
    />

    <div class="mt-6 space-y-6">
      <CardSection
        title="服务配置"
        description="配置 Bark Device Key、服务器地址和服务启用状态"
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

        <div class="space-y-5">
          <div class="flex items-center justify-between gap-4 rounded-lg border border-border/70 px-4 py-3">
            <div>
              <Label class="text-sm font-medium">
                启用 Bark 推送
              </Label>
              <p class="mt-1 text-xs text-muted-foreground">
                通知服务选择 Bark 时会检查此开关
              </p>
            </div>
            <Switch
              v-model="enabled"
              :disabled="!canEnable"
            />
          </div>

          <div class="grid gap-4 lg:grid-cols-2">
            <div>
              <Label
                for="bark-device-key"
                class="block text-sm font-medium"
              >
                Device Key
              </Label>
              <Input
                id="bark-device-key"
                v-model="deviceKeyInput"
                masked
                :placeholder="deviceKeyIsSet ? '已设置（留空保持不变）' : '从 Bark App 推送地址中获取'"
                class="mt-1"
              />
              <p class="mt-1 text-xs text-muted-foreground">
                Bark App 中推送地址
                <span class="font-mono">https://api.day.app/xxxx</span>
                的 <span class="font-mono">xxxx</span> 部分
              </p>
            </div>

            <div>
              <Label
                for="bark-server-url"
                class="block text-sm font-medium"
              >
                服务器地址
              </Label>
              <Input
                id="bark-server-url"
                v-model="serverUrlInput"
                placeholder="https://api.day.app"
                class="mt-1"
              />
              <p class="mt-1 text-xs text-muted-foreground">
                支持官方服务或自建 Bark Server，保存时会去掉末尾斜杠
              </p>
            </div>
          </div>
        </div>
      </CardSection>

      <CardSection
        title="通知模板"
        description="模板支持 {title} 和 {body} 变量"
      >
        <div>
          <Label
            for="bark-template"
            class="block text-sm font-medium"
          >
            模板内容
          </Label>
          <Textarea
            id="bark-template"
            v-model="templateInput"
            rows="10"
            class="mt-1 font-mono text-sm"
            placeholder="{body}"
            spellcheck="false"
          />
        </div>
      </CardSection>

      <CardSection
        title="测试服务"
        description="按已保存配置发送一条 Bark 测试通知"
      >
        <div class="flex flex-wrap gap-2">
          <Button
            variant="outline"
            :disabled="testing || !deviceKeyIsSet"
            @click="handleTest"
          >
            {{ testing ? '发送中...' : '发送测试' }}
          </Button>
          <RouterLink
            to="/admin/notification-service"
            class="inline-flex h-11 items-center rounded-xl px-3 text-sm text-primary hover:underline"
          >
            打开通知服务
          </RouterLink>
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
import { Button, Input, Label, Switch, Textarea } from '@/components/ui'
import { PageHeader, PageContainer, CardSection } from '@/components/layout'
import { adminApi } from '@/api/admin'
import { modulesApi } from '@/api/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

const CONFIG_KEYS = {
  enabled: 'module.bark_push.enabled',
  device_key: 'module.bark_push.device_key',
  server_url: 'module.bark_push.server_url',
  template: 'module.bark_push.template',
} as const

const DEFAULT_SERVER_URL = 'https://api.day.app'

const { success, error } = useToast()

const saving = ref(false)
const testing = ref(false)
const enabled = ref(false)
const deviceKeyIsSet = ref(false)
const deviceKeyInput = ref('')
const serverUrlInput = ref(DEFAULT_SERVER_URL)
const templateInput = ref('')
const lastTestResult = ref<Array<{ channel: string; success: boolean; message: string }>>([])

const canEnable = computed(() => deviceKeyIsSet.value || deviceKeyInput.value.trim() !== '')

onMounted(() => {
  loadConfig()
})

async function loadConfig() {
  try {
    const [moduleStatus, deviceKey, serverUrl, template] = await Promise.all([
      modulesApi.getStatus('bark_push'),
      adminApi.getSystemConfig(CONFIG_KEYS.device_key),
      adminApi.getSystemConfig(CONFIG_KEYS.server_url),
      adminApi.getSystemConfig(CONFIG_KEYS.template),
    ])

    enabled.value = moduleStatus.enabled === true
    deviceKeyIsSet.value = deviceKey.is_set === true
    deviceKeyInput.value = ''
    serverUrlInput.value = typeof serverUrl.value === 'string' && serverUrl.value.trim()
      ? serverUrl.value
      : DEFAULT_SERVER_URL
    templateInput.value = typeof template.value === 'string' ? template.value : ''
  } catch (err) {
    error(parseApiError(err, '加载 Bark 推送配置失败'))
    log.error('加载 Bark 推送配置失败:', err)
  }
}

async function saveConfig() {
  saving.value = true
  try {
    const updates: Array<Promise<unknown>> = [
      adminApi.updateSystemConfig(
        CONFIG_KEYS.server_url,
        normalizeServerUrl(serverUrlInput.value),
        'Bark 服务器地址'
      ),
      adminApi.updateSystemConfig(CONFIG_KEYS.template, templateInput.value, 'Bark 推送模板'),
    ]
    const trimmedKey = deviceKeyInput.value.trim()
    if (trimmedKey) {
      updates.push(adminApi.updateSystemConfig(
        CONFIG_KEYS.device_key,
        trimmedKey,
        'Bark Device Key'
      ))
    }
    await Promise.all(updates)
    if (trimmedKey) {
      deviceKeyIsSet.value = true
      deviceKeyInput.value = ''
    }
    if (!canEnable.value) {
      enabled.value = false
    }
    await modulesApi.setEnabled('bark_push', enabled.value)
    success('Bark 推送配置已保存')
  } catch (err) {
    error(parseApiError(err, '保存 Bark 推送配置失败'))
    log.error('保存 Bark 推送配置失败:', err)
  } finally {
    saving.value = false
  }
}

async function handleTest() {
  testing.value = true
  try {
    const result = await adminApi.testImportantNotification({ channel: 'bark' })
    lastTestResult.value = result.channels || []
    if (result.success) {
      success(result.message || '测试通知已发送')
    } else {
      error(result.message || '测试通知发送失败')
    }
  } catch (err) {
    error(parseApiError(err, '测试通知发送失败'))
    log.error('测试 Bark 推送失败:', err)
  } finally {
    testing.value = false
  }
}

function normalizeServerUrl(value: string): string {
  const trimmed = value.trim().replace(/\/+$/, '')
  return trimmed || DEFAULT_SERVER_URL
}

function formatChannel(channel: string): string {
  if (channel === 'bark') return 'Bark'
  if (channel === 'server_chan') return 'Server 酱'
  if (channel === 'email') return '邮件'
  if (channel === 'module') return '模块'
  if (channel === 'none') return '无可用服务'
  return channel
}
</script>
