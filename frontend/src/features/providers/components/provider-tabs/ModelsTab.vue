<template>
  <Card class="overflow-hidden">
    <!-- 标题头部 -->
    <div class="p-4 border-b border-border/60">
      <div class="flex items-center justify-between">
        <h3 class="text-sm font-semibold flex items-center gap-2">
          模型列表
        </h3>
        <Button
          variant="outline"
          size="sm"
          class="h-8"
          @click="openBatchAssignDialog"
        >
          <Layers class="w-3.5 h-3.5 mr-1.5" />
          关联模型
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

    <!-- 模型列表 -->
    <div
      v-else-if="models.length > 0"
      class="overflow-hidden"
    >
      <table class="w-full text-sm table-fixed">
        <thead class="bg-muted/50 text-xs uppercase tracking-wide text-muted-foreground">
          <tr>
            <th class="text-left px-4 py-3 font-semibold w-[45%]">
              模型
            </th>
            <th class="text-left px-4 py-3 font-semibold w-[30%]">
              价格 ($/M)
            </th>
            <th class="text-center px-4 py-3 font-semibold w-[25%]">
              操作
            </th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="model in sortedModels"
            :key="model.id"
            class="border-b border-border/40 last:border-b-0 hover:bg-muted/30 transition-colors"
          >
            <td class="align-top px-4 py-3">
              <div class="flex items-center gap-2.5">
                <!-- 状态指示灯 -->
                <div
                  class="w-2 h-2 rounded-full shrink-0"
                  :class="getStatusIndicatorClass(model)"
                  :title="getStatusTitle(model)"
                />
                <!-- 模型信息 -->
                <div class="text-left flex-1 min-w-0">
                  <span class="font-semibold text-sm">
                    {{ model.global_model_display_name || model.provider_model_name }}
                  </span>
                  <div class="text-xs text-muted-foreground mt-1 flex items-center gap-1">
                    <span class="font-mono truncate">{{ model.provider_model_name }}</span>
                    <button
                      class="p-0.5 hover:bg-muted rounded transition-colors shrink-0"
                      title="复制模型 ID"
                      @click.stop="copyModelId(model.provider_model_name)"
                    >
                      <Copy class="w-3 h-3" />
                    </button>
                  </div>
                </div>
              </div>
            </td>
            <td class="align-top px-4 py-3 text-xs whitespace-nowrap">
              <div
                class="grid gap-1"
                style="grid-template-columns: auto 1fr;"
              >
                <!-- 按 Token 计费 -->
                <template v-if="hasTokenPricing(model)">
                  <span class="text-muted-foreground text-right">输入/输出:</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(model.effective_input_price) }}/${{ formatPrice(model.effective_output_price) }}
                  </span>
                </template>
                <template v-if="getEffectiveCachePrice(model, 'creation') > 0 || getEffectiveCachePrice(model, 'read') > 0">
                  <span class="text-muted-foreground text-right">缓存:</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(getEffectiveCachePrice(model, 'creation')) }}/${{ formatPrice(getEffectiveCachePrice(model, 'read')) }}
                  </span>
                </template>
                <!-- 1h 缓存价格 -->
                <template v-if="get1hCachePrice(model) > 0">
                  <span class="text-muted-foreground text-right">1h 缓存:</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(get1hCachePrice(model)) }}
                  </span>
                </template>
                <!-- 按次计费 -->
                <template v-if="hasRequestPricing(model)">
                  <span class="text-muted-foreground text-right">按次:</span>
                  <span class="font-mono font-semibold">
                    ${{ formatPrice(model.effective_price_per_request ?? model.price_per_request) }}/次
                  </span>
                </template>
                <!-- 无计��配置 -->
                <template v-if="!hasTokenPricing(model) && !hasRequestPricing(model)">
                  <span class="text-muted-foreground">—</span>
                </template>
              </div>
            </td>
            <td class="align-top px-4 py-3">
              <div class="flex justify-center gap-1.5">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="添加映射"
                  @click="addMapping(model)"
                >
                  <Link class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="编辑"
                  @click="editModel(model)"
                >
                  <Edit class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :disabled="togglingModelId === model.id"
                  :title="model.is_active ? '点击停用' : '点击启用'"
                  @click="toggleModelActive(model)"
                >
                  <Power class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8 hover:text-destructive"
                  title="删除"
                  @click="deleteModel(model)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </Button>
              </div>
            </td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- 空状态 -->
    <div
      v-else
      class="p-8 text-center text-muted-foreground"
    >
      <Box class="w-12 h-12 mx-auto mb-3 opacity-50" />
      <p class="text-sm">
        暂无模型
      </p>
      <p class="text-xs mt-1">
        请前往"模型目录"页面添加模型
      </p>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { Box, Edit, Trash2, Layers, Power, Copy, Link } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import { getProviderModels, type Model } from '@/api/endpoints'
