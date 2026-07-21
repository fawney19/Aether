<template>
  <Dialog
    :model-value="open"
    title="批量编辑密钥"
    :description="dialogDescription"
    :icon="SquarePen"
    size="3xl"
    persistent
    @update:model-value="handleDialogUpdate"
  >
    <Tabs v-model="activeTab">
      <TabsList class="grid w-full grid-cols-2">
        <TabsTrigger value="configuration">
          密钥配置
        </TabsTrigger>
        <TabsTrigger value="models">
          可用模型范围
        </TabsTrigger>
      </TabsList>

      <TabsContent
        value="configuration"
        class="max-h-[min(64vh,40rem)] overflow-y-auto pr-1"
      >
        <div class="divide-y divide-border/70">
          <section class="space-y-3 py-4 first:pt-2">
            <label class="flex items-center gap-2 text-sm font-medium">
              <Checkbox v-model="form.applyApiFormats" />
              <span>支持的 API</span>
            </label>
            <div
              class="grid gap-2 sm:grid-cols-2"
              :class="!form.applyApiFormats ? 'pointer-events-none opacity-45' : ''"
            >
              <label
                v-for="format in visibleApiFormats"
                :key="format"
                class="flex min-h-9 cursor-pointer items-center gap-2 rounded-md border px-3 py-2 text-sm transition-colors hover:bg-muted/40"
                :class="form.apiFormats.includes(format) ? 'border-primary/50 bg-primary/5' : 'border-border/70'"
              >
                <Checkbox
                  :checked="form.apiFormats.includes(format)"
                  :disabled="!form.applyApiFormats"
                  @update:checked="checked => toggleApiFormat(format, checked)"
                />
                <span class="truncate">{{ formatApiFormat(format) }}</span>
              </label>
              <p
                v-if="visibleApiFormats.length === 0"
                class="text-xs text-muted-foreground sm:col-span-2"
              >
                当前提供商没有可配置的 API 格式
              </p>
            </div>
          </section>

          <section class="space-y-3 py-4">
            <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
              <BatchFieldToggle
                v-model="form.applyActive"
                label="启用状态"
              >
                <Select
                  v-model="activeValue"
                  :disabled="!form.applyActive"
                >
                  <SelectTrigger class="h-9">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="enabled">
                      启用
                    </SelectItem>
                    <SelectItem value="disabled">
                      禁用
                    </SelectItem>
                  </SelectContent>
                </Select>
              </BatchFieldToggle>

              <BatchFieldToggle
                v-model="form.applyInternalPriority"
                label="优先级"
              >
                <Input
                  v-model="form.internalPriority"
                  type="number"
                  min="0"
                  class="h-9"
                  :disabled="!form.applyInternalPriority"
                />
              </BatchFieldToggle>

              <BatchFieldToggle
                v-model="form.applyRpmLimit"
                label="RPM 限制"
              >
                <Input
                  v-model="form.rpmLimit"
                  type="number"
                  min="1"
                  max="10000"
                  placeholder="自适应"
                  class="h-9"
                  :disabled="!form.applyRpmLimit"
                />
              </BatchFieldToggle>

              <BatchFieldToggle
                v-model="form.applyConcurrentLimit"
                label="并发请求上限"
              >
                <Input
                  v-model="form.concurrentLimit"
                  type="number"
                  min="0"
                  placeholder="不限制"
                  class="h-9"
                  :disabled="!form.applyConcurrentLimit"
                />
              </BatchFieldToggle>

              <BatchFieldToggle
                v-model="form.applyCacheTtl"
                label="缓存 TTL"
              >
                <div class="relative">
                  <Input
                    v-model="form.cacheTtlMinutes"
                    type="number"
                    min="0"
                    max="60"
                    class="h-9 pr-12"
                    :disabled="!form.applyCacheTtl"
                  />
                  <span class="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-muted-foreground">分钟</span>
                </div>
              </BatchFieldToggle>

              <BatchFieldToggle
                v-model="form.applyProbeInterval"
                label="熔断探测"
              >
                <div class="relative">
                  <Input
                    v-model="form.maxProbeIntervalMinutes"
                    type="number"
                    min="0"
                    max="32"
                    class="h-9 pr-12"
                    :disabled="!form.applyProbeInterval"
                  />
                  <span class="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-xs text-muted-foreground">分钟</span>
                </div>
              </BatchFieldToggle>
            </div>
          </section>

          <section class="space-y-3 py-4">
            <BatchFieldToggle
              v-model="form.applyAutoFetchModels"
              label="应用自动获取设置"
            >
              <div class="space-y-3 rounded-md border border-border/60 bg-muted/30 px-3 py-3">
                <div class="flex items-center justify-between gap-4">
                  <div class="space-y-0.5">
                    <Label class="text-sm font-medium">自动获取上游可用模型</Label>
                    <p class="text-xs text-muted-foreground">
                      定时更新上游模型，配合模型映射使用
                    </p>
                  </div>
                  <Switch
                    v-model="form.autoFetchModels"
                    :disabled="!form.applyAutoFetchModels"
                  />
                </div>

                <div
                  v-if="form.autoFetchModels"
                  class="space-y-2 border-t border-border/40 pt-3"
                >
                  <div class="grid gap-3 sm:grid-cols-2">
                    <div class="space-y-1.5">
                      <Label class="text-xs">包含规则</Label>
                      <Input
                        v-model="form.includePatterns"
                        placeholder="gpt-*, claude-*, 留空包含全部"
                        class="h-9"
                        :disabled="!form.applyAutoFetchModels"
                      />
                    </div>
                    <div class="space-y-1.5">
                      <Label class="text-xs">排除规则</Label>
                      <Input
                        v-model="form.excludePatterns"
                        placeholder="*-preview, *-beta"
                        class="h-9"
                        :disabled="!form.applyAutoFetchModels"
                      />
                    </div>
                  </div>
                  <p class="text-xs text-muted-foreground">
                    逗号分隔，支持 * ? 通配符，不区分大小写
                  </p>
                </div>
              </div>
            </BatchFieldToggle>
          </section>

          <section class="space-y-3 py-4">
            <label class="flex items-center gap-2 text-sm font-medium">
              <Checkbox v-model="form.applyNote" />
              <span>备注</span>
            </label>
            <Textarea
              v-model="form.note"
              rows="3"
              placeholder="留空可清除备注"
              :disabled="!form.applyNote"
            />
          </section>
        </div>
      </TabsContent>

      <TabsContent
        value="models"
        class="max-h-[min(64vh,40rem)] overflow-y-auto pr-1"
      >
        <div class="space-y-4 py-1">
          <div class="space-y-1 border-b border-border/70 pb-4">
            <label class="flex items-center gap-2 text-sm font-medium">
              <Checkbox v-model="form.applyAllowedModels" />
              <span>应用可用模型范围</span>
            </label>
            <p class="pl-6 text-xs text-muted-foreground">
              限制所选账号能够承接的模型；不限制时允许全部模型
            </p>
          </div>

          <div
            class="space-y-4"
            :class="!form.applyAllowedModels ? 'pointer-events-none opacity-45' : ''"
          >
            <div class="flex items-center justify-between gap-4 border-b border-border/70 pb-4">
              <div class="min-w-0">
                <p class="text-sm font-medium">
                  允许全部模型
                </p>
                <p class="text-xs text-muted-foreground">
                  关闭后仅允许下方选中的模型
                </p>
              </div>
              <Switch
                v-model="form.unrestrictedModels"
                :disabled="!form.applyAllowedModels"
              />
            </div>

            <div
              class="space-y-3"
              :class="modelSelectionDisabled ? 'pointer-events-none opacity-45' : ''"
            >
              <div class="flex flex-col gap-2 sm:flex-row sm:items-center">
                <div class="relative min-w-0 flex-1">
                  <Search class="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    v-model="modelSearch"
                    placeholder="搜索或输入自定义模型"
                    class="h-9 pl-8"
                    :disabled="modelSelectionDisabled"
                  />
                </div>
                <Button
                  variant="outline"
                  class="h-9 shrink-0"
                  :disabled="fetchingUpstreamModels || modelSelectionDisabled"
                  @click="fetchUpstreamModels(true)"
                >
                  <RefreshCw
                    class="mr-2 h-4 w-4"
                    :class="fetchingUpstreamModels ? 'animate-spin' : ''"
                  />
                  获取上游模型
                </Button>
              </div>

              <div class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
                <span>已选择 {{ form.selectedModels.length }} 个模型</span>
                <button
                  v-if="filteredModels.length > 0"
                  type="button"
                  class="text-primary hover:underline"
                  :disabled="modelSelectionDisabled"
                  @click="toggleFilteredModels"
                >
                  {{ areFilteredModelsSelected ? '取消当前结果' : '选择当前结果' }}
                </button>
              </div>

              <div class="overflow-hidden rounded-md border border-border/70">
                <button
                  v-if="canAddCustomModel"
                  type="button"
                  class="flex w-full items-center gap-2 border-b border-dashed px-3 py-2 text-left text-sm hover:bg-muted/40"
                  :disabled="modelSelectionDisabled"
                  @click="addCustomModel"
                >
                  <Plus class="h-4 w-4 text-muted-foreground" />
                  <span class="min-w-0 flex-1 truncate font-mono">{{ normalizedModelSearch }}</span>
                  <span class="text-xs text-muted-foreground">添加</span>
                </button>
                <div class="max-h-72 overflow-y-auto">
                  <label
                    v-for="model in filteredModels"
                    :key="model.id"
                    class="flex cursor-pointer items-center gap-2 border-b border-border/60 px-3 py-2 last:border-b-0 hover:bg-muted/30"
                  >
                    <Checkbox
                      :checked="form.selectedModels.includes(model.id)"
                      :disabled="modelSelectionDisabled"
                      @update:checked="checked => toggleModel(model.id, checked)"
                    />
                    <span class="min-w-0 flex-1 truncate font-mono text-sm">{{ model.id }}</span>
                    <Badge
                      variant="outline"
                      class="h-5 shrink-0 px-1.5 text-[10px]"
                    >
                      {{ model.source }}
                    </Badge>
                  </label>
                  <div
                    v-if="loadingModels"
                    class="flex items-center justify-center py-10 text-muted-foreground"
                  >
                    <Loader2 class="h-5 w-5 animate-spin" />
                  </div>
                  <div
                    v-else-if="filteredModels.length === 0"
                    class="py-10 text-center text-sm text-muted-foreground"
                  >
                    暂无匹配模型
                  </div>
                </div>
              </div>

              <div class="flex items-center justify-between gap-4 rounded-md border border-border/60 bg-muted/30 px-3 py-2.5">
                <div class="min-w-0">
                  <p class="text-sm font-medium">
                    锁定已选模型
                  </p>
                  <p class="text-xs text-muted-foreground">
                    自动获取开启时，锁定的模型不会被同步移除
                  </p>
                </div>
                <Switch
                  v-model="form.lockSelectedModels"
                  :disabled="modelSelectionDisabled"
                />
              </div>
            </div>
          </div>
        </div>
      </TabsContent>
    </Tabs>

    <template #footer>
      <Button
        variant="outline"
        :disabled="saving"
        @click="closeDialog"
      >
        取消
      </Button>
      <Button
        :disabled="saving"
        @click="saveChanges"
      >
        <Loader2
          v-if="saving"
          class="mr-2 h-4 w-4 animate-spin"
        />
        {{ saving ? '保存中...' : `应用到 ${keyIds.length} 个密钥` }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
  Textarea,
} from '@/components/ui'
import { Loader2, Plus, RefreshCw, Search, SquarePen } from 'lucide-vue-next'
import { getProviderModels } from '@/api/endpoints/models'
import { batchUpdatePoolKeys } from '@/api/endpoints/pool'
import { formatApiFormat, sortApiFormats, type UpstreamModel } from '@/api/endpoints/types'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { useUpstreamModelsCache } from '@/features/providers/composables/useUpstreamModelsCache'
import BatchFieldToggle from './PoolKeyBatchFieldToggle.vue'
import {
  buildPoolKeyBatchUpdatePatch,
  type PoolKeyBatchEditState,
} from '../utils/poolKeyBatchEdit'

