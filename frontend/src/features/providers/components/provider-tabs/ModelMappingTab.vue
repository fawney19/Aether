<template>
  <Card class="overflow-hidden">
    <!-- 标题头部 -->
    <div class="p-4 border-b border-border/60">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-semibold flex items-center gap-2">
          模型映射
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

    <!-- 映射列表 -->
    <div
      v-else-if="combinedMappings.length > 0"
      ref="mappingsListRef"
      class="divide-y divide-border/40"
    >
      <div
        v-for="item in paginatedMappings"
        :key="item.key"
        class="transition-colors"
      >
        <!-- 行头部（可点击展开） -->
        <div
          class="flex items-center justify-between px-4 py-3 hover:bg-muted/20 cursor-pointer"
          @click="toggleExpand(item.key)"
        >
          <div class="flex items-center gap-2 flex-1 min-w-0">
            <!-- 展开/收起图标 -->
            <ChevronRight
              class="w-4 h-4 text-muted-foreground shrink-0 transition-transform self-start mt-0.5"
              :class="{ 'rotate-90': expandedItems.has(item.key) }"
            />
            <!-- 精确映射 -->
            <template v-if="item.type === 'exact'">
              <div class="flex flex-col min-w-0">
                <span class="font-semibold text-sm truncate">
                  {{ item.targetModelName }}
                </span>
                <span
                  v-if="item.group?.model.provider_model_name"
                  class="text-xs text-muted-foreground truncate"
                >
                  {{ item.group.model.provider_model_name }}
                </span>
              </div>
              <!-- 类型标签 -->
              <Badge
                variant="default"
                class="text-xs shrink-0"
              >
                精确
              </Badge>
              <!-- 分隔符 + 映射数量 -->
              <span class="text-xs text-muted-foreground shrink-0">
                | {{ item.mappings.length }} 个映射
              </span>
            </template>
            <!-- 正则映射 -->
            <template v-else>
              <div class="flex flex-col min-w-0">
                <span class="font-semibold text-sm truncate">
                  {{ item.targetModelName }}
                </span>
                <span
                  v-if="item.globalModelName"
                  class="text-xs text-muted-foreground truncate"
                >
                  {{ item.globalModelName }}
                </span>
              </div>
              <!-- 类型标签 -->
              <Badge
                variant="secondary"
                class="text-xs shrink-0"
              >
                正则
              </Badge>
              <!-- 分隔符 + 映射数量 -->
              <span class="text-xs text-muted-foreground shrink-0">
                | {{ item.mappings.length }} 个映射
              </span>
              <!-- Key 数量 -->
              <span
                v-if="item.matchedKeys && item.matchedKeys.length > 0"
                class="text-xs text-muted-foreground shrink-0"
              >
                · {{ item.matchedKeys.length }} Key
              </span>
            </template>
          </div>
          <!-- 操作按钮（仅精确映射可编辑/删除） -->
          <div
            v-if="item.type === 'exact'"
            class="flex items-center gap-1.5 ml-4 shrink-0"
            @click.stop
          >
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="编辑映射"
              @click="item.group && editGroup(item.group)"
            >
              <Edit class="w-3.5 h-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8 hover:text-destructive"
              title="删除映射"
              @click="item.group && deleteGroup(item.group)"
            >
              <Trash2 class="w-3.5 h-3.5" />
            </Button>
          </div>
        </div>

        <!-- 展开的映射详情 -->
        <div
          v-show="expandedItems.has(item.key)"
          class="bg-muted/30 border-t border-border/30"
        >
          <!-- 精确映射详情 -->
          <div
            v-if="item.type === 'exact'"
            class="px-4 py-2 space-y-1"
          >
            <div
              v-for="mapping in item.mappings"
              :key="mapping.name"
              class="flex items-center justify-between gap-2 py-1"
            >
              <span class="font-mono text-sm truncate">
                {{ mapping.name }}
              </span>
              <!-- 测试按钮（直连测试） -->
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7 shrink-0"
                title="测试映射"
                :disabled="testingMapping === `${item.key}-${mapping.name}`"
                @click="testMapping(item, mapping)"
              >
                <Loader2
                  v-if="testingMapping === `${item.key}-${mapping.name}`"
                  class="w-3 h-3 animate-spin"
                />
                <Play
                  v-else
                  class="w-3 h-3"
                />
              </Button>
            </div>
          </div>

          <!-- 正则映射详情（按 Key 分组显示） -->
          <div
            v-else
            class="px-4 py-3 space-y-3"
          >
            <div
              v-for="keyItem in item.matchedKeys"
              :key="keyItem.keyId"
              class="bg-background rounded-md border p-3"
            >
              <!-- Key 信息和正则表达式（两列布局） -->
              <div class="flex items-center gap-3">
                <!-- 第一列：Key 名称 + sk -->
                <div class="flex flex-col shrink-0">
                  <span class="font-medium text-sm">{{ keyItem.keyName || '未命名密钥' }}</span>
                  <span class="text-xs text-muted-foreground font-mono">
                    {{ keyItem.maskedKey }}
                  </span>
                </div>
                <!-- 分隔线 -->
                <div
                  v-if="getKeyPatterns(keyItem).length > 0"
                  class="w-px h-8 bg-border shrink-0"
                />
                <!-- 第二列：正则表达式（限制2行） -->
                <div
                  v-if="getKeyPatterns(keyItem).length > 0"
                  class="flex-1 min-w-0"
                >
                  <div
                    class="font-mono text-[11px] text-muted-foreground line-clamp-2"
                    :title="getKeyPatterns(keyItem).join(', ')"
                  >
                    {{ getKeyPatterns(keyItem).join(', ') }}
                  </div>
                </div>
              </div>
              <!-- 匹配的模型列表 -->
              <div class="mt-2 space-y-1">
                <div
                  v-for="match in keyItem.matches"
                  :key="match.name"
                  class="flex items-center justify-between gap-2 py-1"
                >
                  <span class="font-mono text-sm truncate">{{ match.name }}</span>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7 shrink-0"
                    title="测试映射"
                    :disabled="testingMapping === `${item.key}-${keyItem.keyId}-${match.name}`"
                    @click="testRegexMapping(item, keyItem, match)"
                  >
                    <Loader2
                      v-if="testingMapping === `${item.key}-${keyItem.keyId}-${match.name}`"
                      class="w-3 h-3 animate-spin"
                    />
                    <Play
                      v-else
                      class="w-3 h-3"
                    />
                  </Button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>
      <!-- 分页控制 -->
      <div
        v-if="shouldPaginateMappings"
        class="px-4 py-2 flex items-center justify-between text-xs text-muted-foreground"
      >
        <span>共 {{ combinedMappings.length }} 个映射</span>
        <div class="flex items-center gap-1.5">
          <Button
            variant="ghost"
            size="sm"
            class="h-6 px-2 text-xs"
            :disabled="currentMappingPage <= 1"
            @click="currentMappingPage--"
          >
            ‹
          </Button>
          <span class="tabular-nums">{{ currentMappingPage }} / {{ totalMappingPages }}</span>
          <Button
            variant="ghost"
            size="sm"
            class="h-6 px-2 text-xs"
            :disabled="currentMappingPage >= totalMappingPages"
            @click="currentMappingPage++"
          >
            ›
          </Button>
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
    :models="models"
    :editing-group="editingGroup"
    :preselected-model-id="preselectedModelId"
    :has-auto-fetch-key="hasAutoFetchKey"
    @saved="onDialogSaved"
  />

  <!-- 删除确认对话框 -->
  <AlertDialog
    v-model="deleteConfirmOpen"
    title="删除映射"
    :description="deleteConfirmDescription"
    confirm-text="删除"
    cancel-text="取消"
    type="danger"
    @confirm="confirmDelete"
    @cancel="deleteConfirmOpen = false"
  />

  <!-- 模型测试对话框 -->
  <ModelTestDialog
    :open="modelTest.dialogOpen.value"
    :result="modelTest.testResult.value"
    mode="direct"
    :selecting-model-name="testingModelName"
    :testing="modelTest.testing.value"
    :trace="modelTest.testTrace.value"
    :request-id="modelTest.requestId.value"
    :message-draft="testMessageDraft"
    @close="handleTestDialogClose"
    @start="handleStartMappingTest"
    @update:message-draft="testMessageDraft = $event"
  />
