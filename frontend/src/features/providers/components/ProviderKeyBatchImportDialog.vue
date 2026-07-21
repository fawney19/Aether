<template>
  <Dialog
    :model-value="open"
    title="批量导入 Key"
    :description="providerName ? `${providerName} · 名称和 Key 均为必填` : '名称和 Key 均为必填'"
    :icon="ListPlus"
    size="4xl"
    persistent
    @update:model-value="handleDialogUpdate"
  >
    <div class="space-y-3.5">
      <nav class="grid grid-cols-3 gap-1.5 rounded-xl bg-muted/40 p-1.5" aria-label="批量导入步骤">
        <button
          v-for="step in steps"
          :key="step.id"
          type="button"
          class="flex min-h-10 items-center justify-center gap-2 rounded-lg px-2 text-xs font-medium transition-[background-color,box-shadow,color,scale] active:scale-[0.96] disabled:cursor-default disabled:opacity-50 sm:text-sm"
          :class="currentStep === step.id
            ? 'bg-background text-foreground shadow-[0_0_0_1px_rgb(0_0_0/0.06),0_1px_2px_rgb(0_0_0/0.06)] dark:shadow-[0_0_0_1px_rgb(255_255_255/0.08)]'
            : 'text-muted-foreground hover:bg-background/60 hover:text-foreground'"
          :disabled="!canNavigateToStep(step.id)"
          @click="goToStep(step.id)"
        >
          <span
            class="flex h-5 w-5 shrink-0 items-center justify-center rounded-md text-[10px] tabular-nums"
            :class="currentStep === step.id ? 'bg-foreground text-background' : 'bg-muted text-muted-foreground'"
          >{{ step.id }}</span>
          <span class="truncate">{{ step.label }}</span>
        </button>
      </nav>

      <section
        v-if="currentStep === 1"
        class="overflow-hidden rounded-xl bg-background shadow-[0_0_0_1px_rgb(0_0_0/0.07),0_1px_2px_-1px_rgb(0_0_0/0.08),0_3px_8px_-3px_rgb(0_0_0/0.08)] dark:shadow-[0_0_0_1px_rgb(255_255_255/0.09)]"
      >
        <header class="flex min-h-14 items-center justify-between gap-3 border-b border-border/60 bg-muted/15 px-4 py-3">
          <div class="flex min-w-0 items-center gap-3">
            <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-foreground text-xs font-semibold text-background">1</span>
            <div class="min-w-0">
              <h3 class="text-balance text-sm font-semibold text-foreground">粘贴名称与 Key</h3>
              <p class="text-pretty text-[11px] leading-4 text-muted-foreground">每行一条，仅接受四个短横线分隔</p>
            </div>
          </div>
          <Badge
            :variant="parsed.errors.length > 0 ? 'destructive' : parsed.items.length > 0 ? 'success' : 'secondary'"
            class="shrink-0 tabular-nums"
          >
            {{ inputStatusText }}
          </Badge>
        </header>

        <div class="min-w-0">
            <Label for="provider-key-batch-input" class="sr-only">Key 列表</Label>
            <Textarea
              id="provider-key-batch-input"
              v-model="inputText"
              class="h-[280px] min-h-[220px] max-h-[520px] !resize-y !rounded-none !border-0 !bg-transparent !px-4 !py-4 font-mono text-[13px] leading-6 !shadow-none !ring-0 focus-visible:!ring-0"
              spellcheck="false"
              placeholder="主账号----sk-xxxx&#10;备用账号----sk-yyyy"
            />
            <div
              v-if="parsed.errors.length > 0"
              class="mx-4 mb-3 space-y-1 rounded-lg bg-destructive/5 px-3 py-2 text-[11px] text-destructive ring-1 ring-destructive/20"
            >
              <div
                v-for="(item, index) in parsed.errors.slice(0, 6)"
                :key="`${item.lineNumber}-${index}`"
              >
                {{ item.lineNumber ? `第 ${item.lineNumber} 行：` : '' }}{{ item.message }}
              </div>
              <div v-if="parsed.errors.length > 6" class="font-medium">
                另有 {{ parsed.errors.length - 6 }} 个问题
              </div>
            </div>
            <div class="flex flex-wrap items-center gap-2 border-t border-border/50 bg-muted/10 px-4 py-2.5 text-[11px] text-muted-foreground">
              <span class="rounded-md bg-muted px-2 py-1 font-mono text-foreground/80">名称----Key</span>
              <span>名称和 Key 都不能为空</span>
              <span class="ml-auto hidden tabular-nums sm:inline">已识别 {{ parsed.items.length }} 条</span>
            </div>
          </div>
      </section>

      <section
        v-else-if="currentStep === 2"
        class="overflow-hidden rounded-xl bg-background shadow-[0_0_0_1px_rgb(0_0_0/0.07),0_1px_2px_-1px_rgb(0_0_0/0.08),0_3px_8px_-3px_rgb(0_0_0/0.08)] dark:shadow-[0_0_0_1px_rgb(255_255_255/0.09)]"
      >
        <header class="flex min-h-[72px] items-center gap-3 px-4 py-3">
          <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-foreground text-xs font-semibold text-background">
            2
          </span>
          <span class="min-w-0 flex-1">
            <span class="block text-sm font-semibold">统一配置</span>
            <span class="mt-1 flex flex-wrap gap-1.5">
              <span
                v-for="item in settingsSummaryItems"
                :key="item"
                class="rounded-md bg-muted px-2 py-0.5 text-[10px] leading-4 text-muted-foreground"
              >{{ item }}</span>
            </span>
          </span>
          <Badge :variant="selectedApiFormats.length > 0 ? 'success' : 'destructive'" class="ml-auto shrink-0 tabular-nums">
            {{ selectedApiFormats.length }} 种格式
          </Badge>
        </header>

        <div class="border-t border-border/60 bg-muted/10 p-3 sm:p-4">
          <ProviderKeyImportSettingsFields
            :auth-type="authType"
            :api-formats="selectedApiFormats"
            :settings="settings"
            :available-api-formats="availableApiFormats"
            @update:auth-type="authType = $event"
            @update:api-formats="selectedApiFormats = $event"
            @update:settings="updateGlobalSettings"
          />
        </div>
      </section>

      <section
        v-else
        class="overflow-hidden rounded-xl bg-background shadow-[0_0_0_1px_rgb(0_0_0/0.07),0_1px_2px_-1px_rgb(0_0_0/0.08),0_3px_8px_-3px_rgb(0_0_0/0.08)] dark:shadow-[0_0_0_1px_rgb(255_255_255/0.09)]"
      >
        <header class="flex min-h-[72px] items-center gap-3 border-b border-border/60 bg-muted/15 px-4 py-3">
          <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-foreground text-xs font-semibold text-background">3</span>
          <div class="min-w-0 flex-1">
            <h3 class="text-balance text-sm font-semibold">逐项确认</h3>
            <p class="text-pretty text-[11px] leading-4 text-muted-foreground">展开任意 Key 可修改内容或设置单独配置</p>
          </div>
          <div class="shrink-0 text-right text-[11px] text-muted-foreground">
            <div><span class="font-semibold tabular-nums text-foreground">{{ reviewItems.length }}</span> 个 Key</div>
            <div v-if="customizedItemCount > 0"><span class="tabular-nums">{{ customizedItemCount }}</span> 个单独配置</div>
          </div>
        </header>

        <div
          v-if="reviewErrorItemCount > 0"
          class="border-b border-destructive/15 bg-destructive/5 px-4 py-2 text-xs text-destructive"
        >
          {{ reviewErrorItemCount }} 个 Key 需要修正后才能导入
        </div>

        <div class="max-h-[min(52vh,520px)] divide-y divide-border/50 overflow-y-auto overscroll-contain">
          <article
            v-for="entry in pagedReviewItems"
            :key="entry.item.lineNumber"
            class="bg-background"
          >
            <div class="flex min-h-14 items-center gap-3 px-3 py-2 sm:px-4">
              <span class="flex h-7 w-7 shrink-0 items-center justify-center rounded-md bg-muted text-[10px] tabular-nums text-muted-foreground">
                {{ entry.index + 1 }}
              </span>
              <div class="min-w-0 flex-1">
                <div class="flex min-w-0 items-center gap-2">
                  <span class="truncate text-xs font-semibold">{{ entry.item.name || '未填写名称' }}</span>
                  <Badge
                    v-if="reviewErrorsByIndex.has(entry.index)"
                    variant="destructive"
                    class="shrink-0 text-[10px]"
                  >需修正</Badge>
                </div>
                <div class="mt-0.5 truncate font-mono text-[10px] text-muted-foreground">{{ maskSecret(entry.item.apiKey) }}</div>
              </div>
              <div class="hidden shrink-0 items-center gap-1.5 sm:flex">
                <span class="rounded-md bg-muted px-2 py-0.5 text-[10px] text-muted-foreground">{{ effectiveAuthLabel(entry.item) }}</span>
                <span class="rounded-md bg-muted px-2 py-0.5 text-[10px] text-muted-foreground">{{ effectiveFormatCount(entry.item) }} 种格式</span>
                <span
                  class="rounded-md px-2 py-0.5 text-[10px]"
                  :class="entry.item.customized ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground'"
                >{{ entry.item.customized ? '单独配置' : '统一配置' }}</span>
              </div>
              <button
                type="button"
                class="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-[background-color,color,scale] hover:bg-muted hover:text-foreground active:scale-[0.96]"
                :aria-label="`编辑 ${entry.item.name || `第 ${entry.index + 1} 项`}`"
                :title="editingItemIndex === entry.index ? '收起编辑' : '编辑此 Key'"
                @click="toggleItemEditor(entry.index)"
              >
                <Pencil class="h-4 w-4" />
              </button>
            </div>

            <div
              v-if="editingItemIndex === entry.index"
              class="space-y-4 border-t border-border/50 bg-muted/10 p-3 sm:p-4"
            >
              <div class="grid gap-3 sm:grid-cols-2">
                <div class="space-y-1.5">
                  <Label class="text-xs">名称</Label>
                  <Input v-model="entry.item.name" class="h-10" placeholder="必填" />
                </div>
                <div class="space-y-1.5">
                  <Label class="text-xs">Key</Label>
                  <Input v-model="entry.item.apiKey" class="h-10 font-mono text-xs" placeholder="必填" />
                </div>
              </div>

              <div class="flex min-h-12 items-center justify-between gap-3 rounded-lg bg-background px-3 shadow-[0_0_0_1px_rgb(0_0_0/0.06)] dark:shadow-[0_0_0_1px_rgb(255_255_255/0.08)]">
                <div>
                  <div class="text-xs font-medium">单独配置此 Key</div>
                  <div class="text-[11px] text-muted-foreground">开启后覆盖第二步中的统一配置</div>
                </div>
                <Switch
                  :model-value="entry.item.customized"
                  @update:model-value="setItemCustomized(entry.item, $event)"
                />
              </div>

              <ProviderKeyImportSettingsFields
                v-if="entry.item.customized"
                :auth-type="entry.item.authType"
                :api-formats="entry.item.apiFormats"
                :settings="entry.item.settings"
                :available-api-formats="availableApiFormats"
                @update:auth-type="entry.item.authType = $event"
                @update:api-formats="entry.item.apiFormats = $event"
                @update:settings="entry.item.settings = $event"
              />

              <div
                v-if="reviewErrorsByIndex.has(entry.index)"
                class="space-y-1 rounded-lg bg-destructive/5 px-3 py-2 text-[11px] text-destructive"
              >
                <div v-for="message in reviewErrorsByIndex.get(entry.index)" :key="message">{{ message }}</div>
              </div>
            </div>
          </article>
        </div>

        <div
          v-if="reviewPageCount > 1"
          class="flex min-h-12 items-center justify-between gap-3 border-t border-border/60 bg-muted/10 px-3 sm:px-4"
        >
          <Button variant="ghost" size="sm" class="h-9" :disabled="reviewPage === 1" @click="changeReviewPage(reviewPage - 1)">
            <ChevronLeft class="mr-1 h-4 w-4" />
            上一页
          </Button>
          <span class="text-[11px] tabular-nums text-muted-foreground">{{ reviewPage }} / {{ reviewPageCount }}</span>
          <Button variant="ghost" size="sm" class="h-9" :disabled="reviewPage === reviewPageCount" @click="changeReviewPage(reviewPage + 1)">
            下一页
            <ChevronRight class="ml-1 h-4 w-4" />
          </Button>
        </div>
      </section>
    </div>

    <template #footer>
      <div class="flex w-full flex-col-reverse gap-2 sm:flex-row sm:justify-end">
        <Button class="w-full sm:w-auto" variant="outline" :disabled="importing" @click="handleBack">
          <ArrowLeft v-if="currentStep > 1" class="mr-2 h-4 w-4" />
          {{ currentStep === 1 ? '取消' : '上一步' }}
        </Button>
        <Button class="w-full sm:w-auto" :disabled="primaryActionDisabled" @click="handlePrimaryAction">
          <Loader2 v-if="importing" class="mr-2 h-4 w-4 animate-spin" />
          <ListPlus v-else-if="currentStep === 3" class="mr-2 h-4 w-4" />
          <ArrowRight v-else class="mr-2 h-4 w-4" />
          {{ primaryActionLabel }}
        </Button>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import {
  ArrowLeft,
  ArrowRight,
  ChevronLeft,
  ChevronRight,
  ListPlus,
  Loader2,
  Pencil,
} from 'lucide-vue-next'
import {
  Badge,
  Button,
  Dialog,
  Input,
  Label,
  Switch,
  Textarea,
} from '@/components/ui'
import ProviderKeyImportSettingsFields from './ProviderKeyImportSettingsFields.vue'
import { batchImportPoolKeys, type PoolKeySettingsPatch } from '@/api/endpoints/pool'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { parseProviderKeyBatchImport } from '@/features/providers/utils/providerKeyBatchImport'

