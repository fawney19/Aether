<template>
  <Card class="overflow-hidden">
    <!-- 标题头部 -->
    <div class="p-4 border-b border-border/60">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-semibold flex items-center gap-2">
          模型名称映射
        </h3>
        <Button
          variant="outline"
          size="sm"
          class="h-8"
          @click="openAddDialog"
        >
          <Plus class="w-3.5 h-3.5 mr-1.5" />
          添加映射
        </Button>
      </div>
    </div>

    <!-- 加载状态 -->
    <div
      v-if="loading"
      class="flex items-center justify-center py-12"
    >
      <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
    </div>

    <!-- 分组映射列表 -->
    <div
      v-else-if="aliasGroups.length > 0"
      class="divide-y divide-border/40"
    >
      <div
        v-for="group in aliasGroups"
        :key="`${group.model.id}-${group.apiFormatsKey}`"
        class="transition-colors"
      >
        <!-- 分组头部（可点击展开） -->
        <div
          class="flex items-center justify-between px-4 py-3 hover:bg-muted/20 cursor-pointer"
          @click="toggleAliasGroupExpand(`${group.model.id}-${group.apiFormatsKey}`)"
        >
          <div class="flex items-center gap-2 flex-1 min-w-0">
            <!-- 展开/收起图标 -->
            <ChevronRight
              class="w-4 h-4 text-muted-foreground shrink-0 transition-transform"
              :class="{ 'rotate-90': expandedAliasGroups.has(`${group.model.id}-${group.apiFormatsKey}`) }"
            />
            <!-- 模型名称 -->
            <span class="font-semibold text-sm truncate">
              {{ group.model.global_model_display_name || group.model.provider_model_name }}
            </span>
            <!-- 作用域标签 -->
            <div class="flex items-center gap-1 shrink-0">
              <Badge
                v-if="group.apiFormats.length === 0"
                variant="outline"
                class="text-xs"
              >
                全部
              </Badge>
              <Badge
                v-for="format in group.apiFormats"
                v-else
                :key="format"
                variant="outline"
                class="text-xs"
              >
                {{ API_FORMAT_LABELS[format] || format }}
              </Badge>
            </div>
            <!-- 映射数量 -->
            <span class="text-xs text-muted-foreground shrink-0">
              ({{ group.aliases.length }} 个映射)
            </span>
          </div>
          <!-- 操作按钮 -->
          <div
            class="flex items-center gap-1.5 ml-4 shrink-0"
            @click.stop
          >
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="编辑映射组"
              @click="editGroup(group)"
            >
              <Edit class="w-3.5 h-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8 text-destructive hover:text-destructive"
              title="删除映射组"
              @click="deleteGroup(group)"
            >
              <Trash2 class="w-3.5 h-3.5" />
            </Button>
          </div>
        </div>

        <!-- 展开的映射列表 -->
        <div
          v-show="expandedAliasGroups.has(`${group.model.id}-${group.apiFormatsKey}`)"
          class="bg-muted/30 border-t border-border/30"
        >
          <div class="px-4 py-2 space-y-1">
            <div
              v-for="mapping in group.aliases"
              :key="mapping.name"
              class="flex items-center justify-between gap-2 py-1"
            >
              <div class="flex items-center gap-2 flex-1 min-w-0">
                <!-- 优先级标签 -->
                <span class="inline-flex items-center justify-center w-5 h-5 rounded bg-background border text-xs font-medium shrink-0">
                  {{ mapping.priority }}
                </span>
                <!-- 映射名称 -->
                <span class="font-mono text-sm truncate">
                  {{ mapping.name }}
                </span>
              </div>
              <!-- 测试按钮 -->
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0"
                title="测试映射"
                :disabled="testingMapping === `${group.model.id}-${group.apiFormatsKey}-${mapping.name}`"
                @click="testMapping(group, mapping)"
              >
                <Loader2 v-if="testingMapping === `${group.model.id}-${group.apiFormatsKey}-${mapping.name}`" class="w-3 h-3 animate-spin" />
                <Play v-else class="w-3 h-3" />
              </Button>
            </div>
          </div>
        </div>
      </div>
    </div>

    <!-- 空状态 -->
    <div
      v-else
      class="p-8 text-center text-muted-foreground"
    >
      <Tag class="w-12 h-12 mx-auto mb-3 opacity-50" />
      <p class="text-sm">
        暂无模型映射
      </p>
      <p class="text-xs mt-1">
        点击上方"添加映射"按钮为模型创建名称映射
      </p>
    </div>
  </Card>

  <!-- 添加/编辑映射对话框 -->
  <ModelMappingDialog
    v-model:open="dialogOpen"
    :provider-id="provider.id"
    :provider-api-formats="providerApiFormats"
    :models="models"
    :editing-group="editingGroup"
    @saved="onDialogSaved"
  />

  <!-- 删除确认对话框 -->
  <AlertDialog
    v-model="deleteConfirmOpen"
    title="删除映射组"
    :description="deleteConfirmDescription"
    confirm-text="删除"
    cancel-text="取消"
    type="danger"
    @confirm="confirmDelete"
    @cancel="deleteConfirmOpen = false"
  />
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { Tag, Plus, Edit, Trash2, ChevronRight, Loader2, Play } from 'lucide-vue-next'
import { Card, Button, Badge } from '@/components/ui'
import AlertDialog from '@/components/common/AlertDialog.vue'
import ModelMappingDialog, { type AliasGroup } from '../ModelMappingDialog.vue'
import { useToast } from '@/composables/useToast'
import {
  getProviderModels,
  testModel,
  API_FORMAT_LABELS,
  type Model,
  type ProviderModelAlias
} from '@/api/endpoints'
import { updateModel } from '@/api/endpoints/models'
import { parseTestModelError } from '@/utils/errorParser'