</template>

<script setup lang="ts">
import { ref, computed } from 'vue'
import { useSmartPagination } from '@/composables/useSmartPagination'
import { useModelTest } from '@/composables/useModelTest'
import { Tag, Plus, Edit, Trash2, ChevronRight, Loader2, Play } from 'lucide-vue-next'
import {
  Card, Button, Badge,
} from '@/components/ui'
import AlertDialog from '@/components/common/AlertDialog.vue'
import ModelMappingDialog, { type AliasGroup } from '../ModelMappingDialog.vue'
import ModelTestDialog from './ModelTestDialog.vue'
import { useToast } from '@/composables/useToast'
import {
  type Model,
  type ProviderModelAlias,
  type ProviderMappingPreviewResponse,
} from '@/api/endpoints'
import { type EndpointAPIKey } from '@/api/endpoints/keys'
import { updateModel } from '@/api/endpoints/models'
import { parseApiError } from '@/utils/errorParser'
import type { ProviderWithEndpointsSummary } from '@/api/endpoints'

interface MappingItem {
  name: string
  priority?: number
  pattern?: string
}

interface MatchedKeyInfo {
  keyId: string
  keyName: string
  maskedKey: string
  matches: MappingItem[]
}

interface CombinedMapping {
  key: string
  type: 'exact' | 'regex'
  targetModelName: string
  targetModelId?: string
  globalModelName?: string  // 正则映射的全局模型名称
  mappings: MappingItem[]
  matchedKeys?: MatchedKeyInfo[]  // 正则映射的 Key 信息
  group?: AliasGroup
}

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
  providerKeys?: EndpointAPIKey[]
  models?: Model[]
  mappingPreview?: ProviderMappingPreviewResponse | null
}>()