import { updateModel } from '@/api/endpoints/models'

const props = defineProps<{
  provider: any
}>()

const emit = defineEmits<{
  'editModel': [model: Model]
  'deleteModel': [model: Model]
  'batchAssign': []
  'addMapping': [model: Model]
}>()

const { error: showError, success: showSuccess } = useToast()
const { copyToClipboard } = useClipboard()

// 状态
const loading = ref(false)
const models = ref<Model[]>([])
const togglingModelId = ref<string | null>(null)

// 按名称排序的模型列表
const sortedModels = computed(() => {
  return [...models.value].sort((a, b) => {
    const nameA = (a.global_model_display_name || a.provider_model_name || '').toLowerCase()
    const nameB = (b.global_model_display_name || b.provider_model_name || '').toLowerCase()
    return nameA.localeCompare(nameB)
  })
})

// 复制模型 ID 到剪贴板
async function copyModelId(modelId: string) {
  await copyToClipboard(modelId)
}

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

// 格式化价格显示
function formatPrice(price: number | null | undefined): string {
  if (price === null || price === undefined) return '-'
  // 如果是整数或小数点后只有1-2位，直接显示
  if (price >= 0.01 || price === 0) {
    return price.toFixed(2)
  }
  // 对于非常小的数字，使用科学计数法
  if (price < 0.0001) {
    return price.toExponential(2)
  }
  // 其他情况保留4位小数
  return price.toFixed(4)
}

// 检查是否有按 Token 计费
function hasTokenPricing(model: Model): boolean {
  const inputPrice = model.effective_input_price
  const outputPrice = model.effective_output_price
  return (inputPrice != null && inputPrice > 0) || (outputPrice != null && outputPrice > 0)
}

// 获取有效的缓存价格（从 effective_tiered_pricing 或 tiered_pricing 中提取）
function getEffectiveCachePrice(model: Model, type: 'creation' | 'read'): number {
  const tiered = model.effective_tiered_pricing || model.tiered_pricing
  if (!tiered?.tiers?.length) return 0
  const firstTier = tiered.tiers[0]
  if (type === 'creation') {
    return firstTier.cache_creation_price_per_1m || 0
  }
  return firstTier.cache_read_price_per_1m || 0
}

// 获取 1h 缓存价格
function get1hCachePrice(model: Model): number {
  const tiered = model.effective_tiered_pricing || model.tiered_pricing
  if (!tiered?.tiers?.length) return 0
  const firstTier = tiered.tiers[0]
  const ttl1h = firstTier.cache_ttl_pricing?.find(t => t.ttl_minutes === 60)
  return ttl1h?.cache_creation_price_per_1m || 0
}

// 检查是否有按次计费
function hasRequestPricing(model: Model): boolean {
  const requestPrice = model.effective_price_per_request ?? model.price_per_request
  return requestPrice != null && requestPrice > 0
}

// 获取状态指示灯样式
function getStatusIndicatorClass(model: Model): string {
  if (!model.is_active) {
    // 已停用 - 灰色
    return 'bg-gray-400 dark:bg-gray-600'
  }
  if (model.is_available) {
    // 活跃且可用 - 绿色
    return 'bg-green-500 dark:bg-green-400'
  }
  // 活跃但不可用 - 红色
  return 'bg-red-500 dark:bg-red-400'
}

// 获取状态提示文本
function getStatusTitle(model: Model): string {
  if (!model.is_active) {
    return '已停用'
  }
  if (model.is_available) {
    return '活跃且可用'
  }
  return '活跃但不可用'
}

// ��辑模型
function editModel(model: Model) {
  emit('editModel', model)
}

// 删除模型
function deleteModel(model: Model) {
  emit('deleteModel', model)
}

// 添加映射
function addMapping(model: Model) {
  emit('addMapping', model)
}

// 打开批量关联对话框
function openBatchAssignDialog() {
  emit('batchAssign')
}

// 切换模型启用状态
async function toggleModelActive(model: Model) {
  if (togglingModelId.value) return

  togglingModelId.value = model.id
  try {
    const newStatus = !model.is_active
    await updateModel(props.provider.id, model.id, { is_active: newStatus })
    model.is_active = newStatus
    showSuccess(newStatus ? '模型已启用' : '模型已停用')
  } catch (err: any) {
    showError(err.response?.data?.detail || '操作失败', '错误')
  } finally {
    togglingModelId.value = null
  }
}

onMounted(() => {
  loadModels()
})
</script>
