<template>
  <PageContainer>
    <PageHeader
      title="邮件配置"
      description="配置邮件发送服务和注册邮箱限制"
    />

    <div class="mt-6 space-y-6">
      <!-- SMTP 邮件配置 -->
      <CardSection
        title="SMTP 邮件配置"
        description="配置 SMTP 服务用于发送验证码邮件"
      >
        <template #actions>
          <div class="flex gap-2">
            <Button
              size="sm"
              variant="outline"
              :disabled="testSmtpLoading"
              @click="handleTestSmtp"
            >
              {{ testSmtpLoading ? '测试中...' : '测试连接' }}
            </Button>
            <Button
              size="sm"
              :disabled="smtpSaveLoading"
              @click="saveSmtpConfig"
            >
              {{ smtpSaveLoading ? '保存中...' : '保存' }}
            </Button>
          </div>
        </template>
        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
          <div>
            <Label
              for="smtp-host"
              class="block text-sm font-medium"
            >
              SMTP 服务器地址
            </Label>
            <Input
              id="smtp-host"
              v-model="emailConfig.smtp_host"
              type="text"
              placeholder="smtp.gmail.com"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              邮件服务器地址
            </p>
          </div>

          <div>
            <Label
              for="smtp-port"
              class="block text-sm font-medium"
            >
              SMTP 端口
            </Label>
            <Input
              id="smtp-port"
              v-model.number="emailConfig.smtp_port"
              type="number"
              placeholder="587"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              常用端口: 587 (TLS), 465 (SSL), 25 (无加密)
            </p>
          </div>

          <div>
            <Label
              for="smtp-user"
              class="block text-sm font-medium"
            >
              SMTP 用户名
            </Label>
            <Input
              id="smtp-user"
              v-model="emailConfig.smtp_user"
              type="text"
              placeholder="your-email@example.com"
              class="mt-1"
              autocomplete="off"
              data-lpignore="true"
              data-1p-ignore="true"
              data-form-type="other"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              通常是您的邮箱地址
            </p>
          </div>

          <div>
            <Label
              for="smtp-password"
              class="block text-sm font-medium"
            >
              SMTP 密码
            </Label>
            <div class="mt-1">
              <Input
                id="smtp-password"
                v-model="emailConfig.smtp_password"
                masked
                :placeholder="smtpPasswordIsSet ? '已设置（留空保持不变）' : '请输入密码'"
              />
            </div>
            <p class="mt-1 text-xs text-muted-foreground">
              邮箱密码或应用专用密码
            </p>
          </div>

          <div>
            <Label
              for="smtp-from-email"
              class="block text-sm font-medium"
            >
              发件人邮箱
            </Label>
            <Input
              id="smtp-from-email"
              v-model="emailConfig.smtp_from_email"
              type="email"
              placeholder="noreply@example.com"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              显示为发件人的邮箱地址
            </p>
          </div>

          <div>
            <Label
              for="smtp-from-name"
              class="block text-sm font-medium"
            >
              发件人名称
            </Label>
            <Input
              id="smtp-from-name"
              v-model="emailConfig.smtp_from_name"
              type="text"
              placeholder="Aether"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              显示为发件人的名称
            </p>
          </div>

          <div>
            <Label
              for="smtp-encryption"
              class="block text-sm font-medium mb-2"
            >
              加密方式
            </Label>
            <Select
              v-model="smtpEncryption"
            >
              <SelectTrigger
                id="smtp-encryption"
                class="mt-1"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="ssl">
                  SSL (隐式加密)
                </SelectItem>
                <SelectItem value="tls">
                  TLS / STARTTLS
                </SelectItem>
                <SelectItem value="none">
                  无加密
                </SelectItem>
              </SelectContent>
            </Select>
            <p class="mt-1 text-xs text-muted-foreground">
              Gmail 等服务推荐使用 SSL
            </p>
          </div>
        </div>
      </CardSection>

      <!-- 注册邮箱验证 -->
      <CardSection
        title="注册邮箱验证"
        description="控制用户注册时的邮箱验证要求和后缀限制"
      >
        <template #actions>
          <Button
            size="sm"
            :disabled="emailVerificationSaveLoading"
            @click="saveEmailVerificationConfig"
          >
            {{ emailVerificationSaveLoading ? '保存中...' : '保存' }}
          </Button>
        </template>
        <div class="space-y-6">
          <!-- 第一行：需要邮箱验证 + 后缀限制模式 -->
          <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
            <!-- 需要邮箱验证 -->
            <div class="flex items-center justify-between h-full">
              <div>
                <Label
                  for="require-email-verification"
                  class="block text-sm font-medium cursor-pointer"
                  :class="{ 'text-muted-foreground': !smtpConfigured }"
                >
                  需要邮箱验证
                </Label>
                <p class="mt-1 text-xs text-muted-foreground">
                  <template v-if="!smtpConfigured">
                    需先配置 SMTP 服务
                  </template>
                  <template v-else>
                    开启后，用户注册时必须验证邮箱
                  </template>
                </p>
              </div>
              <Switch
                id="require-email-verification"
                v-model="requireEmailVerification"
                :disabled="!smtpConfigured"
              />
            </div>

            <!-- 后缀限制模式 -->
            <div>
              <Label
                for="email-suffix-mode"
                class="block text-sm font-medium mb-2"
              >
                后缀限制模式
              </Label>
              <Select
                v-model="emailConfig.email_suffix_mode"
                :disabled="!requireEmailVerification"
              >
                <SelectTrigger
                  id="email-suffix-mode"
                  class="mt-1"
                  :disabled="!requireEmailVerification"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">
                    不限制 - 允许所有邮箱
                  </SelectItem>
                  <SelectItem value="whitelist">
                    白名单 - 仅允许列出的后缀
                  </SelectItem>
                  <SelectItem value="blacklist">
                    黑名单 - 拒绝列出的后缀
                  </SelectItem>
                </SelectContent>
              </Select>
              <p class="mt-1 text-xs text-muted-foreground">
                <template v-if="emailConfig.email_suffix_mode === 'none'">
                  不限制邮箱后缀，所有邮箱均可注册
                </template>
                <template v-else-if="emailConfig.email_suffix_mode === 'whitelist'">
                  仅允许列出后缀的邮箱注册
                </template>
                <template v-else>
                  拒绝列出后缀的邮箱注册
                </template>
              </p>
            </div>
          </div>

          <!-- 第二行：邮箱后缀列表 -->
          <div v-if="emailConfig.email_suffix_mode !== 'none'">
            <Label
              for="email-suffix-list"
              class="block text-sm font-medium mb-2"
            >
              邮箱后缀列表
            </Label>
            <Input
              id="email-suffix-list"
              v-model="emailSuffixListStr"
              placeholder="gmail.com, outlook.com, qq.com"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              逗号分隔，例如: gmail.com, outlook.com, qq.com
            </p>
          </div>
        </div>
      </CardSection>

      <!-- 邮件模板配置 -->
      <CardSection
        title="邮件模板"
        description="配置不同类型邮件的 HTML 模板"
      >
        <template #actions>
          <Button
            size="sm"
            :disabled="templateSaveLoading"
            @click="handleSaveTemplate"
          >
            {{ templateSaveLoading ? '保存中...' : '保存' }}
          </Button>
        </template>

        <!-- 当前模板编辑区 -->
        <div
          v-if="currentTemplate"
          class="space-y-4"
        >
          <!-- 模板类型选择 + 可用变量 -->
          <div class="flex items-center justify-between gap-4 flex-wrap">
            <div class="flex items-center border-b border-border">
              <button
                v-for="tpl in templateTypes"
                :key="tpl.type"
                class="px-4 py-2 text-sm font-medium transition-colors relative"
                :class="activeTemplateType === tpl.type
                  ? 'text-foreground'
                  : 'text-muted-foreground hover:text-foreground'"
                @click="handleTemplateTypeChange(tpl.type)"
              >
                {{ tpl.name }}
                <span
                  v-if="tpl.is_custom"
                  class="ml-1 text-xs opacity-70"
                >(已自定义)</span>
                <span
                  v-if="activeTemplateType === tpl.type"
                  class="absolute bottom-0 left-0 right-0 h-0.5 bg-primary"
                />
              </button>
            </div>
            <div class="text-xs text-muted-foreground">
              可用变量:
              <code
                v-for="(v, i) in currentTemplate.variables"
                :key="v"
                class="mx-0.5 px-1.5 py-0.5 bg-muted rounded text-foreground"
              >{{ formatVariable(v) }}<span v-if="i < currentTemplate.variables.length - 1">,</span></code>
            </div>
          </div>

          <!-- 邮件主题 -->
          <div>
            <Label
              for="template-subject"
              class="block text-sm font-medium"
            >
              邮件主题
            </Label>
            <Input
              id="template-subject"
              v-model="templateSubject"
              type="text"
              :placeholder="currentTemplate.default_subject || '验证码'"
              class="mt-1"
            />
          </div>

          <!-- HTML 模板编辑 -->
          <div>
            <Label
              for="template-html"
              class="block text-sm font-medium"
            >
              HTML 模板
            </Label>
            <textarea
              id="template-html"
              v-model="templateHtml"
              rows="12"
              class="mt-1 w-full font-mono text-sm bg-muted/30 border border-border rounded-md p-3 focus:outline-none focus:ring-2 focus:ring-primary focus:border-transparent resize-y"
              :placeholder="currentTemplate.default_html || '<!DOCTYPE html>...'"
              spellcheck="false"
            />
          </div>

          <!-- 操作按钮 -->
          <div class="flex gap-2">
            <Button
              variant="outline"
              size="sm"
              :disabled="previewLoading"
              @click="handlePreviewTemplate"
            >
              {{ previewLoading ? '加载中...' : '预览' }}
            </Button>
            <Button
              variant="outline"
              size="sm"
              :disabled="!currentTemplate.is_custom"
              @click="handleResetTemplate"
            >
              重置为默认
            </Button>
          </div>
        </div>

        <!-- 加载中状态 -->
        <div
          v-else-if="templateLoading"
          class="py-8 text-center text-muted-foreground"
        >
          正在加载模板...
        </div>
      </CardSection>

      <!-- 预览对话框 -->
      <Dialog
        v-model:open="previewDialogOpen"
        no-padding
        max-width="xl"
      >
        <!-- 自定义窗口布局 -->
        <div class="flex flex-col max-h-[80vh]">
          <!-- 窗口标题栏 -->
          <div class="flex items-center justify-between px-4 py-2.5 bg-muted/50 border-b border-border/50 flex-shrink-0">
            <div class="flex items-center gap-3">
              <button
                type="button"
                class="flex gap-1.5 group"
                title="关闭"
                @click="previewDialogOpen = false"
              >
                <div class="w-2.5 h-2.5 rounded-full bg-red-400/80 group-hover:bg-red-500" />
                <div class="w-2.5 h-2.5 rounded-full bg-yellow-400/80" />
                <div class="w-2.5 h-2.5 rounded-full bg-green-400/80" />
              </button>
              <span class="text-sm font-medium text-foreground/80">邮件预览</span>
            </div>
            <div class="text-xs text-muted-foreground font-mono">
              {{ currentTemplate?.name || '模板' }}
            </div>
          </div>

          <!-- 邮件头部信息 -->
          <div class="px-4 py-3 bg-muted/30 border-b border-border/30 space-y-1.5 flex-shrink-0">
            <div class="flex items-center gap-2 text-sm">
              <span class="text-muted-foreground w-14">主题:</span>
              <span class="font-medium text-foreground">{{ templateSubject || '(无主题)' }}</span>
            </div>
            <div class="flex items-center gap-2 text-sm">
              <span class="text-muted-foreground w-14">收件人:</span>
              <span class="text-foreground/80">example@example.com</span>
            </div>
          </div>

          <!-- 邮件内容区域 - 直接显示邮件模板 -->
          <div class="flex-1 overflow-auto">
            <iframe
              v-if="previewHtml"
              ref="previewIframe"
              :srcdoc="previewHtml"
              class="w-full border-0"
              style="min-height: 400px;"
              sandbox="allow-same-origin"
              @load="adjustIframeHeight"
            />
          </div>
        </div>
      </Dialog>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Switch from '@/components/ui/switch.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import Dialog from '@/components/ui/dialog/Dialog.vue'
