<template>
  <Dialog
    :model-value="isOpen"
    title="编辑账号"
    description="修改 OAuth 账号配置"
    :icon="SquarePen"
    size="xl"
    @update:model-value="handleDialogUpdate"
  >
    <form
      class="space-y-3"
      autocomplete="off"
      @submit.prevent="handleSave"
    >
      <!-- 基本信息：账号名称 + 备注 -->
      <div class="grid grid-cols-2 gap-3">
        <div>
          <Label for="name">账号名称 *</Label>
          <Input
            id="name"
            v-model="form.name"
            required
            placeholder="例如：主账号、备用账号"
            maxlength="100"
            autocomplete="off"
          />
        </div>
        <div>
          <Label for="note">备注</Label>
          <Input
            id="note"
            v-model="form.note"
            placeholder="可选的备注信息"
          />
        </div>
      </div>

      <!-- 配置项 -->
      <div class="grid grid-cols-4 gap-3">
        <div>
          <Label
            for="internal_priority"
            class="text-xs"
          >优先级</Label>
          <Input
            id="internal_priority"
            v-model.number="form.internal_priority"
            type="number"
            min="0"
            placeholder="10"
            class="h-8"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            越小越优先
          </p>
        </div>
        <div>
          <Label
            for="rpm_limit"
            class="text-xs"
          >RPM 限制</Label>
          <Input
            id="rpm_limit"
            :model-value="form.rpm_limit ?? ''"
            type="number"
            min="1"
            max="10000"
            placeholder="自适应"
            class="h-8"
            @update:model-value="(v) => form.rpm_limit = parseNullableNumberInput(v, { min: 1, max: 10000 })"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            留空自适应
          </p>
        </div>
        <div>
          <Label
            for="cache_ttl_minutes"
            class="text-xs"
          >缓存 TTL</Label>
          <Input
            id="cache_ttl_minutes"
            :model-value="form.cache_ttl_minutes ?? ''"
            type="number"
            min="0"
            max="60"
            class="h-8"
            @update:model-value="(v) => form.cache_ttl_minutes = parseNumberInput(v, { min: 0, max: 60 }) ?? 5"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            分钟，0禁用
          </p>
        </div>
        <div>
          <Label
            for="max_probe_interval_minutes"
            class="text-xs"
          >熔断探测</Label>
          <Input
            id="max_probe_interval_minutes"
            :model-value="form.max_probe_interval_minutes ?? ''"
            type="number"
            min="0"
            max="32"
            placeholder="32"
            class="h-8"
            @update:model-value="(v) => form.max_probe_interval_minutes = parseNumberInput(v, { min: 0, max: 32 }) ?? 32"
          />
          <p class="text-xs text-muted-foreground mt-0.5">
            0-32分钟
          </p>
        </div>
      </div>

      <!-- 自动获取模型 -->
      <div class="space-y-3 py-2 px-3 rounded-md border border-border/60 bg-muted/30">
        <div class="flex items-center justify-between">
          <div class="space-y-0.5">
            <Label class="text-sm font-medium">自动获取上游可用模型</Label>
            <p class="text-xs text-muted-foreground">
              定时更新上游模型, 配合模型映射使用
            </p>
            <p
              v-if="showAutoFetchWarning"
              class="text-xs text-amber-600 dark:text-amber-400"
            >
              {{ autoFetchWarningMessage }}
            </p>
          </div>
          <Switch v-model="form.auto_fetch_models" />
        </div>

        <!-- 模型过滤规则（仅当开启自动获取时显示） -->
        <div
          v-if="form.auto_fetch_models"
          class="space-y-2 pt-2 border-t border-border/40"
        >
          <div class="grid grid-cols-2 gap-3">
            <div>
              <Label class="text-xs">包含规则</Label>
              <Input
                v-model="form.model_include_patterns_text"
                placeholder="gpt-*, claude-*, 留空包含全部"
                class="h-8 text-sm"
              />
            </div>
            <div>
              <Label class="text-xs">排除规则</Label>
              <Input
                v-model="form.model_exclude_patterns_text"
                placeholder="*-preview, *-beta"
                class="h-8 text-sm"
              />
            </div>
          </div>
          <p class="text-xs text-muted-foreground">
            逗号分隔，支持 * ? 通配符，不区分大小写
          </p>
        </div>
      </div>
    </form>

    <template #footer>
      <Button
        variant="outline"
        @click="handleCancel"
      >
        取消
      </Button>
      <Button
        :disabled="saving || !canSave"
        @click="handleSave"
      >
        {{ saving ? '保存中...' : '保存' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { Dialog, Button, Input, Label, Switch } from '@/components/ui'
import { SquarePen } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useFormDialog } from '@/composables/useFormDialog'
import { parseApiError } from '@/utils/errorParser'
import { parseNumberInput, parseNullableNumberInput } from '@/utils/form'
import {
  updateProviderKey,
  type EndpointAPIKey,
  type EndpointAPIKeyUpdate,
} from '@/api/endpoints'

const props = defineProps<{
  open: boolean
  editingKey: EndpointAPIKey | null
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, error: showError } = useToast()
const { confirmWarning } = useConfirm()

// 显示自动获取模型警告：编辑模式下，原本未启用但现在启用，且已有 allowed_models
const showAutoFetchWarning = computed(() => {
  if (!props.editingKey) return false
  // 原本已启用，不需要警告
  if (props.editingKey.auto_fetch_models) return false
  // 现在未启用，不需要警告
  if (!form.value.auto_fetch_models) return false
  // 检查是否有已配置的模型权限
  const allowedModels = props.editingKey.allowed_models
  if (!allowedModels) return false
  if (Array.isArray(allowedModels) && allowedModels.length === 0) return false
  if (typeof allowedModels === 'object' && Object.keys(allowedModels).length === 0) return false
  return true
})

const autoFetchWarningMessage = computed(() => {
  if (!showAutoFetchWarning.value || !props.editingKey?.allowed_models) return ''
  const models = Array.isArray(props.editingKey.allowed_models)
    ? props.editingKey.allowed_models
    : []
  if (models.length === 0) return ''
  return `当前 Key 模型权限存在以下模型：${models.map(model => `“${model}”`).join('、')}，开启自动获取后将被覆盖`
})

// 表单是否可以保存
const canSave = computed(() => {
  // 必须填写名称
  if (!form.value.name.trim()) return false
  return true
})

const isOpen = computed(() => props.open)
const saving = ref(false)

const form = ref({
  name: '',
  internal_priority: 10,
  rpm_limit: undefined as number | null | undefined,
  cache_ttl_minutes: 5,
  max_probe_interval_minutes: 32,
  note: '',
  auto_fetch_models: false,
  model_include_patterns_text: '',
  model_exclude_patterns_text: ''
})

// ---------------------------------------------------------------------------
// Dirty 状态跟踪：通过快照比较判断表单是否被修改
// ---------------------------------------------------------------------------
const formSnapshot = ref('')

function takeSnapshot() {
  formSnapshot.value = JSON.stringify(form.value)
}

const isDirty = computed(() => {
  if (!formSnapshot.value) return false
  return JSON.stringify(form.value) !== formSnapshot.value
})

// 对话框关闭时清除快照（快照在 loadKeyData 中拍摄）
watch(isOpen, (val) => {
  if (!val) {
    formSnapshot.value = ''
  }
})

// 重置表单
function resetForm() {
  form.value = {
    name: '',
    internal_priority: 10,
    rpm_limit: undefined,
    cache_ttl_minutes: 5,
    max_probe_interval_minutes: 32,
    note: '',
    auto_fetch_models: false,
    model_include_patterns_text: '',
    model_exclude_patterns_text: ''
  }
  formSnapshot.value = ''
}

// 加载密钥数据
function loadKeyData() {
  if (!props.editingKey) return
  form.value = {
    name: props.editingKey.name,
    internal_priority: props.editingKey.internal_priority ?? 10,
    rpm_limit: props.editingKey.rpm_limit ?? undefined,
    cache_ttl_minutes: props.editingKey.cache_ttl_minutes ?? 5,
    max_probe_interval_minutes: props.editingKey.max_probe_interval_minutes ?? 32,
    note: props.editingKey.note || '',
    auto_fetch_models: props.editingKey.auto_fetch_models ?? false,
    model_include_patterns_text: (props.editingKey.model_include_patterns || []).join(', '),
    model_exclude_patterns_text: (props.editingKey.model_exclude_patterns || []).join(', ')
  }
  // 数据加载完成后拍快照，作为 dirty 判断的基准
  takeSnapshot()
}

// 使用 useFormDialog 统一处理对话框逻辑
const {
  handleDialogUpdate: _baseHandleDialogUpdate,
  handleCancel: _baseHandleCancel,
} = useFormDialog({
  isOpen: () => props.open,
  entity: () => props.editingKey,
  isLoading: saving,
  onClose: () => emit('close'),
  loadData: loadKeyData,
  resetForm,
})

// 包装关闭逻辑：有未保存更改时弹出确认
async function handleDialogUpdate(value: boolean) {
  if (!value && isDirty.value) {
    const confirmed = await confirmWarning('有未保存的更改，确定要关闭吗？', '放弃更改')
    if (!confirmed) return
  }
  _baseHandleDialogUpdate(value)
}

async function handleCancel() {
  if (isDirty.value) {
    const confirmed = await confirmWarning('有未保存的更改，确定要关闭吗？', '放弃更改')
    if (!confirmed) return
  }
  _baseHandleCancel()
}

// 将逗号分隔的文本解析为数组（去空、去重）
function parsePatternText(text: string): string[] {
  if (!text.trim()) return []
  const patterns = text
    .split(',')
    .map(s => s.trim())
    .filter(s => s.length > 0)
  return [...new Set(patterns)]
}

async function handleSave() {
  if (!props.editingKey) {
    showError('无法保存：缺少账号信息', '错误')
    return
  }

  saving.value = true
  try {
    const shouldClearAllowedModels = !!props.editingKey.auto_fetch_models && !form.value.auto_fetch_models
    const updateData: EndpointAPIKeyUpdate = {
      name: form.value.name,
      internal_priority: form.value.internal_priority,
      rpm_limit: form.value.rpm_limit,
      cache_ttl_minutes: form.value.cache_ttl_minutes,
      max_probe_interval_minutes: form.value.max_probe_interval_minutes,
      note: form.value.note,
      allowed_models: shouldClearAllowedModels ? null : undefined,
      auto_fetch_models: form.value.auto_fetch_models,
      model_include_patterns: parsePatternText(form.value.model_include_patterns_text),
      model_exclude_patterns: parsePatternText(form.value.model_exclude_patterns_text)
    }

    await updateProviderKey(props.editingKey.id, updateData)
    success('账号已更新', '成功')
    emit('saved')
    emit('close')
  } catch (err: unknown) {
    const errorMessage = parseApiError(err, '保存失败')
    showError(errorMessage, '错误')
  } finally {
    saving.value = false
  }
}
</script>