interface ModelOption {
  id: string
  source: '提供商' | '上游' | '自定义'
}

const props = defineProps<{
  open: boolean
  providerId: string
  providerName?: string
  keyIds: string[]
  availableApiFormats: string[]
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, warning, error: showError } = useToast()
const { confirm } = useConfirm()
const { fetchModelsForKeys } = useUpstreamModelsCache()

const activeTab = ref('configuration')
const saving = ref(false)
const loadingProviderModels = ref(false)
const fetchingUpstreamModels = ref(false)
const providerModelIds = ref<string[]>([])
const upstreamModels = ref<UpstreamModel[]>([])
const modelSearch = ref('')

function createInitialForm(): PoolKeyBatchEditState {
  return {
    applyApiFormats: false,
    apiFormats: [],
    applyActive: false,
    isActive: true,
    applyInternalPriority: false,
    internalPriority: '0',
    applyRpmLimit: false,
    rpmLimit: '',
    applyConcurrentLimit: false,
    concurrentLimit: '',
    applyCacheTtl: false,
    cacheTtlMinutes: '5',
    applyProbeInterval: false,
    maxProbeIntervalMinutes: '32',
    applyNote: false,
    note: '',
    applyAutoFetchModels: false,
    autoFetchModels: false,
    includePatterns: '',
    excludePatterns: '',
    applyAllowedModels: false,
    unrestrictedModels: true,
    selectedModels: [],
    lockSelectedModels: true,
  }
}

