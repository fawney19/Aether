<template>
  <PageContainer>
    <PageHeader
      title="敏感信息替换保护"
      description="对聊天消息中的手机号、邮箱、证件号、银行卡号、IP、API Key 与令牌进行可逆占位符替换，防止原文发送给上游供应商。"
      :icon="ShieldCheck"
    >
      <template #actions>
        <Button
          variant="outline"
          :disabled="loading || saving"
          @click="loadConfig"
        >
          <RefreshCw
            class="w-4 h-4 mr-2"
            :class="{ 'animate-spin': loading }"
          />
          刷新
        </Button>
        <Button
          :disabled="loading || saving || !hasChanges"
          @click="saveConfig"
        >
          {{ saving ? '保存中...' : '保存配置' }}
        </Button>
      </template>
    </PageHeader>

    <div class="mt-6 space-y-6">
      <section class="rounded-2xl border border-border bg-card p-5">
        <div class="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div class="space-y-2">
            <div class="flex items-center gap-2">
              <span
                class="h-2.5 w-2.5 rounded-full ring-2 ring-offset-2 ring-offset-background"
                :class="redactionConfig.enabled ? 'bg-primary ring-primary/30' : 'bg-muted ring-muted/60'"
              />
              <p class="text-sm font-semibold text-foreground">
                {{ statusLabel }}
              </p>
            </div>
            <p class="max-w-3xl text-sm text-muted-foreground">
              发送给供应商前将聊天消息中的敏感信息替换为占位符，返回客户端前自动还原。
            </p>
          </div>
          <div class="flex items-center gap-3 rounded-xl border border-border bg-muted/40 px-4 py-3">
            <div class="text-right">
              <p class="text-sm font-medium text-foreground">
                开启敏感信息替换保护
              </p>
              <p class="text-xs text-muted-foreground">
                关闭后所有供应商均不会执行替换
              </p>
            </div>
            <Switch
              :model-value="redactionConfig.enabled"
              @update:model-value="(value: boolean) => redactionConfig.enabled = value"
            />
          </div>
        </div>
      </section>

      <CardSection
        title="启用范围"
        description="关闭后，所有供应商都不会执行替换；开启后，按下方“启用范围”决定是全部供应商生效还是指定供应商生效。"
      >
        <div class="grid grid-cols-1 gap-3 md:grid-cols-2">
          <button
            v-for="option in scopeOptions"
            :key="option.value"
            type="button"
            class="rounded-xl border p-4 text-left transition-all duration-200"
            :class="redactionConfig.provider_scope === option.value
              ? 'border-primary bg-primary/10 text-primary shadow-sm'
              : 'border-border bg-card/70 text-muted-foreground hover:border-primary/50 hover:text-foreground'"
            @click="redactionConfig.provider_scope = option.value"
          >
            <div class="flex items-center justify-between gap-3">
              <span class="text-sm font-semibold">{{ option.label }}</span>
              <span
                class="h-2 w-2 rounded-full"
                :class="redactionConfig.provider_scope === option.value ? 'bg-primary' : 'bg-muted'"
              />
            </div>
            <p class="mt-2 text-xs leading-relaxed text-muted-foreground">
              {{ option.helper }}
            </p>
          </button>
        </div>
      </CardSection>

      <CardSection
        title="替换类型配置"
        description="适用于所有已开启该功能的供应商。"
      >
        <div class="space-y-5">
          <div
            v-for="group in entityGroups"
            :key="group.title"
            class="rounded-xl border border-border bg-muted/30 p-4"
          >
            <div class="flex items-center justify-between gap-3">
              <div>
                <h3 class="text-sm font-semibold text-foreground">
                  {{ group.title }}
                </h3>
                <p class="mt-1 text-xs text-muted-foreground">
                  已选择 {{ selectedCount(group.entities) }} / {{ group.entities.length }} 项
                </p>
              </div>
              <Button
                variant="outline"
                size="sm"
                @click="setGroupSelection(group.entities, selectedCount(group.entities) !== group.entities.length)"
              >
                {{ selectedCount(group.entities) === group.entities.length ? '清除本组' : '选择本组' }}
              </Button>
            </div>
            <div class="mt-4 grid grid-cols-1 gap-3 md:grid-cols-2">
              <label
                v-for="entity in group.entities"
                :key="entity.key"
                class="flex items-start gap-3 rounded-lg border border-border bg-card/70 px-3 py-3 text-sm transition-colors hover:border-primary/40"
              >
                <Checkbox
                  :checked="redactionConfig.entities.includes(entity.key)"
                  @update:checked="(checked: boolean) => toggleEntity(entity.key, checked)"
                />
                <span class="leading-tight">
                  <span class="block font-medium text-foreground">{{ entity.label }}</span>
                  <span class="mt-1 block text-xs text-muted-foreground">{{ entity.description }}</span>
                </span>
              </label>
            </div>
          </div>
          <div class="rounded-xl border border-border bg-card px-4 py-3 text-sm text-muted-foreground">
            真实姓名、地址、公司名暂不支持自动识别，避免误判影响模型理解。
          </div>
        </div>
      </CardSection>

      <CardSection
        title="多轮上下文缓存"
        description="此时间控制“真实值 ↔ 占位符”映射在 Redis 中的缓存窗口。窗口内相同敏感值使用相同占位符，以减少上游 prompt cache 失效；窗口过期后新请求会生成新的占位符。缓存写入 Redis，必须设置 TTL，不写入数据库或日志。"
      >
        <div class="grid grid-cols-1 gap-3 md:grid-cols-2">
          <button
            v-for="option in ttlOptions"
            :key="option.value"
            type="button"
            class="rounded-xl border p-4 text-left transition-all duration-200"
            :class="redactionConfig.cache_ttl_seconds === option.value
              ? 'border-primary bg-primary/10 text-primary shadow-sm'
              : 'border-border bg-card/70 text-muted-foreground hover:border-primary/50 hover:text-foreground'"
            @click="redactionConfig.cache_ttl_seconds = option.value"
          >
            <span class="text-sm font-semibold">{{ option.label }}</span>
            <p class="mt-2 text-xs leading-relaxed text-muted-foreground">
              {{ option.helper }}
            </p>
          </button>
        </div>
      </CardSection>

      <CardSection title="模型提示说明">
        <div class="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div class="space-y-1">
            <p class="text-sm font-medium text-foreground">
              向模型说明占位符含义
            </p>
            <p class="max-w-3xl text-xs leading-relaxed text-muted-foreground">
              开启后，Aether 会在发往供应商的请求中插入一条简短内部说明，说明 `&lt;AETHER:TYPE:ID&gt;` 是已保护的真实信息占位符，应按对应类型正常理解和处理，不要要求用户重新提供原文。
            </p>
          </div>
          <Switch
            :model-value="redactionConfig.inject_model_instruction"
            @update:model-value="(value: boolean) => redactionConfig.inject_model_instruction = value"
          />
        </div>
      </CardSection>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { RefreshCw, ShieldCheck } from 'lucide-vue-next'
