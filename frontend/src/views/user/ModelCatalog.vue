<template>
  <div class="space-y-6 pb-8">
    <!-- 模型列表 -->
    <Card class="overflow-hidden">
      <!-- 标题和操作栏 -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
          <!-- 左侧：标题 -->
          <h3 class="text-sm sm:text-base font-semibold shrink-0">
            可用模型
          </h3>

          <!-- 右侧：操作区 -->
          <div class="flex flex-wrap items-center gap-2">
            <!-- 搜索框 -->
            <div class="relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground" />
              <Input
                id="model-search"
                v-model="searchQuery"
                type="text"
                placeholder="搜索模型名称..."
                class="w-32 sm:w-44 pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60 focus:border-primary/40 transition-colors"
              />
            </div>

            <div class="hidden sm:block h-4 w-px bg-border" />

            <!-- 能力筛选 -->
            <div class="flex items-center border rounded-md border-border/60 h-8 overflow-hidden">
              <button
                class="px-2.5 h-full text-xs transition-colors"
                :class="capabilityFilters.vision ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                title="Vision"
                @click="capabilityFilters.vision = !capabilityFilters.vision"
              >
                <Eye class="w-3.5 h-3.5" />
              </button>
              <div class="w-px h-4 bg-border/60" />
              <button
                class="px-2.5 h-full text-xs transition-colors"
                :class="capabilityFilters.toolUse ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                title="Tool Use"
                @click="capabilityFilters.toolUse = !capabilityFilters.toolUse"
              >
                <Wrench class="w-3.5 h-3.5" />
              </button>
              <div class="w-px h-4 bg-border/60" />
              <button
                class="px-2.5 h-full text-xs transition-colors"
                :class="capabilityFilters.extendedThinking ? 'bg-primary text-primary-foreground' : 'hover:bg-muted'"
                title="Extended Thinking"
                @click="capabilityFilters.extendedThinking = !capabilityFilters.extendedThinking"
              >
                <Brain class="w-3.5 h-3.5" />
              </button>
            </div>

            <div class="hidden sm:block h-4 w-px bg-border" />

            <!-- 刷新按钮 -->
            <RefreshButton
              :loading="loading"
              @click="refreshData"
            />
          </div>
        </div>
      </div>

      <div class="overflow-x-auto">
        <Table class="hidden xl:table table-fixed w-full">
          <TableHeader>
            <TableRow class="border-b border-border/60 hover:bg-transparent">
              <TableHead class="w-[140px] h-12 font-semibold">
                模型名称
              </TableHead>
              <TableHead class="w-[120px] h-12 font-semibold">
                模型偏好
              </TableHead>
              <TableHead class="w-[100px] h-12 font-semibold">
                能力
              </TableHead>
              <TableHead class="w-[140px] h-12 font-semibold text-center">
                价格 ($/M)
              </TableHead>
              <TableHead class="w-[70px] h-12 font-semibold text-center">
                状态
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-if="loading">
              <TableCell
                colspan="5"
                class="text-center py-12"
              >
                <Loader2 class="w-6 h-6 animate-spin mx-auto" />
              </TableCell>
            </TableRow>
            <TableRow v-else-if="filteredModels.length === 0">
              <TableCell
                colspan="5"
                class="text-center py-12 text-muted-foreground"
              >
                没有找到匹配的模型
              </TableCell>
            </TableRow>
            <template v-else>
              <TableRow
                v-for="model in paginatedModels"
                :key="model.id"
                class="border-b border-border/40 hover:bg-muted/30 transition-colors cursor-pointer"
                @mousedown="handleMouseDown"
                @click="openModelDetail(model, $event)"
              >
                <TableCell class="py-4">
                  <div>
                    <div class="flex items-center gap-2">
                      <span class="font-medium hover:text-primary transition-colors">{{ model.display_name || model.name }}</span>
                    </div>
                    <div class="text-xs text-muted-foreground flex items-center gap-1 mt-0.5">
                      <span>{{ model.name }}</span>
                      <button
                        class="p-0.5 rounded hover:bg-muted transition-colors"
                        title="复制模型 ID"
                        @click.stop="copyToClipboard(model.name)"
                      >
                        <Copy class="w-3 h-3" />
                      </button>
                    </div>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="flex gap-1.5 flex-wrap items-center">
                    <template v-if="getModelSupportedCapabilities(model).length > 0">
                      <button
                        v-for="cap in getModelSupportedCapabilitiesDetails(model)"
                        :key="cap.name"
                        class="inline-flex items-center gap-1 px-2 py-1 rounded-full text-xs font-medium transition-all"
                        :class="[
                          isCapabilityEnabled(model.name, cap.name)
                            ? 'bg-primary text-primary-foreground'
                            : 'bg-transparent text-muted-foreground border border-dashed border-muted-foreground/50 hover:border-primary/50 hover:text-foreground'
                        ]"
                        :title="cap.description"
                        @click.stop="toggleCapability(model.name, cap.name)"
                      >
                        <Check
                          v-if="isCapabilityEnabled(model.name, cap.name)"
                          class="w-3 h-3"
                        />
                        <Plus
                          v-else
                          class="w-3 h-3"
                        />
                        {{ cap.short_name || cap.display_name }}
                      </button>
                    </template>
                    <span
                      v-else
                      class="text-muted-foreground text-xs"
                    >-</span>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="flex gap-1.5">
                    <Eye
                      v-if="model.config?.vision === true"
                      class="w-4 h-4 text-muted-foreground"
                      title="Vision"
                    />
                    <Wrench
                      v-if="model.config?.function_calling === true"
                      class="w-4 h-4 text-muted-foreground"
                      title="Tool Use"
                    />
                    <Brain
                      v-if="model.config?.extended_thinking === true"
                      class="w-4 h-4 text-muted-foreground"
                      title="Extended Thinking"
                    />
                  </div>
                </TableCell>
                <TableCell class="py-4 text-center">
                  <div class="text-xs space-y-0.5">
                    <!-- 按 Token 计费 -->
                    <div v-if="getFirstTierPrice(model, 'input') || getFirstTierPrice(model, 'output')">
                      <span class="text-muted-foreground">In:</span>
                      <span class="font-mono ml-1">{{ getFirstTierPrice(model, 'input')?.toFixed(2) || '-' }}</span>
                      <span class="text-muted-foreground mx-1">/</span>
                      <span class="text-muted-foreground">Out:</span>
                      <span class="font-mono ml-1">{{ getFirstTierPrice(model, 'output')?.toFixed(2) || '-' }}</span>
                      <span
                        v-if="hasTieredPricing(model)"
                        class="ml-1 text-muted-foreground"
                        title="阶梯计费"
                      >[阶梯]</span>
                    </div>
                    <!-- 按次计费 -->
                    <div v-if="model.default_price_per_request && model.default_price_per_request > 0">
                      <span class="text-muted-foreground">按次:</span>
                      <span class="font-mono ml-1">${{ model.default_price_per_request.toFixed(3) }}/次</span>
                    </div>
                    <!-- 无计费配置 -->
                    <div
                      v-if="!getFirstTierPrice(model, 'input') && !getFirstTierPrice(model, 'output') && !model.default_price_per_request"
                      class="text-muted-foreground"
                    >
                      -
                    </div>
                  </div>
                </TableCell>
                <TableCell class="py-4 text-center">
                  <Badge :variant="model.is_active ? 'success' : 'secondary'">
                    {{ model.is_active ? '可用' : '停用' }}
                  </Badge>
                </TableCell>
              </TableRow>
            </template>
          </TableBody>
        </Table>

        <!-- 移动端卡片列表 -->
        <div
          v-if="!loading && filteredModels.length > 0"
          class="xl:hidden divide-y divide-border/40"
        >
          <div
            v-for="model in paginatedModels"
            :key="model.id"
            class="p-4 space-y-3 hover:bg-muted/30 cursor-pointer transition-colors"
            @click="selectedModel = model; drawerOpen = true"
          >
            <!-- 第一行：名称 + 状态 -->
            <div class="flex items-start justify-between gap-3">
              <div class="flex-1 min-w-0">
                <span class="font-medium truncate block">{{ model.display_name || model.name }}</span>
                <div class="text-xs text-muted-foreground flex items-center gap-1 mt-0.5">
                  <span class="truncate">{{ model.name }}</span>
                  <button
                    class="p-0.5 rounded hover:bg-muted transition-colors shrink-0"
                    @click.stop="copyToClipboard(model.name)"
                  >
                    <Copy class="w-3 h-3" />
                  </button>
                </div>
              </div>
              <Badge :variant="model.is_active ? 'success' : 'secondary'">
                {{ model.is_active ? '可用' : '停用' }}
              </Badge>
            </div>

            <!-- 第二行：能力图标 -->
            <div class="flex gap-1.5">
              <Eye
                v-if="model.config?.vision === true"
                class="w-4 h-4 text-muted-foreground"
              />
              <Wrench
                v-if="model.config?.function_calling === true"
                class="w-4 h-4 text-muted-foreground"
              />
              <Brain
                v-if="model.config?.extended_thinking === true"
                class="w-4 h-4 text-muted-foreground"
              />
            </div>

            <!-- 第三行：价格 -->
            <div
              v-if="getFirstTierPrice(model, 'input') || getFirstTierPrice(model, 'output')"
              class="text-xs text-muted-foreground font-mono"
            >
              In: ${{ getFirstTierPrice(model, 'input')?.toFixed(2) || '-' }} / Out: ${{ getFirstTierPrice(model, 'output')?.toFixed(2) || '-' }}
            </div>

            <!-- 第四行：模型偏好按钮 -->
            <div
              v-if="getModelSupportedCapabilities(model).length > 0"
              class="flex gap-1.5 flex-wrap"
              @click.stop
            >
              <button
                v-for="cap in getModelSupportedCapabilitiesDetails(model)"
                :key="cap.name"
                class="inline-flex items-center gap-1 px-2 py-1 rounded-full text-xs font-medium transition-all"
                :class="[
                  isCapabilityEnabled(model.name, cap.name)
                    ? 'bg-primary text-primary-foreground'
                    : 'bg-transparent text-muted-foreground border border-dashed border-muted-foreground/50'
                ]"
                @click="toggleCapability(model.name, cap.name)"
              >
                <Check
                  v-if="isCapabilityEnabled(model.name, cap.name)"
                  class="w-3 h-3"
                />
                <Plus
                  v-else
                  class="w-3 h-3"
                />
                {{ cap.short_name || cap.display_name }}
              </button>
            </div>
          </div>
        </div>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="!loading && filteredModels.length > 0"
        :current="currentPage"
        :total="filteredModels.length"
        :page-size="pageSize"
        cache-key="model-catalog-page-size"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>

    <!-- 模型详情抽屉 -->
    <UserModelDetailDrawer
      v-model:open="drawerOpen"
      :model="selectedModel"
      :capabilities="allCapabilities"
      :user-configurable-capabilities="userConfigurableCapabilities"
      :model-capability-settings="modelCapabilitySettings"
      @toggle-capability="toggleCapability"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import {
  Loader2,
  Eye,
  Wrench,
  Brain,
  Search,
  Copy,
  Check,
  Plus,
} from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import {
  Card,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Badge,
  Input,
  Pagination,
  RefreshButton,
  getPageSizeCache,
} from '@/components/ui'
import {
  getPublicGlobalModels,
  type PublicGlobalModel,
} from '@/api/public-models'
import { meApi } from '@/api/me'
import {
  getUserConfigurableCapabilities,
  getAllCapabilities,
  type CapabilityDefinition
} from '@/api/endpoints'
import UserModelDetailDrawer from './components/UserModelDetailDrawer.vue'
import { useRowClick } from '@/composables/useRowClick'
import { log } from '@/utils/logger'