const props = defineProps<{
  provider: any
}>()

const emit = defineEmits<{
  'refresh': []
}>()

const { error: showError, success: showSuccess } = useToast()

// 状态
const loading = ref(false)
const models = ref<Model[]>([])
const dialogOpen = ref(false)
const deleteConfirmOpen = ref(false)
const editingGroup = ref<AliasGroup | null>(null)
const deletingGroup = ref<AliasGroup | null>(null)
const testingMapping = ref<string | null>(null)

// 列表展开状态
const expandedAliasGroups = ref<Set<string>>(new Set())

// 获取 Provider 支持的 API 格式
const providerApiFormats = computed(() => {
  const formats = props.provider?.api_formats
  if (Array.isArray(formats) && formats.length > 0) {
    const order = Object.keys(API_FORMAT_LABELS)
    return [...formats].sort((a, b) => order.indexOf(a) - order.indexOf(b))
  }
  return []
})

// 生成作用域唯一键
function getApiFormatsKey(formats: string[] | undefined): string {
  if (!formats || formats.length === 0) return ''
  return [...formats].sort().join(',')
}

// 按"模型+作用域"分组的映射列表
const aliasGroups = computed<AliasGroup[]>(() => {
  const groups: AliasGroup[] = []
  const groupMap = new Map<string, AliasGroup>()

  for (const model of models.value) {
    if (!model.provider_model_mappings || !Array.isArray(model.provider_model_mappings)) continue

    for (const alias of model.provider_model_mappings) {
      const apiFormatsKey = getApiFormatsKey(alias.api_formats)
      const groupKey = `${model.id}|${apiFormatsKey}`

      if (!groupMap.has(groupKey)) {
        const group: AliasGroup = {
          model,
          apiFormatsKey,
          apiFormats: alias.api_formats || [],
          aliases: []
        }
        groupMap.set(groupKey, group)
        groups.push(group)
      }
      groupMap.get(groupKey)!.aliases.push(alias)
    }
  }

  for (const group of groups) {
    group.aliases.sort((a, b) => a.priority - b.priority)
  }

  return groups.sort((a, b) => {
    const nameA = (a.model.global_model_display_name || a.model.provider_model_name || '').toLowerCase()
    const nameB = (b.model.global_model_display_name || b.model.provider_model_name || '').toLowerCase()
    if (nameA !== nameB) return nameA.localeCompare(nameB)
    return a.apiFormatsKey.localeCompare(b.apiFormatsKey)
  })
})

// 加载模型
async function loadModels() {
  try {
    loading.value = true
    models.value = await getProviderModels(props.provider.id)
  } catch (err: any) {
    showError(err.response?.data?.detail || '加载失败', '错误')
  } finally {
    loading.value = false
  }
}