type WizardStep = 1 | 2 | 3
type AuthType = 'api_key' | 'bearer'
type ImportSettings = Required<Pick<PoolKeySettingsPatch,
  'internal_priority' | 'rpm_limit' | 'concurrent_limit' | 'cache_ttl_minutes'
  | 'max_probe_interval_minutes' | 'is_active' | 'note' | 'proxy_node_id'
>>

interface ReviewImportItem {
  lineNumber: number
  name: string
  apiKey: string
  customized: boolean
  authType: AuthType
  apiFormats: string[]
  settings: ImportSettings
}

const REVIEW_PAGE_SIZE = 50
const steps: ReadonlyArray<{ id: WizardStep; label: string }> = [
  { id: 1, label: '导入内容' },
  { id: 2, label: '统一配置' },
  { id: 3, label: '逐项确认' },
]

const props = defineProps<{
  open: boolean
  providerId: string
  providerName?: string
  availableApiFormats: string[]
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, warning, error: showError } = useToast()
const currentStep = ref<WizardStep>(1)
const inputText = ref('')
const importing = ref(false)
const authType = ref<AuthType>('api_key')
const selectedApiFormats = ref<string[]>([])
const settings = reactive<ImportSettings>(createDefaultSettings())
const reviewItems = ref<ReviewImportItem[]>([])
const reviewPage = ref(1)
const editingItemIndex = ref<number | null>(null)