import { PageHeader, PageContainer, CardSection } from '@/components/layout'
import { useToast } from '@/composables/useToast'
import { adminApi, type EmailTemplateInfo } from '@/api/admin'
import { authApi } from '@/api/auth'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

const { success, error } = useToast()

interface EmailConfig {
  // SMTP 邮件配置
  smtp_host: string | null
  smtp_port: number
  smtp_user: string | null
  smtp_password: string | null
  smtp_use_tls: boolean
  smtp_use_ssl: boolean
  smtp_from_email: string | null
  smtp_from_name: string
  // 注册邮箱限制
  email_suffix_mode: 'none' | 'whitelist' | 'blacklist'
  email_suffix_list: string[]
}

const smtpSaveLoading = ref(false)
const emailVerificationSaveLoading = ref(false)
const testSmtpLoading = ref(false)
const smtpPasswordIsSet = ref(false)
const requireEmailVerification = ref(false) // 是否开启了邮箱验证
const smtpConfigured = ref(false) // SMTP 是否已配置

// 邮件模板相关状态
const templateLoading = ref(false)
const templateSaveLoading = ref(false)
const previewLoading = ref(false)
const previewDialogOpen = ref(false)
const previewHtml = ref('')
const templateTypes = ref<EmailTemplateInfo[]>([])
const activeTemplateType = ref('verification')
const templateSubject = ref('')
const templateHtml = ref('')
const previewIframe = ref<HTMLIFrameElement | null>(null)