const form = reactive<PoolKeyBatchEditState>(createInitialForm())

const dialogDescription = computed(() => {
  const providerName = props.providerName?.trim()
  const prefix = providerName ? `${providerName} · ` : ''
  return `${prefix}已选 ${props.keyIds.length} 个密钥`
})
const visibleApiFormats = computed(() => sortApiFormats(props.availableApiFormats || []))
const loadingModels = computed(() => loadingProviderModels.value || fetchingUpstreamModels.value)
const normalizedModelSearch = computed(() => modelSearch.value.trim())
const modelSelectionDisabled = computed(() => (
  !form.applyAllowedModels || form.unrestrictedModels
))
const activeValue = computed({
  get: () => form.isActive ? 'enabled' : 'disabled',
  set: value => { form.isActive = value === 'enabled' },
})

const allModels = computed<ModelOption[]>(() => {
  const byId = new Map<string, ModelOption>()
  for (const id of providerModelIds.value) {
    const normalized = id.trim()
    if (normalized) byId.set(normalized, { id: normalized, source: '提供商' })
  }
  for (const model of upstreamModels.value) {
    const normalized = model.id?.trim()
    if (normalized && !byId.has(normalized)) {
      byId.set(normalized, { id: normalized, source: '上游' })
    }
  }
  for (const id of form.selectedModels) {
    const normalized = id.trim()
    if (normalized && !byId.has(normalized)) {
      byId.set(normalized, { id: normalized, source: '自定义' })
    }
  }
  return [...byId.values()].sort((a, b) => a.id.localeCompare(b.id))
})