const parsed = computed(() => parseProviderKeyBatchImport(inputText.value))
const canContinueInput = computed(() => (
  parsed.value.items.length > 0 && parsed.value.errors.length === 0
))
const canContinueSettings = computed(() => selectedApiFormats.value.length > 0)
const inputStatusText = computed(() => {
  if (parsed.value.errors.length > 0) return `${parsed.value.errors.length} 个问题`
  if (parsed.value.items.length > 0) return `${parsed.value.items.length} 条有效`
  return '等待输入'
})
const reviewErrorsByIndex = computed(() => {
  const errors = new Map<number, string[]>()
  const seenNames = new Set<string>()
  const seenKeys = new Set<string>()

  reviewItems.value.forEach((item, index) => {
    const messages: string[] = []
    const name = item.name.trim()
    const apiKey = item.apiKey.trim()
    if (!name) messages.push('名称不能为空')
    else if (name.length > 100) messages.push('名称不能超过 100 个字符')
    else if (seenNames.has(name)) messages.push('名称与前面的 Key 重复')
    else seenNames.add(name)

    if (!apiKey) messages.push('Key 不能为空')
    else if (seenKeys.has(apiKey)) messages.push('Key 与前面的 Key 重复')
    else seenKeys.add(apiKey)

    if (item.customized && item.apiFormats.length === 0) {
      messages.push('单独配置时至少选择一种 API 格式')
    }
    if (messages.length > 0) errors.set(index, messages)
  })
  return errors
})
const reviewErrorItemCount = computed(() => reviewErrorsByIndex.value.size)
const customizedItemCount = computed(() => (
  reviewItems.value.filter(item => item.customized).length
))
const reviewPageCount = computed(() => (
  Math.max(1, Math.ceil(reviewItems.value.length / REVIEW_PAGE_SIZE))
))
const pagedReviewItems = computed(() => {
  const start = (reviewPage.value - 1) * REVIEW_PAGE_SIZE
  return reviewItems.value
    .slice(start, start + REVIEW_PAGE_SIZE)
    .map((item, offset) => ({ item, index: start + offset }))
})
const canImport = computed(() => (
  !importing.value
  && reviewItems.value.length > 0
  && reviewErrorsByIndex.value.size === 0
  && canContinueSettings.value
))
const primaryActionDisabled = computed(() => {
  if (importing.value) return true
  if (currentStep.value === 1) return !canContinueInput.value
  if (currentStep.value === 2) return !canContinueSettings.value
  return !canImport.value
})
const primaryActionLabel = computed(() => {
  if (importing.value) return '正在导入...'
  if (currentStep.value === 1) return '下一步：统一配置'
  if (currentStep.value === 2) return '下一步：逐项确认'
  return `导入 ${reviewItems.value.length} 个 Key`
})
const settingsSummaryItems = computed(() => {
  const rpm = settings.rpm_limit == null ? 'RPM 自适应' : `RPM ${settings.rpm_limit}`
  const concurrent = settings.concurrent_limit == null || settings.concurrent_limit === 0
    ? '不限并发'
    : `并发 ${settings.concurrent_limit}`
  const proxy = settings.proxy_node_id ? '独立代理' : '沿用 Provider 代理'
  return [authType.value === 'bearer' ? 'Bearer' : 'API Key', rpm, concurrent, proxy]
})