// 当前选中的模板
const currentTemplate = computed(() => {
  return templateTypes.value.find(t => t.type === activeTemplateType.value)
})

// 格式化变量显示（避免 Vue 模板中的双花括号语法冲突）
function formatVariable(name: string): string {
  return `{{${name}}}`
}

// 调整 iframe 高度以适应内容
function adjustIframeHeight() {
  if (previewIframe.value) {
    try {
      const doc = previewIframe.value.contentDocument || previewIframe.value.contentWindow?.document
      if (doc && doc.body) {
        // 获取内容实际高度，添加一点余量
        const height = doc.body.scrollHeight + 20
        // 限制最大高度为视口的 70%
        const maxHeight = window.innerHeight * 0.7
        previewIframe.value.style.height = `${Math.min(height, maxHeight)}px`
      }
    } catch {
      // 跨域限制时使用默认高度
      previewIframe.value.style.height = '500px'
    }
  }
}

const emailConfig = ref<EmailConfig>({
  // SMTP 邮件配置
  smtp_host: null,
  smtp_port: 587,
  smtp_user: null,
  smtp_password: null,
  smtp_use_tls: true,
  smtp_use_ssl: false,
  smtp_from_email: null,
  smtp_from_name: 'Aether',
  // 注册邮箱限制
  email_suffix_mode: 'none',
  email_suffix_list: [],
})

