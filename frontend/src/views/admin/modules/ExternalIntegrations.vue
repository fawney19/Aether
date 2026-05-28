<template>
  <PageContainer>
    <PageHeader
      title="外部系统集成"
      description="把工单、状态页、知识库等外部系统作为受控入口嵌入 Aether。"
      :icon="ExternalLink"
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
            <Save class="mr-2 h-4 w-4" />
            {{ saving ? '保存中...' : '保存配置' }}
          </Button>
      </template>
    </PageHeader>

    <div class="mt-6 space-y-6">
      <CardSection
        title="模块开关"
        description="启用后，符合可见范围的入口会出现在用户或管理员侧导航中。"
      >
        <div class="flex flex-col gap-4 md:flex-row md:items-center md:justify-between">
          <div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
            <div class="rounded-xl border border-border bg-muted/30 px-4 py-3">
              <p class="text-xs text-muted-foreground">
                总入口
              </p>
              <p class="mt-1 text-2xl font-semibold">
                {{ config.items.length }}
              </p>
            </div>
            <div class="rounded-xl border border-border bg-muted/30 px-4 py-3">
              <p class="text-xs text-muted-foreground">
                已启用
              </p>
              <p class="mt-1 text-2xl font-semibold">
                {{ enabledCount }}
              </p>
            </div>
          </div>

          <div class="flex flex-wrap items-center gap-3">
            <div class="flex items-center gap-3 rounded-xl border border-border bg-background px-4 py-3">
              <Label class="text-sm font-medium">
                启用集成
              </Label>
              <Switch v-model="config.enabled" />
            </div>
            <Button
              variant="outline"
              :disabled="config.items.length >= maxItems"
              @click="addItem"
            >
              <Plus class="mr-2 h-4 w-4" />
              新增入口
            </Button>
          </div>
        </div>
      </CardSection>

      <div class="space-y-4">
        <div
          v-if="config.items.length === 0"
          class="rounded-xl border border-dashed border-border bg-card px-6 py-12 text-center"
        >
          <ExternalLink class="mx-auto h-8 w-8 text-muted-foreground" />
          <p class="mt-3 text-sm font-medium text-foreground">
            还没有外部系统入口
          </p>
        </div>

        <section
          v-for="(item, index) in config.items"
          :key="item.id || index"
          class="rounded-xl border border-border bg-card p-4 shadow-sm"
        >
          <div class="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
            <div class="min-w-0 space-y-1">
              <div class="flex items-center gap-3">
                <span
                  class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border bg-muted/40"
                  :style="{ color: item.color || '#64748b' }"
                >
                  <!-- eslint-disable vue/no-v-html -->
                  <span
                    v-if="itemIconSvg(item)"
                    class="inline-flex h-5 w-5 [&>svg]:h-full [&>svg]:w-full"
                    v-html="itemIconSvg(item)"
                  />
                  <!-- eslint-enable vue/no-v-html -->
                  <component
                    :is="itemIconComponent(item.icon)"
                    v-else
                    class="h-5 w-5"
                  />
                </span>
                <h3 class="truncate text-base font-semibold text-foreground">
                  {{ item.name || '未命名入口' }}
                </h3>
              </div>
              <p class="truncate text-xs text-muted-foreground">
                {{ item.url || '未配置地址' }}
              </p>
            </div>

            <div class="flex flex-wrap items-center gap-2">
              <Switch v-model="item.enabled" />
              <Button
                variant="ghost"
                size="icon"
                class="h-9 w-9"
                title="上移"
                :disabled="index === 0"
                @click="moveItem(index, -1)"
              >
                <ArrowUp class="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-9 w-9"
                title="下移"
                :disabled="index === config.items.length - 1"
                @click="moveItem(index, 1)"
              >
                <ArrowDown class="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-9 w-9"
                title="预览"
                :disabled="hasChanges || !item.enabled || !config.enabled"
                @click="previewItem(item.id)"
              >
                <ExternalLink class="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-9 w-9 text-destructive"
                title="删除"
                @click="removeItem(index)"
              >
                <Trash2 class="h-4 w-4" />
              </Button>
            </div>
          </div>

          <div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
            <div class="space-y-2">
              <Label :for="`external-name-${index}`">
                名称
              </Label>
              <Input
                :id="`external-name-${index}`"
                v-model="item.name"
                maxlength="64"
                placeholder="状态页"
              />
            </div>
            <div class="space-y-2">
              <Label>可见范围</Label>
              <div class="grid grid-cols-3 rounded-lg border border-border bg-muted/30 p-1">
                <button
                  v-for="option in visibilityOptions"
                  :key="option.value"
                  type="button"
                  class="rounded-md px-3 py-2 text-sm font-medium transition"
                  :class="item.visibility === option.value ? 'bg-primary text-primary-foreground shadow-sm ring-1 ring-primary/35' : 'text-muted-foreground hover:bg-background/70 hover:text-foreground'"
                  @click="item.visibility = option.value"
                >
                  {{ option.label }}
                </button>
              </div>
            </div>
            <div class="space-y-2 md:col-span-2">
              <Label :for="`external-url-${index}`">
                页面 URL
              </Label>
              <Input
                :id="`external-url-${index}`"
                v-model="item.url"
                placeholder="https://status.example.com"
              />
            </div>
            <div class="space-y-2">
              <Label>打开方式</Label>
              <div class="grid grid-cols-2 rounded-lg border border-border bg-muted/30 p-1">
                <button
                  v-for="option in openModeOptions"
                  :key="option.value"
                  type="button"
                  class="rounded-md px-3 py-2 text-sm font-medium transition"
                  :class="item.open_mode === option.value ? 'bg-primary text-primary-foreground shadow-sm ring-1 ring-primary/35' : 'text-muted-foreground hover:bg-background/70 hover:text-foreground'"
                  @click="item.open_mode = option.value"
                >
                  {{ option.label }}
                </button>
              </div>
            </div>
            <div class="grid grid-cols-[minmax(0,1fr)_52px] gap-3">
              <div class="space-y-2">
                <Label>
                  图标
                </Label>
                <div class="grid grid-cols-[52px_minmax(0,1fr)] gap-3 rounded-lg border border-border bg-muted/20 p-2">
                  <div
                    class="flex h-12 w-12 items-center justify-center rounded-lg border border-border bg-background"
                    :style="{ color: item.color || '#64748b' }"
                  >
                    <!-- eslint-disable vue/no-v-html -->
                    <span
                      v-if="itemIconSvg(item)"
                      class="inline-flex h-6 w-6 [&>svg]:h-full [&>svg]:w-full"
                      v-html="itemIconSvg(item)"
                    />
                    <!-- eslint-enable vue/no-v-html -->
                    <component
                      :is="itemIconComponent(item.icon)"
                      v-else
                      class="h-6 w-6"
                    />
                  </div>
                  <div class="min-w-0 space-y-2">
                    <select
                      :id="`external-icon-${index}`"
                      v-model="item.icon"
                      class="h-10 w-full rounded-md border border-input bg-background px-3 text-sm"
                    >
                      <option
                        v-for="icon in iconOptions"
                        :key="icon"
                        :value="icon"
                      >
                        {{ icon }}
                      </option>
                    </select>
                    <div class="flex flex-wrap gap-2">
                      <input
                        :id="`external-icon-file-${index}`"
                        type="file"
                        accept=".svg,image/svg+xml"
                        class="sr-only"
                        @change="handleIconFileUpload(index, $event)"
                      >
                      <label
                        :for="`external-icon-file-${index}`"
                        class="inline-flex h-9 cursor-pointer items-center justify-center rounded-lg border border-border/60 bg-card/60 px-3 text-sm font-semibold text-foreground transition hover:border-primary/60 hover:bg-primary/10 hover:text-primary"
                      >
                        <Upload class="mr-2 h-4 w-4" />
                        上传 SVG
                      </label>
                      <Button
                        variant="ghost"
                        size="sm"
                        :disabled="!item.icon_svg"
                        @click="clearCustomIcon(index)"
                      >
                        <X class="mr-2 h-4 w-4" />
                        清除
                      </Button>
                    </div>
                  </div>
                </div>
              </div>
              <div class="space-y-2">
                <Label :for="`external-color-${index}`">
                  色彩
                </Label>
                <input
                  :id="`external-color-${index}`"
                  :value="item.color || '#64748b'"
                  type="color"
                  class="h-10 w-full rounded-md border border-input bg-background p-1"
                  @input="updateItemColor(index, $event)"
                >
              </div>
            </div>
            <div class="space-y-2 md:col-span-2">
              <Label :for="`external-description-${index}`">
                描述
              </Label>
              <Textarea
                :id="`external-description-${index}`"
                v-model="item.description"
                maxlength="200"
                rows="2"
              />
            </div>
          </div>

          <p
            v-if="itemIssue(index)"
            class="mt-3 rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive"
          >
            {{ itemIssue(index) }}
          </p>
        </section>
      </div>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, type Component } from 'vue'