watch(
  () => props.open,
  (open) => {
    if (!open) return
    currentStep.value = 1
    inputText.value = ''
    importing.value = false
    authType.value = 'api_key'
    selectedApiFormats.value = [...props.availableApiFormats]
    Object.assign(settings, createDefaultSettings())
    reviewItems.value = []
    reviewPage.value = 1
    editingItemIndex.value = null
  },
  { immediate: true },
)

watch(inputText, () => {
  reviewItems.value = []
  reviewPage.value = 1
  editingItemIndex.value = null
})

function createDefaultSettings(): ImportSettings {
  return {
    internal_priority: 50,
    rpm_limit: null,
    concurrent_limit: null,
    cache_ttl_minutes: 5,
    max_probe_interval_minutes: 32,
    is_active: true,
    note: '',
    proxy_node_id: '',
  }
}

function copySettings(source: ImportSettings): ImportSettings {
  return { ...source }
}

function buildSettingsPayload(
  source: ImportSettings,
  includeEmptyProxy = false,
): PoolKeySettingsPatch {
  return {
    internal_priority: source.internal_priority,
    rpm_limit: source.rpm_limit,
    concurrent_limit: source.concurrent_limit,
    cache_ttl_minutes: source.cache_ttl_minutes,
    max_probe_interval_minutes: source.max_probe_interval_minutes,
    is_active: source.is_active,
    note: source.note.trim() || null,
    ...((source.proxy_node_id || includeEmptyProxy)
      ? { proxy_node_id: source.proxy_node_id || null }
      : {}),
  }
}

