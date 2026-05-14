<template>
  <PageContainer>
    <PageHeader
      title="敏感信息保护"
      description="发送给供应商前将聊天消息中的敏感信息替换为占位符，返回客户端前自动还原。"
      :icon="ShieldCheck"
    >
      <template #actions>
        <Button
          variant="outline"
          :disabled="loading || saving"
          @click="loadConfig"
        >
          <RefreshCw
            class="mr-2 h-4 w-4"
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
          <div class="space-y-1">
            <div
              v-if="statusLabel"
              class="flex items-center gap-2"
            >
              <span class="h-2.5 w-2.5 rounded-full bg-primary ring-2 ring-primary/30 ring-offset-2 ring-offset-background" />
              <p class="text-sm font-semibold text-foreground">
                {{ statusLabel }}
              </p>
            </div>
            <p class="max-w-3xl text-sm text-muted-foreground">
              管理员只配置功能是否启用和匹配规则。用户、用户 Key、独立余额 Key 可在各自配置中附加此功能。
            </p>
          </div>
          <div class="flex items-center gap-3 rounded-xl border border-border bg-muted/40 px-4 py-3">
            <div class="text-right">
              <p class="text-sm font-medium text-foreground">
                启用敏感信息保护
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
        title="替换类型配置"
        description="统一配置所有可用规则。"
      >
        <div class="space-y-4">
          <div class="flex items-center justify-between gap-3">
            <div class="text-sm text-muted-foreground">
              规则按表格顺序保存。系统预置规则可直接修改，自定义规则可删除。
            </div>
            <Button
              variant="outline"
              size="sm"
              @click="addCustomRule"
            >
              <Plus class="mr-2 h-4 w-4" />
              新增规则
            </Button>
          </div>

          <div class="overflow-x-auto rounded-xl border border-border">
            <table class="min-w-[920px] w-full text-sm">
              <thead class="bg-muted/50 text-left text-xs font-medium text-muted-foreground">
                <tr>
                  <th class="w-[220px] px-4 py-3">
                    规则名称
                  </th>
                  <th class="px-4 py-3">
                    正则
                  </th>
                  <th class="w-[120px] px-4 py-3">
                    是否启用
                  </th>
                  <th class="w-[150px] px-4 py-3 text-right">
                    操作
                  </th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="(rule, index) in redactionConfig.rules"
                  :key="rule.id"
                  class="border-t border-border align-top"
                >
                  <td class="px-4 py-3">
                    <Input
                      :model-value="rule.name"
                      class="h-9"
                      @update:model-value="(value) => updateRule(index, { name: String(value) })"
                    />
                    <div
                      v-if="rule.system"
                      class="mt-1 text-[11px] text-muted-foreground"
                    >
                      系统预置
                    </div>
                  </td>
                  <td class="px-4 py-3">
                    <Textarea
                      :model-value="rule.pattern"
                      class="min-h-[72px] font-mono text-xs"
                      @update:model-value="(value) => updateRule(index, { pattern: String(value) })"
                    />
                  </td>
                  <td class="px-4 py-3">
                    <Switch
                      :model-value="rule.enabled"
                      @update:model-value="(value: boolean) => updateRule(index, { enabled: value })"
                    />
                  </td>
                  <td class="px-4 py-3">
                    <div class="flex justify-end gap-1">
                      <Button
                        v-if="rule.system"
                        variant="ghost"
                        size="icon"
                        class="h-8 w-8"
                        title="恢复默认"
                        @click="resetSystemRule(index)"
                      >
                        <RotateCcw class="h-4 w-4" />
                      </Button>
                      <Button
                        v-if="!rule.system"
                        variant="ghost"
                        size="icon"
                        class="h-8 w-8 text-destructive"
                        title="删除"
                        @click="removeRule(index)"
                      >
                        <Trash2 class="h-4 w-4" />
                      </Button>
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </div>
      </CardSection>

      <CardSection
        title="占位符配置"
        description="配置供应商侧看到的占位符前缀。"
      >
        <div class="grid grid-cols-1 gap-4 md:grid-cols-[minmax(0,320px)_1fr] md:items-start">
          <div class="space-y-2">
            <Input
              :model-value="redactionConfig.placeholder_prefix"
              class="h-9 font-mono uppercase"
              maxlength="32"
              @update:model-value="(value) => redactionConfig.placeholder_prefix = normalizePlaceholderPrefixInput(String(value))"
            />
            <p class="text-xs text-muted-foreground">
              仅支持字母、数字、下划线，保存后统一转为大写。
            </p>
          </div>
          <div class="rounded-xl border border-border bg-muted/40 px-4 py-3 text-sm">
            <span class="text-muted-foreground">示例：</span>
            <code class="ml-2 rounded bg-background px-2 py-1 font-mono text-xs text-foreground">
              &lt;{{ redactionConfig.placeholder_prefix || 'AETHER' }}:EMAIL:ABCDEFGHIJKLMNOPQRST&gt;
            </code>
          </div>
        </div>
      </CardSection>

      <CardSection
        title="多轮上下文缓存"
        description="此时间控制真实值与占位符映射在 Redis 中的缓存窗口。"
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
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Plus, RefreshCw, RotateCcw, ShieldCheck, Trash2 } from 'lucide-vue-next'
import { PageContainer, PageHeader, CardSection } from '@/components/layout'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Switch from '@/components/ui/switch.vue'
import Textarea from '@/components/ui/textarea.vue'
import {
  CHAT_PII_REDACTION_DEFAULT_RULES,
  modulesApi,
  type ChatPiiRedactionConfig,
  type ChatPiiRedactionRule,
} from '@/api/modules'
import { useModuleStore } from '@/stores/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