import { useRouter } from 'vue-router'
import {
  Activity,
  ArrowDown,
  ArrowUp,
  BookOpen,
  Box,
  CreditCard,
  ExternalLink,
  FileText,
  Globe,
  LifeBuoy,
  MessageSquare,
  Plus,
  RefreshCw,
  Save,
  Server,
  Settings as SettingsIcon,
  Trash2,
  Upload,
  X,
} from 'lucide-vue-next'
import { PageContainer, PageHeader, CardSection } from '@/components/layout'
import {
  Button,
  Input,
  Label,
  Switch,
  Textarea,
} from '@/components/ui'
import {
  EXTERNAL_INTEGRATIONS_UPDATED_EVENT,
  modulesApi,
  validateExternalIntegrationUrl,
  type ExternalIntegrationItem,
  type ExternalIntegrationsConfig,
  type ExternalIntegrationOpenMode,
  type ExternalIntegrationVisibility,
} from '@/api/modules'
import { useModuleStore } from '@/stores/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import {
  EXTERNAL_INTEGRATION_ICON_SVG_MAX_LENGTH,
  sanitizeExternalIntegrationIconSvg,
  validateExternalIntegrationIconSvg,
} from '@/utils/externalIntegrationIcons'
import { log } from '@/utils/logger'

const maxItems = 20
const router = useRouter()
const moduleStore = useModuleStore()
const { success, error } = useToast()