// 计算属性：邮箱后缀列表数组和字符串之间的转换
const emailSuffixListStr = computed({
  get: () => emailConfig.value.email_suffix_list.join(', '),
  set: (val: string) => {
    emailConfig.value.email_suffix_list = val
      .split(',')
      .map(s => s.trim().toLowerCase())
      .filter(s => s.length > 0)
  }
})

// 计算属性：SMTP 加密方式（ssl/tls/none）
const smtpEncryption = computed({
  get: () => {
    if (emailConfig.value.smtp_use_ssl) return 'ssl'
    if (emailConfig.value.smtp_use_tls) return 'tls'
    return 'none'
  },
  set: (val: string) => {
    emailConfig.value.smtp_use_ssl = val === 'ssl'
    emailConfig.value.smtp_use_tls = val === 'tls'
  }
})

onMounted(async () => {
  await Promise.all([
    loadEmailConfig(),
    loadEmailTemplates(),
    loadRequireEmailVerification(),
  ])
})

async function loadRequireEmailVerification() {
  try {
    const settings = await authApi.getRegistrationSettings()
    requireEmailVerification.value = !!settings.require_email_verification
    smtpConfigured.value = !!settings.email_configured
  } catch {
    requireEmailVerification.value = false
    smtpConfigured.value = false
  }
}