const filteredModels = computed(() => {
  const search = normalizedModelSearch.value.toLowerCase()
  if (!search) return allModels.value
  return allModels.value.filter(model => model.id.toLowerCase().includes(search))
})
const canAddCustomModel = computed(() => {
  const model = normalizedModelSearch.value
  return Boolean(model) && !allModels.value.some(item => item.id === model)
})
const areFilteredModelsSelected = computed(() => (
  filteredModels.value.length > 0
  && filteredModels.value.every(model => form.selectedModels.includes(model.id))
))

function resetDialog(): void {
  Object.assign(form, createInitialForm())
  activeTab.value = 'configuration'
  modelSearch.value = ''
  providerModelIds.value = []
  upstreamModels.value = []
}

function toggleApiFormat(format: string, checked: boolean): void {
  const next = new Set(form.apiFormats)
  if (checked) next.add(format)
  else next.delete(format)
  form.apiFormats = [...next]
}

function toggleModel(modelId: string, checked: boolean): void {
  const next = new Set(form.selectedModels)
  if (checked) next.add(modelId)
  else next.delete(modelId)
  form.selectedModels = [...next]
}

function toggleFilteredModels(): void {
  const next = new Set(form.selectedModels)
  const select = !areFilteredModelsSelected.value
  for (const model of filteredModels.value) {
    if (select) next.add(model.id)
    else next.delete(model.id)
  }
  form.selectedModels = [...next]
}