const visibilityOptions: Array<{ value: ExternalIntegrationVisibility; label: string }> = [
  { value: 'admin', label: '管理员' },
  { value: 'user', label: '用户' },
  { value: 'all', label: '全部' },
]

const openModeOptions: Array<{ value: ExternalIntegrationOpenMode; label: string }> = [
  { value: 'embed', label: '嵌入' },
  { value: 'new_tab', label: '新窗口' },
]

const iconOptions = [
  'ExternalLink',
  'Activity',
  'BookOpen',
  'Box',
  'CreditCard',
  'FileText',
  'Globe',
  'LifeBuoy',
  'MessageSquare',
  'Server',
  'Settings',
]

const iconMap: Record<string, Component> = {
  ExternalLink,
  Activity,
  BookOpen,
  Box,
  CreditCard,
  FileText,
  Globe,
  LifeBuoy,
  MessageSquare,
  Server,
  Settings: SettingsIcon,
}

const defaultConfig: ExternalIntegrationsConfig = {
  enabled: false,
  items: [],
}

const loading = ref(false)
const saving = ref(false)
const config = ref<ExternalIntegrationsConfig>(cloneConfig(defaultConfig))
const originalConfig = ref<ExternalIntegrationsConfig>(cloneConfig(defaultConfig))

const enabledCount = computed(() => config.value.items.filter(item => item.enabled).length)
const hasChanges = computed(() => JSON.stringify(config.value) !== JSON.stringify(originalConfig.value))
const validationIssues = computed(() => validateConfig(config.value))

onMounted(() => {
  void loadConfig()
})

function cloneConfig(value: ExternalIntegrationsConfig): ExternalIntegrationsConfig {
  return {
    enabled: value.enabled,
    items: value.items.map(item => ({ ...item })),
  }
}

function normalizeItemIds(value: ExternalIntegrationsConfig) {
  const seen = new Set<string>()
  let nextIndex = 1

  for (const item of value.items) {
    item.icon_svg = item.icon_svg?.trim() || null
    const trimmed = item.id.trim()
    if (trimmed && /^[A-Za-z0-9_-]{1,64}$/.test(trimmed) && !seen.has(trimmed)) {
      item.id = trimmed
      seen.add(trimmed)
      const match = /^external_(\d+)$/.exec(trimmed)
      if (match) {
        nextIndex = Math.max(nextIndex, Number(match[1]) + 1)
      }
      continue
    }

    let candidate = `external_${nextIndex}`
    while (seen.has(candidate)) {
      nextIndex += 1
      candidate = `external_${nextIndex}`
    }
    item.id = candidate
    seen.add(candidate)
    nextIndex += 1
  }
}

function nextItemId(): string {
  const ids = config.value.items.map(item => item.id.trim())
  let maxIndex = 0
  for (const id of ids) {
    const match = /^external_(\d+)$/.exec(id)
    if (match) {
      maxIndex = Math.max(maxIndex, Number(match[1]))
    }
  }

  let candidate = `external_${maxIndex + 1}`
  while (ids.includes(candidate)) {
    maxIndex += 1
    candidate = `external_${maxIndex + 1}`
  }
  return candidate
}

function createItem(): ExternalIntegrationItem {
  return {
    id: nextItemId(),
    name: `外部系统 ${config.value.items.length + 1}`,
    url: '',
    enabled: true,
    visibility: 'admin',
    open_mode: 'embed',
    description: '',
    icon: 'ExternalLink',
    icon_svg: null,
    color: '#2563eb',
  }
}