const { error: showError } = useToast()
const { copyToClipboard } = useClipboard()

// 状态
const loading = ref(false)
const searchQuery = ref('')
const models = ref<PublicGlobalModel[]>([])

// 抽屉状态
const drawerOpen = ref(false)
const selectedModel = ref<PublicGlobalModel | null>(null)

// 使用复用的行点击逻辑
const { handleMouseDown, shouldTriggerRowClick } = useRowClick()

function openModelDetail(model: PublicGlobalModel, event: MouseEvent) {
  if (!shouldTriggerRowClick(event)) return
  selectedModel.value = model
  drawerOpen.value = true
}

// 分页
const currentPage = ref(1)
const pageSize = ref(getPageSizeCache('model-catalog-page-size', 20))

// 能力筛选
const capabilityFilters = ref({
  vision: false,
  toolUse: false,
  extendedThinking: false,
})

// 能力配置相关
const availableCapabilities = ref<CapabilityDefinition[]>([])
const allCapabilities = ref<CapabilityDefinition[]>([])
const userConfigurableCapabilities = computed(() =>
  availableCapabilities.value.filter(cap => cap.config_mode === 'user_configurable')
)
const modelCapabilitySettings = ref<Record<string, Record<string, boolean>>>({})
const savingCapability = ref<string | null>(null) // 正在保存的能力标识 "modelName:capName"