import { PageContainer, PageHeader, CardSection } from '@/components/layout'
import Button from '@/components/ui/button.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Switch from '@/components/ui/switch.vue'
import { modulesApi, type ChatPiiRedactionConfig, type ChatPiiRedactionEntity, type ChatPiiRedactionProviderScope } from '@/api/modules'
import { useModuleStore } from '@/stores/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

interface EntityOption {
  key: ChatPiiRedactionEntity
  label: string
  description: string
}

const defaultConfig: ChatPiiRedactionConfig = {
  enabled: false,
  provider_scope: 'selected_providers',
  entities: [
    'email',
    'cn_phone',
    'global_phone',
    'cn_id',
    'payment_card',
    'ipv4',
    'ipv6',
    'api_key',
    'access_token',
    'secret_key',
    'bearer_token',
    'jwt',
  ],
  cache_ttl_seconds: 300,
  inject_model_instruction: true,
}

const scopeOptions: Array<{ value: ChatPiiRedactionProviderScope; label: string; helper: string }> = [
  {
    value: 'all_providers',
    label: '全部供应商',
    helper: '所有供应商都会执行替换保护，无需逐个开启。',
  },
  {
    value: 'selected_providers',
    label: '指定供应商',
    helper: '仅对在供应商管理中开启的供应商生效。',
  },
]

const ttlOptions = [
  {
    value: 300 as const,
    label: '5 分钟（默认）',
    helper: '更保守，适合短对话或较高隐私偏好；同一敏感信息在 5 分钟内保持相同占位符。',
  },
  {
    value: 3600 as const,
    label: '1 小时',
    helper: '更适合长多轮对话，可减少重复上下文检测成本；同一敏感信息在 1 小时内保持相同占位符。',
  },
]