async function loadConfig() {
  loading.value = true
  try {
    const loaded = await modulesApi.getExternalIntegrationsConfig()
    const normalized = cloneConfig(loaded)
    normalizeItemIds(normalized)
    config.value = normalized
    originalConfig.value = cloneConfig(normalized)
  } catch (err) {
    error(parseApiError(err, '加载外部系统集成配置失败'))
    log.error('加载外部系统集成配置失败:', err)
  } finally {
    loading.value = false
  }
}

async function saveConfig() {
  normalizeItemIds(config.value)
  const firstIssue = validationIssues.value[0]
  if (firstIssue) {
    error(firstIssue.message)
    return
  }

  saving.value = true
  try {
    const saved = await modulesApi.updateExternalIntegrationsConfig(config.value)
    config.value = cloneConfig(saved)
    originalConfig.value = cloneConfig(saved)
    await moduleStore.fetchModules()
    window.dispatchEvent(new Event(EXTERNAL_INTEGRATIONS_UPDATED_EVENT))
    success('外部系统集成配置已保存')
  } catch (err) {
    error(parseApiError(err, '保存外部系统集成配置失败'))
    log.error('保存外部系统集成配置失败:', err)
  } finally {
    saving.value = false
  }
}

function addItem() {
  if (config.value.items.length >= maxItems) return
  config.value.items.push(createItem())
}

function removeItem(index: number) {
  config.value.items.splice(index, 1)
}

function moveItem(index: number, direction: -1 | 1) {
  const target = index + direction
  if (target < 0 || target >= config.value.items.length) return
  const [item] = config.value.items.splice(index, 1)
  config.value.items.splice(target, 0, item)
}

function updateItemColor(index: number, event: Event) {
  const input = event.target as HTMLInputElement | null
  if (!input) return
  const item = config.value.items[index]
  if (item) item.color = input.value
}

function itemIconComponent(icon: string): Component {
  return iconMap[icon] || ExternalLink
}

function itemIconSvg(item: ExternalIntegrationItem): string {
  return sanitizeExternalIntegrationIconSvg(item.icon_svg)
}

async function handleIconFileUpload(index: number, event: Event) {
  const input = event.target as HTMLInputElement | null
  const file = input?.files?.[0]
  if (input) input.value = ''
  if (!file) return

  if (file.type !== 'image/svg+xml' && !file.name.toLowerCase().endsWith('.svg')) {
    error('请上传 SVG 文件')
    return
  }
  if (file.size > EXTERNAL_INTEGRATION_ICON_SVG_MAX_LENGTH) {
    error('SVG 图标不能超过 8KB')
    return
  }

  try {
    const text = (await file.text()).trim()
    const issue = validateExternalIntegrationIconSvg(text)
    if (issue) {
      error(issue)
      return
    }
    const item = config.value.items[index]
    if (!item) return
    item.icon_svg = text
    success('SVG 图标已载入')
  } catch (err) {
    error('读取 SVG 文件失败')
    log.error('读取外部系统图标失败:', err)
  }
}

function clearCustomIcon(index: number) {
  const item = config.value.items[index]
  if (item) item.icon_svg = null
}

function previewItem(id: string) {
  const trimmed = id.trim()
  if (!trimmed) return
  void router.push(`/dashboard/external/${encodeURIComponent(trimmed)}`)
}

function itemIssue(index: number): string | null {
  return validationIssues.value.find(issue => issue.index === index)?.message ?? null
}

function validateConfig(value: ExternalIntegrationsConfig): Array<{ index: number; message: string }> {
  const issues: Array<{ index: number; message: string }> = []
  if (value.items.length > maxItems) {
    issues.push({ index: 0, message: `最多配置 ${maxItems} 个外部系统入口` })
  }

  const seen = new Map<string, number>()
  value.items.forEach((item, index) => {
    const id = item.id.trim()
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(id)) {
      issues.push({ index, message: '标识仅支持 1-64 位字母、数字、下划线和短横线' })
    } else if (seen.has(id)) {
      issues.push({ index, message: `标识与第 ${(seen.get(id) ?? 0) + 1} 个入口重复` })
    } else {
      seen.set(id, index)
    }

    if (!item.name.trim() || item.name.trim().length > 64) {
      issues.push({ index, message: '名称不能为空，且不能超过 64 个字符' })
    }
    const urlError = validateExternalIntegrationUrl(item.url)
    if (urlError) {
      issues.push({ index, message: urlError })
    }
    if (item.description.length > 200) {
      issues.push({ index, message: '描述不能超过 200 个字符' })
    }
    if (item.color && !/^#[0-9a-fA-F]{3}([0-9a-fA-F]{3})?$/.test(item.color)) {
      issues.push({ index, message: '色彩必须是十六进制颜色' })
    }
    const iconIssue = validateExternalIntegrationIconSvg(item.icon_svg)
    if (iconIssue) {
      issues.push({ index, message: iconIssue })
    }
  })

  return issues
}
</script>
