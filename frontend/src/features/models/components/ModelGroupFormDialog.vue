<template>
  <Dialog
    :model-value="open"
    size="3xl"
    @update:model-value="handleDialogUpdate"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg bg-primary/10">
            <Layers3 class="h-5 w-5 text-primary" />
          </div>
          <div class="min-w-0 flex-1">
            <h3 class="text-lg font-semibold leading-tight text-foreground">
              {{ isEditMode ? '编辑模型分组' : '新增模型分组' }}
            </h3>
            <p class="text-xs text-muted-foreground">
              统一管理模型成员、路由策略和用户计费倍率
            </p>
          </div>
        </div>
      </div>
    </template>

    <form @submit.prevent="handleSubmit">
      <div class="grid grid-cols-2 gap-0">
        <div class="space-y-4 pr-6">
          <div class="flex items-center gap-2 border-b border-border/60 pb-2">
            <span class="text-sm font-medium">基本信息</span>
          </div>

          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-2">
              <Label class="text-sm font-medium">分组标识</Label>
              <Input
                v-model="form.name"
                :disabled="isEditMode"
                placeholder="如 premium"
                class="h-10"
              />
            </div>
            <div class="space-y-2">
              <Label class="text-sm font-medium">显示名称</Label>
              <Input
                v-model="form.display_name"
                placeholder="如 高级模型组"
                class="h-10"
              />
            </div>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">描述</Label>
            <Textarea
              v-model="form.description"
              rows="5"
              placeholder="描述这个模型分组面向什么用户、承载什么路由策略"
            />
          </div>

          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-2">
              <Label class="text-sm font-medium">路由模式</Label>
              <Select v-model="form.routing_mode">
                <SelectTrigger class="h-10">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="inherit">
                    继承全局路由
                  </SelectItem>
                  <SelectItem value="custom">
                    自定义渠道路由
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div class="space-y-2">
              <Label class="text-sm font-medium">默认用户计费倍率</Label>
              <Input
                :model-value="form.default_user_billing_multiplier"
                type="number"
                step="0.01"
                min="0"
                class="h-10"
                @update:model-value="(v) => form.default_user_billing_multiplier = parseNumberInput(v, { allowFloat: true, min: 0 }) ?? 1"
              />
            </div>
          </div>

          <div class="grid grid-cols-2 gap-3">
            <div class="space-y-2">
              <Label class="text-sm font-medium">排序</Label>
              <Input
                :model-value="form.sort_order"
                type="number"
                min="0"
                class="h-10"
                @update:model-value="(v) => form.sort_order = parseNumberInput(v, { min: 0 }) ?? 100"
              />
            </div>
            <div class="space-y-2">
              <Label class="text-sm font-medium">分组状态</Label>
              <div class="flex h-10 items-center justify-between rounded-lg border border-border/60 px-3">
                <span class="text-sm text-muted-foreground">
                  {{ form.is_active ? '已启用' : '已停用' }}
                </span>
                <Switch
                  v-model="form.is_active"
                  class="shrink-0"
                />
              </div>
              <p class="text-xs text-muted-foreground">
                停用后不会参与用户路由匹配
              </p>
            </div>
          </div>

          <div class="space-y-2">
            <Label class="text-sm font-medium">包含模型</Label>
            <MultiSelect
              v-model="form.model_ids"
              :options="modelOptions"
              :search-threshold="0"
              placeholder="选择属于该分组的统一模型"
              empty-text="暂无可用模型"
              no-results-text="未找到匹配的模型"
              search-placeholder="搜索模型..."
            />
            <p class="text-xs text-muted-foreground">
              一个统一模型可以同时属于多个模型分组。
            </p>
          </div>
        </div>

        <div class="space-y-4 border-l border-border pl-6">
          <div class="flex items-center justify-between gap-2 border-b border-border/60 pb-2">
            <span class="text-sm font-medium">渠道路由</span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              class="h-8 px-3"
              @click="addRoute"
            >
              <Plus class="mr-1.5 h-3.5 w-3.5" />
              添加规则
            </Button>
          </div>

          <div
            v-if="form.routing_mode !== 'custom'"
            class="rounded-lg border border-dashed border-border/60 px-3 py-4 text-xs text-muted-foreground"
          >
            当前使用“继承全局路由”，下面的渠道规则不会参与调度。切换为“自定义渠道路由”后，系统只会在这里配置的 Provider / Key 里选路。
          </div>

          <div
            v-if="form.routes.length === 0"
            class="rounded-lg border border-dashed border-border/60 px-3 py-4 text-xs text-muted-foreground"
          >
            还没有配置渠道规则。
          </div>

          <div
            v-else
            class="space-y-3"
          >
            <div
              v-for="(route, index) in form.routes"
              :key="route.key"
              class="rounded-lg border border-border/60 bg-muted/15 p-3"
            >
              <div class="mb-3 flex items-center justify-between gap-2">
                <div class="flex items-center gap-2">
                  <Badge
                    variant="secondary"
                    class="h-5 px-1.5 py-0 text-[10px]"
                  >
                    #{{ index + 1 }}
                  </Badge>
                  <span class="text-sm font-medium text-foreground">渠道规则</span>
                </div>
                <div class="flex items-center gap-2">
                  <Switch v-model="route.is_active" />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8 text-rose-600 hover:bg-rose-50 hover:text-rose-600 dark:hover:bg-rose-950/30"
                    @click="removeRoute(index)"
                  >
                    <Trash2 class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </div>

              <div class="grid grid-cols-2 gap-3">
                <div class="space-y-2">
                  <Label class="text-xs font-medium">Provider</Label>
                  <Select
                    :model-value="route.provider_id"
                    @update:model-value="(value) => handleRouteProviderChange(route, String(value ?? ''))"
                  >
                    <SelectTrigger class="h-9">
                      <SelectValue placeholder="选择 Provider" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem
                        v-for="provider in providerOptions"
                        :key="provider.value"
                        :value="provider.value"
                      >
                        {{ provider.label }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div class="space-y-2">
                  <Label class="text-xs font-medium">供应商 Key</Label>
                  <Select
                    v-model="route.provider_api_key_id"
                    :disabled="!route.provider_id"
                  >
                    <SelectTrigger class="h-9">
                      <SelectValue :placeholder="route.provider_id ? '选择供应商 Key' : '先选择 Provider'" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem
                        v-for="option in getProviderKeyOptions(route.provider_id)"
                        :key="option.value"
                        :value="option.value"
                      >
                        {{ option.label }}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div class="space-y-2">
                  <Label class="text-xs font-medium">优先级</Label>
                  <Input
                    :model-value="route.priority"
                    type="number"
                    min="0"
                    class="h-9"
                    @update:model-value="(v) => route.priority = parseNumberInput(v, { min: 0 }) ?? 50"
                  />
                </div>

                <div class="space-y-2">
                  <Label class="text-xs font-medium">计费倍率覆盖</Label>
                  <Input
                    :model-value="route.user_billing_multiplier_override ?? ''"
                    type="number"
                    step="0.01"
                    min="0"
                    class="h-9"
                    placeholder="留空 = 使用分组默认倍率"
                    @update:model-value="(v) => route.user_billing_multiplier_override = parseNumberInput(v, { allowFloat: true, min: 0 }) ?? null"
                  />
                </div>

                <div class="col-span-2 space-y-2">
                  <Label class="text-xs font-medium">备注</Label>
                  <Input
                    v-model="route.notes"
                    class="h-9"
                    placeholder="可选，说明这条渠道规则的用途"
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
    </form>

    <template #footer>
      <Button
        variant="outline"
        type="button"
        class="h-10 px-5"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        class="h-10 px-5"
        :disabled="saving || !isFormValid"
        @click="handleSubmit"
      >
        {{ saving ? '处理中...' : isEditMode ? '更新' : '创建' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Layers3, Plus, Trash2 } from 'lucide-vue-next'
import {
  Badge,
  Button,
  Dialog,
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
import { MultiSelect } from '@/components/common'
import { useFormDialog } from '@/composables/useFormDialog'
import { getProvidersSummary } from '@/api/endpoints/providers'
import { getProviderKeys, type EndpointAPIKey } from '@/api/endpoints/keys'
import { getGlobalModels } from '@/api/global-models'
import {
  type ModelGroupDetail,
  type ModelGroupRoute,
  type UpsertModelGroupRequest,
} from '@/api/model-groups'
import type { ProviderWithEndpointsSummary, GlobalModelResponse } from '@/api/endpoints/types'
import { log } from '@/utils/logger'
import { parseNumberInput } from '@/utils/form'

interface RouteFormItem {
  key: string
  provider_id: string
  provider_api_key_id: string
  priority: number
  user_billing_multiplier_override: number | null
  is_active: boolean
  notes: string
}

export interface ModelGroupFormData extends UpsertModelGroupRequest {
  id?: string
}

const props = defineProps<{
  open: boolean
  group: ModelGroupDetail | null
}>()

const emit = defineEmits<{
  close: []
  submit: [data: ModelGroupFormData]
}>()

const saving = ref(false)
const providers = ref<ProviderWithEndpointsSummary[]>([])
const globalModels = ref<GlobalModelResponse[]>([])
const providerKeysByProviderId = ref<Record<string, EndpointAPIKey[]>>({})

const providerOptions = computed(() =>
  providers.value.map((provider) => ({
    value: provider.id,
    label: provider.name,
  })),
)

const modelOptions = computed(() =>
  globalModels.value.map((model) => ({
    value: model.id,
    label: `${model.display_name} · ${model.name}`,
  })),
)

const form = ref({
  name: '',
  display_name: '',
  description: '',
  default_user_billing_multiplier: 1,
  routing_mode: 'inherit' as 'inherit' | 'custom',
  is_active: true,
  sort_order: 100,
  model_ids: [] as string[],
  routes: [] as RouteFormItem[],
})

function createRouteKey(): string {
  return Math.random().toString(36).slice(2, 10)
}

function toRouteFormItem(route?: ModelGroupRoute): RouteFormItem {
  return {
    key: createRouteKey(),
    provider_id: route?.provider_id ?? '',
    provider_api_key_id: route?.provider_api_key_id ?? '__all__',
    priority: route?.priority ?? 50,
    user_billing_multiplier_override: route?.user_billing_multiplier_override ?? null,
    is_active: route?.is_active ?? true,
    notes: route?.notes ?? '',
  }
}

function resetForm() {
  form.value = {
    name: '',
    display_name: '',
    description: '',
    default_user_billing_multiplier: 1,
    routing_mode: 'inherit',
    is_active: true,
    sort_order: 100,
    model_ids: [],
    routes: [],
  }
}

function loadGroupData() {
  if (!props.group) {
    resetForm()
    return
  }
  form.value = {
    name: props.group.name,
    display_name: props.group.display_name,
    description: props.group.description ?? '',
    default_user_billing_multiplier: props.group.default_user_billing_multiplier ?? 1,
    routing_mode: props.group.routing_mode,
    is_active: props.group.is_active,
    sort_order: props.group.sort_order ?? 100,
    model_ids: props.group.models.map((item) => item.global_model_id),
    routes: props.group.routes.map((route) => toRouteFormItem(route)),
  }
}

const { isEditMode, handleDialogUpdate, handleCancel } = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.group,
  isLoading: saving,
  onClose: () => emit('close'),
  loadData: loadGroupData,
  resetForm,
})

const nameError = computed(() => {
  if (!form.value.name.trim()) return '分组标识不能为空'
  if (!/^[a-zA-Z0-9_.-]+$/.test(form.value.name.trim())) {
    return '分组标识仅支持字母、数字、点、下划线和连字符'
  }
  return ''
})

const displayNameError = computed(() => {
  return form.value.display_name.trim() ? '' : '显示名称不能为空'
})

const routeError = computed(() => {
  if (form.value.routing_mode !== 'custom') return ''
  const invalidRoute = form.value.routes.find((route) => !route.provider_id.trim())
  if (invalidRoute) return '自定义路由模式下，所有渠道规则都必须选择 Provider'
  return ''
})

const isFormValid = computed(() => !nameError.value && !displayNameError.value && !routeError.value)

function addRoute() {
  form.value.routes.push(toRouteFormItem())
}

function removeRoute(index: number) {
  form.value.routes.splice(index, 1)
}

function getProviderKeyOptions(providerId: string) {
  const options = [{ value: '__all__', label: '全部 Key' }]
  const keys = providerKeysByProviderId.value[providerId] ?? []
  return [
    ...options,
    ...keys.map((key) => ({
      value: key.id,
      label: `${key.name}${key.api_key_masked ? ` · ${key.api_key_masked}` : ''}${key.is_active ? '' : ' · 已停用'}`,
    })),
  ]
}

async function ensureProviderKeysLoaded(providerIds: string[]) {
  const normalizedIds = Array.from(
    new Set(providerIds.map(providerId => String(providerId ?? '').trim()).filter(Boolean)),
  ).filter(providerId => !providerKeysByProviderId.value[providerId])

  if (!normalizedIds.length) return

  await Promise.all(
    normalizedIds.map(async (providerId) => {
      try {
        providerKeysByProviderId.value[providerId] = await getProviderKeys(providerId)
      } catch (err) {
        providerKeysByProviderId.value[providerId] = []
        log.error(`加载 Provider ${providerId} 的 Key 列表失败:`, err)
      }
    }),
  )
}

function handleRouteProviderChange(route: RouteFormItem, providerId: string) {
  route.provider_id = providerId
  const availableKeys = providerKeysByProviderId.value[providerId] ?? []
  if (!availableKeys.some(key => key.id === route.provider_api_key_id)) {
    route.provider_api_key_id = '__all__'
  }
  if (providerId) {
    void ensureProviderKeysLoaded([providerId])
  }
}

async function loadOptions() {
  try {
    const [providersResponse, globalModelsResponse] = await Promise.all([
      getProvidersSummary({ page_size: 9999 }),
      getGlobalModels({ limit: 1000 }),
    ])
    providers.value = providersResponse.items
    globalModels.value = globalModelsResponse.models || []
    await ensureProviderKeysLoaded(form.value.routes.map(route => route.provider_id))
  } catch (err) {
    log.error('加载模型分组选项失败:', err)
  }
}

async function handleSubmit() {
  if (!isFormValid.value) return
  saving.value = true
  try {
    emit('submit', {
      id: props.group?.id,
      name: form.value.name.trim(),
      display_name: form.value.display_name.trim(),
      description: form.value.description.trim() || null,
      default_user_billing_multiplier: form.value.default_user_billing_multiplier,
      routing_mode: form.value.routing_mode,
      is_active: form.value.is_active,
      sort_order: form.value.sort_order,
      model_ids: [...form.value.model_ids],
      routes: form.value.routes
        .filter((route) => route.provider_id.trim())
        .map((route) => ({
          provider_id: route.provider_id.trim(),
          provider_api_key_id: route.provider_api_key_id === '__all__'
            ? null
            : route.provider_api_key_id,
          priority: route.priority,
          user_billing_multiplier_override: route.user_billing_multiplier_override,
          is_active: route.is_active,
          notes: route.notes.trim() || null,
        })),
    })
  } finally {
    saving.value = false
  }
}

function setSaving(value: boolean) {
  saving.value = value
}

watch(
  () => props.open,
  (value) => {
    if (value) {
      void loadOptions()
    }
  },
)

watch(
  () => form.value.routes.map(route => route.provider_id).join('|'),
  (value) => {
    if (!props.open || !value) return
    void ensureProviderKeysLoaded(form.value.routes.map(route => route.provider_id))
  },
)

defineExpose({
  setSaving,
})
</script>