function addCustomModel(): void {
  const model = normalizedModelSearch.value
  if (!model) return
  toggleModel(model, true)
  modelSearch.value = ''
}

async function loadProviderModels(): Promise<void> {
  if (!props.providerId) return
  loadingProviderModels.value = true
  try {
    const models = await getProviderModels(props.providerId, { limit: 1000 })
    providerModelIds.value = models
      .map(model => model.provider_model_name?.trim())
      .filter((model): model is string => Boolean(model))
  } catch (err) {
    showError(parseApiError(err, '加载提供商模型失败'))
  } finally {
    loadingProviderModels.value = false
  }
}

async function fetchUpstreamModels(forceRefresh = false): Promise<void> {
  if (!props.providerId || props.keyIds.length === 0) return
  fetchingUpstreamModels.value = true
  try {
    const result = await fetchModelsForKeys(props.providerId, props.keyIds, forceRefresh)
    if (result.error) {
      warning(result.error)
      return
    }
    upstreamModels.value = result.models
    if (result.warning) warning(result.warning)
    else success(`已获取 ${result.models.length} 个上游模型`)
  } finally {
    fetchingUpstreamModels.value = false
  }
}

function handleDialogUpdate(value: boolean): void {
  if (!value && !saving.value) closeDialog()
}

function closeDialog(): void {
  if (saving.value) return
  emit('close')
}

async function saveChanges(): Promise<void> {
  if (saving.value) return
  if (props.keyIds.length === 0) {
    warning('请选择要编辑的密钥')
    return
  }
  const build = buildPoolKeyBatchUpdatePatch(form)
  if (!build.patch || build.error) {
    warning(build.error || '批量配置无效')
    if (form.applyAllowedModels && build.error?.includes('模型')) activeTab.value = 'models'
    return
  }

  const confirmed = await confirm({
    title: '应用批量配置',
    message: `将对 ${props.keyIds.length} 个密钥修改：${build.fieldLabels.join('、')}。是否继续？`,
    confirmText: '确认应用',
  })
  if (!confirmed) return

  saving.value = true
  try {
    const result = await batchUpdatePoolKeys(props.providerId, {
      key_ids: props.keyIds,
      patch: build.patch,
    })
    const modelSync = result.model_sync
    if (modelSync?.failed) {
      warning(`已更新 ${result.affected} 个密钥，${modelSync.failed} 个账号的模型同步失败`)
    } else if (modelSync && modelSync.attempted < modelSync.requested) {
      warning(`已更新 ${result.affected} 个密钥，部分账号未执行即时模型同步`)
    } else {
      success(result.message)
    }
    emit('saved')
    emit('close')
  } catch (err) {
    showError(parseApiError(err, '批量更新密钥失败'))
  } finally {
    saving.value = false
  }
}

watch(
  () => props.open,
  open => {
    if (!open) return
    resetDialog()
    void loadProviderModels()
  },
)
</script>