// 获取模型支持的可配置能力名称列表（从 supported_capabilities 字段读取）
function getModelSupportedCapabilities(model: PublicGlobalModel): string[] {
  if (!model.supported_capabilities) return []
  // 只返回用户可配置的能力
  return model.supported_capabilities.filter(capName =>
    userConfigurableCapabilities.value.some(cap => cap.name === capName)
  )
}

// 获取模型支持的可配置能力详情列表
function getModelSupportedCapabilitiesDetails(model: PublicGlobalModel): CapabilityDefinition[] {
  const supportedNames = getModelSupportedCapabilities(model)
  return userConfigurableCapabilities.value.filter(cap => supportedNames.includes(cap.name))
}

// 检查某个能力是否已启用
function isCapabilityEnabled(modelName: string, capName: string): boolean {
  return modelCapabilitySettings.value[modelName]?.[capName] || false
}

// 切换能力配置
async function toggleCapability(modelName: string, capName: string) {
  const capKey = `${modelName}:${capName}`
  if (savingCapability.value === capKey) return // 防止重复点击

  savingCapability.value = capKey
  try {
    const currentEnabled = isCapabilityEnabled(modelName, capName)
    const newEnabled = !currentEnabled

    // 更新本地状态
    const newSettings = { ...modelCapabilitySettings.value }
    if (!newSettings[modelName]) {
      newSettings[modelName] = {}
    }

    if (newEnabled) {
      newSettings[modelName][capName] = true
    } else {
      delete newSettings[modelName][capName]
      // 如果该模型没有任何能力配置了，删除整个模型条目
      if (Object.keys(newSettings[modelName]).length === 0) {
        delete newSettings[modelName]
      }
    }

    // 调用 API 保存
    await meApi.updateModelCapabilitySettings({
      model_capability_settings: Object.keys(newSettings).length > 0 ? newSettings : null
    })

    // 更新本地状态
    modelCapabilitySettings.value = newSettings
  } catch (err) {
    log.error('保存能力配置失败:', err)
    showError('保存失败，请重试')
  } finally {
    savingCapability.value = null
  }
}