const defaultConfig: ChatPiiRedactionConfig = {
  enabled: false,
  rules: CHAT_PII_REDACTION_DEFAULT_RULES.map(rule => ({ ...rule })),
  cache_ttl_seconds: 300,
  placeholder_prefix: 'AETHER',
}

const ttlOptions = [
  {
    value: 300 as const,
    label: '5 分钟（默认）',
    helper: '适合短对话，同一敏感信息在 5 分钟内保持相同占位符。',
  },
  {
    value: 3600 as const,
    label: '1 小时',
    helper: '适合长多轮对话，同一敏感信息在 1 小时内保持相同占位符。',
  },
]

const moduleStore = useModuleStore()
const { success, error } = useToast()

const loading = ref(false)
const saving = ref(false)
const redactionConfig = ref<ChatPiiRedactionConfig>(cloneConfig(defaultConfig))
const originalConfig = ref<ChatPiiRedactionConfig>(cloneConfig(defaultConfig))

const hasChanges = computed(() => JSON.stringify(redactionConfig.value) !== JSON.stringify(originalConfig.value))

const statusLabel = computed(() => {
  const moduleStatus = moduleStore.modules.chat_pii_redaction
  if (moduleStatus && !moduleStatus.config_validated) return '配置异常'
  return redactionConfig.value.enabled ? '已开启' : ''
})

function cloneConfig(config: ChatPiiRedactionConfig): ChatPiiRedactionConfig {
  return {
    enabled: config.enabled,
    rules: config.rules.map(rule => ({ ...rule })),
    cache_ttl_seconds: config.cache_ttl_seconds,
    placeholder_prefix: config.placeholder_prefix || 'AETHER',
  }
}

function normalizePlaceholderPrefixInput(value: string): string {
  return value.toUpperCase().replace(/[^A-Z0-9_]/g, '').slice(0, 32)
}

function updateRule(index: number, patch: Partial<ChatPiiRedactionRule>) {
  const rules = [...redactionConfig.value.rules]
  rules[index] = { ...rules[index], ...patch }
  redactionConfig.value.rules = rules
}

function addCustomRule() {
  redactionConfig.value.rules = [
    ...redactionConfig.value.rules,
    {
      id: `custom_${Date.now().toString(36)}`,
      name: '自定义规则',
      pattern: '',
      enabled: true,
      system: false,
      features: null,
    },
  ]
}

function removeRule(index: number) {
  redactionConfig.value.rules = redactionConfig.value.rules.filter((_, itemIndex) => itemIndex !== index)
}

function resetSystemRule(index: number) {
  const rule = redactionConfig.value.rules[index]
  const defaultRule = CHAT_PII_REDACTION_DEFAULT_RULES.find(item => item.id === rule.id)
  if (!defaultRule) return
  updateRule(index, { ...defaultRule })
}

function sanitizeRules(): ChatPiiRedactionRule[] | null {
  const seen = new Set<string>()
  const rules: ChatPiiRedactionRule[] = []
  for (const [index, rule] of redactionConfig.value.rules.entries()) {
    const id = (rule.id || `custom_${index + 1}`).trim()
    const name = rule.name.trim()
    const pattern = rule.pattern.trim()
    if (!name || !pattern) {
      error('规则名称和正则不能为空')
      return null
    }
    const uniqueId = seen.has(id) ? `${id}_${index + 1}` : id
    seen.add(uniqueId)
    rules.push({
      id: uniqueId,
      name,
      pattern,
      enabled: rule.enabled,
      system: rule.system === true,
      features: rule.features ?? null,
    })
  }
  return rules
}

async function loadConfig() {
  loading.value = true
  try {
    const [config] = await Promise.all([
      modulesApi.getChatPiiRedactionConfig(),
      moduleStore.fetchModules(),
    ])
    redactionConfig.value = cloneConfig(config)
    originalConfig.value = cloneConfig(config)
  } catch (err) {
    error(parseApiError(err, '加载敏感信息保护配置失败'))
    log.error('加载敏感信息保护配置失败:', err)
  } finally {
    loading.value = false
  }
}

async function saveConfig() {
  const rules = sanitizeRules()
  if (!rules) return
  const placeholderPrefix = normalizePlaceholderPrefixInput(redactionConfig.value.placeholder_prefix).trim()
  if (!placeholderPrefix) {
    error('占位符前缀不能为空')
    return
  }
  saving.value = true
  try {
    const saved = await modulesApi.updateChatPiiRedactionConfig({
      ...redactionConfig.value,
      placeholder_prefix: placeholderPrefix,
      rules,
    })
    redactionConfig.value = cloneConfig(saved)
    originalConfig.value = cloneConfig(saved)
    await moduleStore.fetchModules()
    success('敏感信息保护配置已保存')
  } catch (err) {
    error(parseApiError(err, '保存敏感信息保护配置失败'))
    log.error('保存敏感信息保护配置失败:', err)
  } finally {
    saving.value = false
  }
}

onMounted(loadConfig)
</script>
