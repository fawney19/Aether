<template>
  <PageContainer>
    <PageHeader
      title="Server 酱推送"
      description="第三方推送服务，用于通知服务的 Server 酱渠道"
    />

    <div class="mt-6 space-y-6">
      <CardSection
        title="服务配置"
        description="配置 Server 酱 Turbo SendKey 和服务启用状态"
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
                启用 Server 酱推送
              </Label>
              <p class="mt-1 text-xs text-muted-foreground">
                通知服务选择 Server 酱时会检查此开关
              </p>
            </div>
            <Switch
              v-model="enabled"
              :disabled="!canEnable"
            />
          </div>

          <div>
            <Label
              for="server-chan-send-key"
              class="block text-sm font-medium"
            >
              SendKey
            </Label>
            <Input
              id="server-chan-send-key"
              v-model="sendKeyInput"
              masked
              :placeholder="sendKeyIsSet ? '已设置（留空保持不变）' : 'SCTxxxxxxxxxxxxxxxxxxxxxxxx'"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              可在 <span class="font-mono">sct.ftqq.com</span> 控制台获取
            </p>
          </div>
        </div>
      </CardSection>

      <CardSection
        title="通知模板"
        description="Markdown 模板支持 {title} 和 {body} 变量"
      >
        <div>
          <Label
            for="server-chan-template"
            class="block text-sm font-medium"
          >
            模板内容
          </Label>
          <Textarea
            id="server-chan-template"
            v-model="templateInput"
            rows="10"
            class="mt-1 font-mono text-sm"
            placeholder="**{title}**&#10;&#10;{body}"
            spellcheck="false"
          />
        </div>
      </CardSection>

      <CardSection
        title="测试服务"
        description="按已保存配置发送一条 Server 酱测试通知"
      >
        <div class="flex flex-wrap gap-2">
          <Button
            variant="outline"
            :disabled="testing || !sendKeyIsSet"
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
  enabled: 'module.server_chan_push.enabled',
  send_key: 'module.server_chan_push.send_key',
  template: 'module.server_chan_push.template',
} as const

const { success, error } = useToast()

const saving = ref(false)
const testing = ref(false)
const enabled = ref(false)
const sendKeyIsSet = ref(false)
const sendKeyInput = ref('')
const templateInput = ref('')
const lastTestResult = ref<Array<{ channel: string; success: boolean; message: string }>>([])

const canEnable = computed(() => sendKeyIsSet.value || sendKeyInput.value.trim() !== '')

onMounted(() => {
  loadConfig()
})

async function loadConfig() {
  try {
    const [moduleStatus, sendKey, template] = await Promise.all([
      modulesApi.getStatus('server_chan_push'),
      adminApi.getSystemConfig(CONFIG_KEYS.send_key),
      adminApi.getSystemConfig(CONFIG_KEYS.template),
    ])

    enabled.value = moduleStatus.enabled === true
    sendKeyIsSet.value = sendKey.is_set === true
    sendKeyInput.value = ''
    templateInput.value = typeof template.value === 'string' ? template.value : ''
  } catch (err) {
    error(parseApiError(err, '加载 Server 酱推送配置失败'))
    log.error('加载 Server 酱推送配置失败:', err)
  }
}

async function saveConfig() {
  saving.value = true
  try {
    const updates: Array<Promise<unknown>> = [
      adminApi.updateSystemConfig(CONFIG_KEYS.template, templateInput.value, 'Server 酱推送模板'),
    ]
    const trimmedKey = sendKeyInput.value.trim()
    if (trimmedKey) {
      updates.push(adminApi.updateSystemConfig(CONFIG_KEYS.send_key, trimmedKey, 'Server 酱 SendKey'))
    }
    await Promise.all(updates)
    if (trimmedKey) {
      sendKeyIsSet.value = true
      sendKeyInput.value = ''
    }
    if (!canEnable.value) {
      enabled.value = false
    }
    await modulesApi.setEnabled('server_chan_push', enabled.value)
    success('Server 酱推送配置已保存')
  } catch (err) {
    error(parseApiError(err, '保存 Server 酱推送配置失败'))
    log.error('保存 Server 酱推送配置失败:', err)
  } finally {
    saving.value = false
  }
}

async function handleTest() {
  testing.value = true
  try {
    const result = await adminApi.testImportantNotification({ channel: 'server_chan' })
    lastTestResult.value = result.channels || []
    if (result.success) {
      success(result.message || '测试通知已发送')
    } else {
      error(result.message || '测试通知发送失败')
    }
  } catch (err) {
    error(parseApiError(err, '测试通知发送失败'))
    log.error('测试 Server 酱推送失败:', err)
  } finally {
    testing.value = false
  }
}

function formatChannel(channel: string): string {
  if (channel === 'server_chan') return 'Server 酱'
  if (channel === 'email') return '邮件'
  if (channel === 'module') return '模块'
  if (channel === 'none') return '无可用服务'
  return channel
}
</script>