// 删除确认描述
const deleteConfirmDescription = computed(() => {
  if (!deletingGroup.value) return ''
  const { model, aliases, apiFormats } = deletingGroup.value
  const modelName = model.global_model_display_name || model.provider_model_name
  const scopeText = apiFormats.length === 0 ? '全部' : apiFormats.map(f => API_FORMAT_LABELS[f] || f).join(', ')
  const aliasNames = aliases.map(a => a.name).join(', ')
  return `确定要删除模型「${modelName}」在作用域「${scopeText}」下的 ${aliases.length} 个映射吗？\n\n映射名称：${aliasNames}`
})

// 切换映射组展开状态
function toggleAliasGroupExpand(groupKey: string) {
  if (expandedAliasGroups.value.has(groupKey)) {
    expandedAliasGroups.value.delete(groupKey)
  } else {
    expandedAliasGroups.value.add(groupKey)
  }
}

// 打开添加对话框
function openAddDialog() {
  editingGroup.value = null
  dialogOpen.value = true
}

// 编辑分组
function editGroup(group: AliasGroup) {
  editingGroup.value = group
  dialogOpen.value = true
}

// 删除分组
function deleteGroup(group: AliasGroup) {
  deletingGroup.value = group
  deleteConfirmOpen.value = true
}

// 确认删除
async function confirmDelete() {
  if (!deletingGroup.value) return

  const { model, aliases, apiFormatsKey } = deletingGroup.value

  try {
    const currentAliases = model.provider_model_mappings || []
    const aliasNamesToRemove = new Set(aliases.map(a => a.name))
    const newAliases = currentAliases.filter((a: ProviderModelAlias) => {
      const currentKey = getApiFormatsKey(a.api_formats)
      return !(currentKey === apiFormatsKey && aliasNamesToRemove.has(a.name))
    })

    await updateModel(props.provider.id, model.id, {
      provider_model_mappings: newAliases.length > 0 ? newAliases : null
    })

    showSuccess('映射组已删除')
    deleteConfirmOpen.value = false
    deletingGroup.value = null
    await loadModels()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '删除失败', '错误')
  }
}

// 对话框保存后回调
async function onDialogSaved() {
  await loadModels()
  emit('refresh')
}

// 测试模型映射
async function testMapping(group: any, mapping: any) {
  const testingKey = `${group.model.id}-${group.apiFormatsKey}-${mapping.name}`
  testingMapping.value = testingKey

  try {
    // 根据分组的 API 格式来确定应该使用的格式
    let apiFormat = null
    if (group.apiFormats.length === 1) {
      apiFormat = group.apiFormats[0]
    } else if (group.apiFormats.length === 0) {
      // 如果没有指定格式，但分组显示为"全部"，则使用模型的默认格式
      apiFormat = group.model.effective_api_format || group.model.api_format
    }

    const result = await testModel({
      provider_id: props.provider.id,
      model_name: mapping.name,  // 使用映射名称进行测试
      message: "hello",
      api_format: apiFormat
    })

    if (result.success) {
      showSuccess(`映射 "${mapping.name}" 测试成功`)

      // 如果有响应内容，可以显示更多信息
      if (result.data?.response?.choices?.[0]?.message?.content) {
        const content = result.data.response.choices[0].message.content
        showSuccess(`测试成功，响应: ${content.substring(0, 100)}${content.length > 100 ? '...' : ''}`)
      } else if (result.data?.content_preview) {
        showSuccess(`流式测试成功，预览: ${result.data.content_preview}`)
      }
    } else {
      showError(`映射测试失败: ${parseTestModelError(result)}`)
    }
  } catch (err: any) {
    const errorMsg = err.response?.data?.detail || err.message || '测试请求失败'
    showError(`映射测试失败: ${errorMsg}`)
  } finally {
    testingMapping.value = null
  }
}

// 监听 provider 变化
watch(() => props.provider?.id, (newId) => {
  if (newId) {
    loadModels()
  }
}, { immediate: true })

onMounted(() => {
  if (props.provider?.id) {
    loadModels()
  }
})

// 暴露给父组件，用于检测是否有弹窗打开
defineExpose({
  dialogOpen: computed(() => dialogOpen.value || deleteConfirmOpen.value)
})
</script>