// 筛选后的模型列表
const filteredModels = computed(() => {
  let result = models.value

  // 搜索（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    result = result.filter(m => {
      const searchableText = `${m.name} ${m.display_name || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  // 能力筛选
  if (capabilityFilters.value.vision) {
    result = result.filter(m => m.config?.vision === true)
  }
  if (capabilityFilters.value.toolUse) {
    result = result.filter(m => m.config?.function_calling === true)
  }
  if (capabilityFilters.value.extendedThinking) {
    result = result.filter(m => m.config?.extended_thinking === true)
  }

  return result
})

// 分页计算
const paginatedModels = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  const end = start + pageSize.value
  return filteredModels.value.slice(start, end)
})

// 搜索或筛选变化时重置到第一页
watch([searchQuery, capabilityFilters], () => {
  currentPage.value = 1
}, { deep: true })

async function loadModels() {
  loading.value = true
  try {
    const response = await getPublicGlobalModels({ limit: 1000 })
    models.value = response.models || []
  } catch (err: any) {
    log.error('加载模型失败:', err)
    showError(err.response?.data?.detail || err.message, '加载模型失败')
  } finally {
    loading.value = false
  }
}

async function loadCapabilities() {
  try {
    const [userCaps, allCaps] = await Promise.all([
      getUserConfigurableCapabilities(),
      getAllCapabilities()
    ])
    availableCapabilities.value = userCaps
    allCapabilities.value = allCaps
  } catch (err) {
    log.error('Failed to load capabilities:', err)
  }
}

async function loadModelCapabilitySettings() {
  try {
    const response = await meApi.getModelCapabilitySettings()
    modelCapabilitySettings.value = response.model_capability_settings || {}
  } catch (err) {
    log.error('Failed to load model capability settings:', err)
  }
}

async function refreshData() {
  await Promise.all([loadModels(), loadCapabilities(), loadModelCapabilitySettings()])
}

// 从 PublicGlobalModel 的 default_tiered_pricing 获取第一阶梯价格
function getFirstTierPrice(model: PublicGlobalModel, type: 'input' | 'output'): number | null {
  const tiered = model.default_tiered_pricing
  if (!tiered?.tiers?.length) return null
  const firstTier = tiered.tiers[0]
  if (type === 'input') {
    return firstTier.input_price_per_1m || null
  }
  return firstTier.output_price_per_1m || null
}

// 检测是否有阶梯计费（多于一个阶梯）
function hasTieredPricing(model: PublicGlobalModel): boolean {
  const tiered = model.default_tiered_pricing
  return (tiered?.tiers?.length || 0) > 1
}

onMounted(() => {
  refreshData()
})
</script>