function handleDialogUpdate(value: boolean): void {
  if (!value) emit('close')
}

function canNavigateToStep(step: WizardStep): boolean {
  return step <= currentStep.value
}

function goToStep(step: WizardStep): void {
  if (canNavigateToStep(step)) currentStep.value = step
}

function handleBack(): void {
  if (currentStep.value === 1) {
    emit('close')
    return
  }
  currentStep.value = (currentStep.value - 1) as WizardStep
}

function handlePrimaryAction(): void {
  if (primaryActionDisabled.value) return
  if (currentStep.value === 1) {
    currentStep.value = 2
    return
  }
  if (currentStep.value === 2) {
    prepareReviewItems()
    currentStep.value = 3
    return
  }
  void submitImport()
}

function prepareReviewItems(): void {
  if (reviewItems.value.length > 0) return
  reviewItems.value = parsed.value.items.map(item => ({
    lineNumber: item.lineNumber,
    name: item.name,
    apiKey: item.apiKey,
    customized: false,
    authType: authType.value,
    apiFormats: [...selectedApiFormats.value],
    settings: copySettings(settings),
  }))
  reviewPage.value = 1
  editingItemIndex.value = null
}

function toggleItemEditor(index: number): void {
  editingItemIndex.value = editingItemIndex.value === index ? null : index
}