const emit = defineEmits<{
  'refresh': []
}>()

const { error: showError, success: showSuccess } = useToast()

// 模型测试 composable
const modelTest = useModelTest({ providerId: () => props.provider.id })

// 状态
const loading = ref(false)
const dialogOpen = ref(false)
const deleteConfirmOpen = ref(false)
const editingGroup = ref<AliasGroup | null>(null)
const deletingGroup = ref<AliasGroup | null>(null)
const testingMapping = ref<string | null>(null)
const pendingMappingKey = ref<string | null>(null)
const testingModelName = ref<string | null>(null)
const preselectedModelId = ref<string | null>(null)
const testMessageDraft = ref('')

// 使用 props 传入的数据
const models = computed(() => props.models ?? [])
const aliasMappingPreview = computed(() => props.mappingPreview ?? null)
const providerKeysState = computed(() => props.providerKeys ?? [])

// 是否有 key 配置了自动获取上游模型
const hasAutoFetchKey = computed(() => {
  return providerKeysState.value.some(k => k.auto_fetch_models)
})

// 展开状态
const expandedItems = ref<Set<string>>(new Set())

// 生成作用域唯一键
function getApiFormatsKey(formats: string[] | undefined): string {
  if (!formats || formats.length === 0) return ''
  return [...formats].sort().join(',')
}

// 精确映射分组（来自 provider_model_mappings）
const exactMappingGroups = computed<AliasGroup[]>(() => {
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
      groupMap.get(groupKey)?.aliases.push(alias)
    }
  }

  for (const group of groups) {
    group.aliases.sort((a, b) => a.priority - b.priority)
  }

  return groups
})

// 正则映射（来自 aliasMappingPreview，按 GlobalModel 分组并保留 Key 信息）
const regexMappings = computed<CombinedMapping[]>(() => {
  if (!aliasMappingPreview.value) return []

  const result: CombinedMapping[] = []
  const modelMap = new Map<string, CombinedMapping>()

  for (const keyInfo of aliasMappingPreview.value.keys) {
    for (const gm of keyInfo.matching_global_models) {
      if (!modelMap.has(gm.global_model_id)) {
        modelMap.set(gm.global_model_id, {
          key: `regex-${gm.global_model_id}`,
          type: 'regex',
          targetModelName: gm.display_name,
          targetModelId: gm.global_model_id,
          globalModelName: gm.global_model_name,
          mappings: [],
          matchedKeys: []
        })
        result.push(modelMap.get(gm.global_model_id) as CombinedMapping)
      }

      const mapping = modelMap.get(gm.global_model_id)
      if (!mapping) continue

      // 添加 Key 信息
      const keyMatches: MappingItem[] = gm.matched_models.map(m => ({
        name: m.allowed_model,
        pattern: m.mapping_pattern
      }))

      mapping.matchedKeys?.push({
        keyId: keyInfo.key_id,
        keyName: keyInfo.key_name,
        maskedKey: keyInfo.masked_key,
        matches: keyMatches
      })

      // 收集所有映射（去重）
      for (const match of gm.matched_models) {
        if (!mapping.mappings.some(m => m.name === match.allowed_model)) {
          mapping.mappings.push({
            name: match.allowed_model,
            pattern: match.mapping_pattern
          })
        }
      }
    }
  }

  return result
})