async function saveEmailVerificationConfig() {
  emailVerificationSaveLoading.value = true
  try {
    const configItems = [
      {
        key: 'require_email_verification',
        value: requireEmailVerification.value,
        description: '是否需要邮箱验证'
      },
      {
        key: 'email_suffix_mode',
        value: emailConfig.value.email_suffix_mode,
        description: '邮箱后缀限制模式'
      },
      {
        key: 'email_suffix_list',
        value: emailConfig.value.email_suffix_list,
        description: '邮箱后缀列表'
      },
    ]

    await Promise.all(
      configItems.map(item =>
        adminApi.updateSystemConfig(item.key, item.value, item.description)
      )
    )
    success('配置已保存')
  } catch (err) {
    error('保存配置失败')
    log.error('保存邮箱验证配置失败:', err)
  } finally {
    emailVerificationSaveLoading.value = false
  }
}

async function loadEmailTemplates() {
  templateLoading.value = true
  try {
    const response = await adminApi.getEmailTemplates()
    templateTypes.value = response.templates

    // 设置第一个模板为当前模板
    if (response.templates.length > 0) {
      const firstTemplate = response.templates[0]
      activeTemplateType.value = firstTemplate.type
      templateSubject.value = firstTemplate.subject
      templateHtml.value = firstTemplate.html
    }
  } catch (err) {
    error('加载邮件模板失败')
    log.error('加载邮件模板失败:', err)
  } finally {
    templateLoading.value = false
  }
}

function handleTemplateTypeChange(type: string) {
  activeTemplateType.value = type
  const template = templateTypes.value.find(t => t.type === type)
  if (template) {
    templateSubject.value = template.subject
    templateHtml.value = template.html
  }
}

async function handleSaveTemplate() {
  templateSaveLoading.value = true
  try {
    await adminApi.updateEmailTemplate(activeTemplateType.value, {
      subject: templateSubject.value,
      html: templateHtml.value
    })

    // 更新本地状态
    const idx = templateTypes.value.findIndex(t => t.type === activeTemplateType.value)
    if (idx !== -1) {
      templateTypes.value[idx].subject = templateSubject.value
      templateTypes.value[idx].html = templateHtml.value
      templateTypes.value[idx].is_custom = true
    }

    success('模板保存成功')
  } catch (err) {
    error('保存模板失败')
    log.error('保存模板失败:', err)
  } finally {
    templateSaveLoading.value = false
  }
}

async function handlePreviewTemplate() {
  previewLoading.value = true
  try {
    const response = await adminApi.previewEmailTemplate(activeTemplateType.value, {
      html: templateHtml.value
    })
    previewHtml.value = response.html
    previewDialogOpen.value = true
  } catch (err) {
    error('预览模板失败')
    log.error('预览模板失败:', err)
  } finally {
    previewLoading.value = false
  }
}

async function handleResetTemplate() {
  try {
    const response = await adminApi.resetEmailTemplate(activeTemplateType.value)

    // 更新本地状态
    const idx = templateTypes.value.findIndex(t => t.type === activeTemplateType.value)
    if (idx !== -1) {
      templateTypes.value[idx].subject = response.template.subject
      templateTypes.value[idx].html = response.template.html
      templateTypes.value[idx].is_custom = false
    }

    templateSubject.value = response.template.subject
    templateHtml.value = response.template.html

    success('模板已重置为默认值')
  } catch (err) {
    error('重置模板失败')
    log.error('重置模板失败:', err)
  }
}