function setItemCustomized(item: ReviewImportItem, customized: boolean): void {
  item.customized = customized
  if (!customized) return
  item.authType = authType.value
  item.apiFormats = [...selectedApiFormats.value]
  item.settings = copySettings(settings)
}

function changeReviewPage(page: number): void {
  reviewPage.value = Math.min(Math.max(page, 1), reviewPageCount.value)
  editingItemIndex.value = null
}

function effectiveAuthLabel(item: ReviewImportItem): string {
  const resolved = item.customized ? item.authType : authType.value
  return resolved === 'bearer' ? 'Bearer' : 'API Key'
}

function effectiveFormatCount(item: ReviewImportItem): number {
  return item.customized ? item.apiFormats.length : selectedApiFormats.value.length
}

function updateGlobalSettings(nextSettings: ImportSettings): void {
  Object.assign(settings, nextSettings)
}

function maskSecret(secret: string): string {
  if (secret.length <= 10) return `${secret.slice(0, 3)}•••`
  return `${secret.slice(0, 6)}••••${secret.slice(-4)}`
}

async function submitImport(): Promise<void> {
  if (!canImport.value) return
  importing.value = true
  try {
    const result = await batchImportPoolKeys(props.providerId, {
      keys: reviewItems.value.map(item => ({
        name: item.name.trim(),
        api_key: item.apiKey.trim(),
        auth_type: item.customized ? item.authType : authType.value,
        ...(item.customized
          ? {
              api_formats: item.apiFormats,
              settings: buildSettingsPayload(item.settings, true),
            }
          : {}),
      })),
      api_formats: selectedApiFormats.value,
      settings: buildSettingsPayload(settings),
    })
    if (result.imported > 0) emit('saved')
    if (result.errors.length > 0) {
      warning(`已导入 ${result.imported} 个，${result.errors.length} 个失败`)
      return
    }
    success(`已导入 ${result.imported} 个 Key`)
    emit('close')
  } catch (error) {
    showError(parseApiError(error, '批量导入 Key 失败'))
  } finally {
    importing.value = false
  }
}
</script>