// 合并后的映射列表
const combinedMappings = computed<CombinedMapping[]>(() => {
  const result: CombinedMapping[] = []

  // 添加精确映射
  for (const group of exactMappingGroups.value) {
    result.push({
      key: `exact-${group.model.id}-${group.apiFormatsKey}`,
      type: 'exact',
      targetModelName: group.model.global_model_display_name || group.model.provider_model_name,
      targetModelId: group.model.id,
      mappings: group.aliases.map(a => ({
        name: a.name,
        priority: a.priority
      })),
      group
    })
  }

  // 添加正则映射
  for (const mapping of regexMappings.value) {
    result.push(mapping)
  }

  return result.sort((a, b) => {
    // 精确映射排在前面
    if (a.type !== b.type) return a.type === 'exact' ? -1 : 1
    return a.targetModelName.localeCompare(b.targetModelName)
  })
})

// ===== 模型映射智能分页 =====
const mappingsListRef = ref<HTMLElement | null>(null)
const {
  currentPage: currentMappingPage,
  totalPages: totalMappingPages,
  shouldPaginate: shouldPaginateMappings,
  paginatedItems: paginatedMappings,
} = useSmartPagination(combinedMappings, mappingsListRef)

// 刷新数据（通知父组件刷新）
function refresh() {
  emit('refresh')
}

// 删除确认描述
const deleteConfirmDescription = computed(() => {
  if (!deletingGroup.value) return ''
  const { model, aliases } = deletingGroup.value
  const modelName = model.global_model_display_name || model.provider_model_name
  const aliasNames = aliases.map(a => a.name).join(', ')
  return `确定要删除模型「${modelName}」的 ${aliases.length} 个映射吗？\n\n映射名称：${aliasNames}`
})

// 切换展开状态
function toggleExpand(key: string) {
  if (expandedItems.value.has(key)) {
    expandedItems.value.delete(key)
  } else {
    expandedItems.value.add(key)
  }
}

// 获取单个 Key 的去重正则模式列表
function getKeyPatterns(keyItem: MatchedKeyInfo): string[] {
  const patterns = new Set<string>()
  for (const match of keyItem.matches) {
    if (match.pattern) {
      patterns.add(match.pattern)
    }
  }
  return Array.from(patterns)
}

// 打开添加对话框
function openAddDialog() {
  editingGroup.value = null
  preselectedModelId.value = null
  dialogOpen.value = true
}

// 编辑分组
function editGroup(group: AliasGroup) {
  editingGroup.value = group
  preselectedModelId.value = null
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

    showSuccess('映射已删除')
    deleteConfirmOpen.value = false
    deletingGroup.value = null
    emit('refresh')
  } catch (err: unknown) {
    showError(parseApiError(err, '删除失败'), '错误')
  }
}

// 对话框保存后回调
async function onDialogSaved() {
  emit('refresh')
}

function handleTestDialogClose() {
  modelTest.resetState()
  pendingMappingKey.value = null
  testingModelName.value = null
  testingMapping.value = null
}

// 测试映射（直连测试，带故障转移和实时进度）
function runMappingTest(testingKey: string, modelName: string) {
  pendingMappingKey.value = testingKey
  modelTest.testResult.value = null
  modelTest.dialogOpen.value = true
  testingMapping.value = null
  testingModelName.value = modelName
}

async function handleStartMappingTest() {
  if (modelTest.testing.value || !testingModelName.value || !pendingMappingKey.value) return

  const currentMappingKey = pendingMappingKey.value
  testingMapping.value = currentMappingKey
  await modelTest.startTest({
    mode: 'direct',
    modelName: testingModelName.value,
    displayLabel: `映射 "${testingModelName.value}"`,
    message: testMessageDraft.value,
    onSuccess: () => {
      pendingMappingKey.value = null
      testingModelName.value = null
    },
  })
  if (pendingMappingKey.value === currentMappingKey) {
    pendingMappingKey.value = null
  }
  testingMapping.value = null
}

// 测试精确映射
function testMapping(item: CombinedMapping, mapping: MappingItem) {
  runMappingTest(`${item.key}-${mapping.name}`, mapping.name)
}

// 测试正则映射
function testRegexMapping(item: CombinedMapping, keyItem: MatchedKeyInfo, match: MappingItem) {
  runMappingTest(`${item.key}-${keyItem.keyId}-${match.name}`, match.name)
}

// 暴露给父组件
defineExpose({
  dialogOpen: computed(() => dialogOpen.value || deleteConfirmOpen.value),
  reload: refresh
})
</script>