async function loadEmailConfig() {
  try {
    const configKeys = [
      // SMTP 邮件配置
      'smtp_host',
      'smtp_port',
      'smtp_user',
      'smtp_password',
      'smtp_use_tls',
      'smtp_use_ssl',
      'smtp_from_email',
      'smtp_from_name',
      // 注册邮箱限制
      'email_suffix_mode',
      'email_suffix_list',
    ]
    const configs = await adminApi.getAllSystemConfigs({ cacheTtlMs: 30_000 })
    const configsByKey = new Map(configs.map(config => [config.key, config]))

    for (const key of configKeys) {
      const response = configsByKey.get(key)
      if (!response) continue
      // 特殊处理敏感字段：只记录是否已设置，不填充值
      if (key === 'smtp_password') {
        smtpPasswordIsSet.value = response.is_set === true
        // 不设置 smtp_password 的值，保持为 null
      } else if (response.value !== null && response.value !== undefined) {
        (emailConfig.value as Record<string, unknown>)[key] = response.value
      }
    }
  } catch (err) {
    error('加载邮件配置失败')
    log.error('加载邮件配置失败:', err)
  }
}

// 保存 SMTP 配置
async function saveSmtpConfig() {
  smtpSaveLoading.value = true
  try {
    const configItems = [
      {
        key: 'smtp_host',
        value: emailConfig.value.smtp_host,
        description: 'SMTP 服务器地址'
      },
      {
        key: 'smtp_port',
        value: emailConfig.value.smtp_port,
        description: 'SMTP 端口'
      },
      {
        key: 'smtp_user',
        value: emailConfig.value.smtp_user,
        description: 'SMTP 用户名'
      },
      // 只有输入了新密码才提交（空值表示保持原密码）
      ...(emailConfig.value.smtp_password
        ? [{
            key: 'smtp_password',
            value: emailConfig.value.smtp_password,
            description: 'SMTP 密码'
          }]
        : []),
      {
        key: 'smtp_use_tls',
        value: emailConfig.value.smtp_use_tls,
        description: '是否使用 TLS 加密'
      },
      {
        key: 'smtp_use_ssl',
        value: emailConfig.value.smtp_use_ssl,
        description: '是否使用 SSL 加密'
      },
      {
        key: 'smtp_from_email',
        value: emailConfig.value.smtp_from_email,
        description: '发件人邮箱'
      },
      {
        key: 'smtp_from_name',
        value: emailConfig.value.smtp_from_name,
        description: '发件人名称'
      },
    ]

    await Promise.all(
      configItems.map(item =>
        adminApi.updateSystemConfig(item.key, item.value, item.description)
      )
    )
    success('SMTP 配置已保存')

    // 更新状态
    if (emailConfig.value.smtp_password) {
      smtpPasswordIsSet.value = true
    }
    emailConfig.value.smtp_password = null
  } catch (err) {
    error('保存配置失败')
    log.error('保存 SMTP 配置失败:', err)
  } finally {
    smtpSaveLoading.value = false
  }
}

// 测试 SMTP 连接
async function handleTestSmtp() {
  testSmtpLoading.value = true

  try {
    // 如果没有输入新密码，不发送（后端会使用数据库中的密码）
    const result = await adminApi.testSmtpConnection({
      smtp_host: emailConfig.value.smtp_host,
      smtp_port: emailConfig.value.smtp_port,
      smtp_user: emailConfig.value.smtp_user,
      smtp_password: emailConfig.value.smtp_password || undefined,
      smtp_use_tls: emailConfig.value.smtp_use_tls,
      smtp_use_ssl: emailConfig.value.smtp_use_ssl,
      smtp_from_email: emailConfig.value.smtp_from_email,
      smtp_from_name: emailConfig.value.smtp_from_name
    })
    if (result.success) {
      success('SMTP 连接测试成功')
    } else {
      error(result.message || '未知错误', 'SMTP 连接测试失败')
    }
  } catch (err: unknown) {
    log.error('SMTP 连接测试失败:', err)
    error(parseApiError(err, '未知错误'), 'SMTP 连接测试失败')
  } finally {
    testSmtpLoading.value = false
  }
}
</script>