const entityGroups: Array<{ title: string; entities: EntityOption[] }> = [
  {
    title: '个人信息',
    entities: [
      { key: 'email', label: '邮箱', description: '识别常见电子邮箱地址。' },
      { key: 'cn_phone', label: '手机号/固话', description: '识别中国大陆手机号和固定电话。' },
      { key: 'global_phone', label: '全球电话号码', description: '识别 E.164 风格国际号码。' },
      { key: 'cn_id', label: '中国大陆身份证号', description: '识别通过校验的居民身份证号。' },
      { key: 'payment_card', label: '银行卡号', description: '识别通过 Luhn 校验的卡号。' },
      { key: 'ipv4', label: 'IPv4', description: '识别 IPv4 地址。' },
      { key: 'ipv6', label: 'IPv6', description: '识别 IPv6 地址。' },
    ],
  },
  {
    title: '密钥与令牌',
    entities: [
      { key: 'api_key', label: 'API Key', description: '识别 OpenAI、Anthropic、GitHub、Slack、AWS 等常见密钥。' },
      { key: 'access_token', label: 'Access Token', description: '识别访问令牌形态的敏感凭证。' },
      { key: 'secret_key', label: 'Secret Key', description: '识别高熵密钥和 secret 字段值。' },
      { key: 'bearer_token', label: 'Bearer Token', description: '识别 Authorization Bearer 凭证。' },
      { key: 'jwt', label: 'JWT', description: '识别三段式 JWT 令牌。' },
    ],
  },
]

const moduleStore = useModuleStore()
const { success, error } = useToast()

const loading = ref(false)
const saving = ref(false)
const redactionConfig = ref<ChatPiiRedactionConfig>({ ...defaultConfig })
const originalConfig = ref<ChatPiiRedactionConfig>({ ...defaultConfig })

const hasChanges = computed(() => JSON.stringify(redactionConfig.value) !== JSON.stringify(originalConfig.value))

const statusLabel = computed(() => {
  const moduleStatus = moduleStore.modules.chat_pii_redaction
  if (moduleStatus && !moduleStatus.config_validated) return '配置异常，替换保护未生效'
  if (!redactionConfig.value.enabled) return '全局替换开关未开启，所有供应商均不会执行替换保护'
  return redactionConfig.value.provider_scope === 'all_providers'
    ? '全局替换开关已开启，全部供应商都会执行替换保护'
    : '全局替换开关已开启，可在供应商中单独启用'
})

async function loadConfig() {
  loading.value = true
  try {
    const [config] = await Promise.all([
      modulesApi.getChatPiiRedactionConfig(),
      moduleStore.fetchModules(),
    ])
    redactionConfig.value = { ...config }
    originalConfig.value = { ...config }
  } catch (err) {
    error(parseApiError(err, '加载敏感信息替换保护配置失败'))
    log.error('加载敏感信息替换保护配置失败:', err)
  } finally {
    loading.value = false
  }
}

async function saveConfig() {
  saving.value = true
  try {
    const saved = await modulesApi.updateChatPiiRedactionConfig(redactionConfig.value)
    redactionConfig.value = { ...saved }
    originalConfig.value = { ...saved }
    await moduleStore.fetchModules()
    success('敏感信息替换保护配置已保存')
  } catch (err) {
    error(parseApiError(err, '保存敏感信息替换保护配置失败'))
    log.error('保存敏感信息替换保护配置失败:', err)
  } finally {
    saving.value = false
  }
}

function selectedCount(entities: EntityOption[]) {
  return entities.filter((entity) => redactionConfig.value.entities.includes(entity.key)).length
}

function toggleEntity(entity: ChatPiiRedactionEntity, checked: boolean) {
  const current = new Set(redactionConfig.value.entities)
  if (checked) {
    current.add(entity)
  } else {
    current.delete(entity)
  }
  redactionConfig.value.entities = defaultConfig.entities.filter((item) => current.has(item))
}

function setGroupSelection(entities: EntityOption[], checked: boolean) {
  const current = new Set(redactionConfig.value.entities)
  for (const entity of entities) {
    if (checked) {
      current.add(entity.key)
    } else {
      current.delete(entity.key)
    }
  }
  redactionConfig.value.entities = defaultConfig.entities.filter((item) => current.has(item))
}

onMounted(loadConfig)
</script>
