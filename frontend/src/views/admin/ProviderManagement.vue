<template>
  <div class="space-y-4">
    <!-- 提供商表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和操作栏 -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/50">
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
          <!-- 左侧：标题 -->
          <h3 class="text-sm sm:text-base font-semibold text-foreground shrink-0">
            提供商管理
          </h3>

          <!-- 右侧：操作区 -->
          <div class="flex flex-wrap items-center gap-2">
            <!-- 搜索框 -->
            <div class="relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground/70 z-10 pointer-events-none" />
              <Input
                id="provider-search"
                v-model="searchQuery"
                type="text"
                placeholder="搜索提供商..."
                class="w-32 sm:w-44 pl-8 pr-3 h-8 text-sm bg-muted/30 border-border/50 focus:border-primary/50 transition-colors"
              />
            </div>

            <div class="hidden sm:block h-4 w-px bg-border" />

            <!-- 调度策略 -->
            <button
              class="group inline-flex items-center gap-1.5 px-2.5 h-8 rounded-md border border-border/50 bg-muted/20 hover:bg-muted/40 hover:border-primary/40 transition-all duration-200 text-xs"
              title="点击调整调度策略"
              @click="openPriorityDialog"
            >
              <span class="text-muted-foreground/80 hidden sm:inline">调度:</span>
              <span class="font-medium text-foreground/90">{{ priorityModeConfig.label }}</span>
              <ChevronDown class="w-3 h-3 text-muted-foreground/70 group-hover:text-foreground transition-colors" />
            </button>

            <div class="hidden sm:block h-4 w-px bg-border" />

            <!-- 操作按钮 -->
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="新增提供商"
              @click="openAddProviderDialog"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>
            <RefreshButton
              :loading="loading"
              @click="loadProviders"
            />
          </div>
        </div>
      </div>

      <!-- 加载状态 -->
      <div
        v-if="loading"
        class="flex items-center justify-center py-12"
      >
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
      </div>

      <!-- 空状态 -->
      <div
        v-else-if="filteredProviders.length === 0"
        class="flex flex-col items-center justify-center py-16 text-center"
      >
        <div class="text-muted-foreground mb-2">
          <template v-if="searchQuery">
            未找到匹配 "{{ searchQuery }}" 的提供商
          </template>
          <template v-else>
            暂无提供商，点击右上角添加
          </template>
        </div>
        <Button
          v-if="searchQuery"
          variant="outline"
          size="sm"
          @click="searchQuery = ''"
        >
          清除搜索
        </Button>
      </div>

      <!-- 桌面端表格 -->
      <div
        v-else
        class="hidden xl:block overflow-x-auto"
      >
        <Table>
          <TableHeader>
            <TableRow class="border-b border-border/40 hover:bg-transparent">
              <TableHead class="w-[150px] h-11 font-medium text-foreground/80">
                提供商信息
              </TableHead>
              <TableHead class="w-[100px] h-11 font-medium text-foreground/80">
                计费类型
              </TableHead>
              <TableHead class="w-[120px] h-11 font-medium text-foreground/80">
                官网
              </TableHead>
              <TableHead class="w-[120px] h-11 font-medium text-foreground/80 text-center">
                资源统计
              </TableHead>
              <TableHead class="w-[240px] h-11 font-medium text-foreground/80">
                端点健康
              </TableHead>
              <TableHead class="w-[140px] h-11 font-medium text-foreground/80">
                配额/限流
              </TableHead>
              <TableHead class="w-[80px] h-11 font-medium text-foreground/80 text-center">
                状态
              </TableHead>
              <TableHead class="w-[120px] h-11 font-medium text-foreground/80 text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="provider in paginatedProviders"
              :key="provider.id"
              class="border-b border-border/30 hover:bg-muted/20 transition-colors cursor-pointer"
              @mousedown="handleMouseDown"
              @click="handleRowClick($event, provider.id)"
            >
              <TableCell class="py-3.5">
                <div class="flex flex-col gap-0.5">
                  <span class="text-sm font-medium text-foreground">{{ provider.display_name }}</span>
                  <span class="text-xs text-muted-foreground/70 font-mono">{{ provider.name }}</span>
                </div>
              </TableCell>
              <TableCell class="py-3.5">
                <Badge
                  variant="outline"
                  class="text-xs font-normal border-border/50"
                >
                  {{ formatBillingType(provider.billing_type || 'pay_as_you_go') }}
                </Badge>
              </TableCell>
              <TableCell class="py-3.5">
                <a
                  v-if="provider.website"
                  :href="provider.website"
                  target="_blank"
                  rel="noopener noreferrer"
                  class="text-xs text-primary/80 hover:text-primary hover:underline truncate block max-w-[100px]"
                  :title="provider.website"
                  @click.stop
                >
                  {{ formatWebsiteDisplay(provider.website) }}
                </a>
                <span
                  v-else
                  class="text-xs text-muted-foreground/50"
                >-</span>
              </TableCell>
              <TableCell class="py-3.5 text-center">
                <div class="space-y-0.5 text-xs">
                  <div class="flex items-center justify-center gap-1.5">
                    <span class="text-muted-foreground/70">端点:</span>
                    <span class="font-medium text-foreground/90">{{ provider.active_endpoints }}</span>
                    <span class="text-muted-foreground/50">/{{ provider.total_endpoints }}</span>
                  </div>
                  <div class="flex items-center justify-center gap-1.5">
                    <span class="text-muted-foreground/70">密钥:</span>
                    <span class="font-medium text-foreground/90">{{ provider.active_keys }}</span>
                    <span class="text-muted-foreground/50">/{{ provider.total_keys }}</span>
                  </div>
                  <div class="flex items-center justify-center gap-1.5">
                    <span class="text-muted-foreground/70">模型:</span>
                    <span class="font-medium text-foreground/90">{{ provider.active_models }}</span>
                    <span class="text-muted-foreground/50">/{{ provider.total_models }}</span>
                  </div>
                </div>
              </TableCell>
              <TableCell class="py-3.5 align-middle">
                <div
                  v-if="provider.endpoint_health_details && provider.endpoint_health_details.length > 0"
                  class="flex flex-wrap gap-1.5 max-w-[280px]"
                >
                  <span
                    v-for="endpoint in sortEndpoints(provider.endpoint_health_details)"
                    :key="endpoint.api_format"
                    class="inline-flex items-center gap-1 px-2 py-0.5 rounded-md border text-[10px] font-medium tracking-wide uppercase leading-none"
                    :class="getEndpointTagClass(endpoint, provider)"
                    :title="getEndpointTooltip(endpoint, provider)"
                  >
                    <span
                      class="w-1.5 h-1.5 rounded-full"
                      :class="getEndpointDotColor(endpoint, provider)"
                    />
                    {{ endpoint.api_format }}
                  </span>
                </div>
                <span
                  v-else
                  class="text-xs text-muted-foreground/50"
                >暂无端点</span>
              </TableCell>
              <TableCell class="py-3.5">
                <div class="space-y-0.5 text-xs">
                  <div
                    v-if="provider.billing_type === 'monthly_quota'"
                    class="text-muted-foreground/70"
                  >
                    配额: <span
                      class="font-semibold"
                      :class="getQuotaUsedColorClass(provider)"
                    >${{ (provider.monthly_used_usd ?? 0).toFixed(2) }}</span> / <span class="font-medium">${{ (provider.monthly_quota_usd ?? 0).toFixed(2) }}</span>
                  </div>
                  <div
                    v-if="rpmUsage(provider)"
                    class="flex items-center gap-1"
                  >
                    <span class="text-muted-foreground/70">RPM:</span>
                    <span class="font-medium text-foreground/80">{{ rpmUsage(provider) }}</span>
                  </div>
                  <div
                    v-if="provider.billing_type !== 'monthly_quota' && !rpmUsage(provider)"
                    class="text-muted-foreground/50"
                  >
                    无限制
                  </div>
                </div>
              </TableCell>
              <TableCell class="py-3.5 text-center">
                <Badge
                  :variant="provider.is_active ? 'success' : 'secondary'"
                  class="text-xs"
                >
                  {{ provider.is_active ? '活跃' : '已停用' }}
                </Badge>
              </TableCell>
              <TableCell
                class="py-3.5"
                @click.stop
              >
                <div class="flex items-center justify-center gap-0.5">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7 text-muted-foreground/70 hover:text-foreground"
                    title="查看详情"
                    @click="openProviderDrawer(provider.id)"
                  >
                    <Eye class="h-3.5 w-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7 text-muted-foreground/70 hover:text-foreground"
                    title="编辑提供商"
                    @click="openEditProviderDialog(provider)"
                  >
                    <Edit class="h-3.5 w-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7 text-muted-foreground/70 hover:text-foreground"
                    :title="provider.is_active ? '停用提供商' : '启用提供商'"
                    @click="toggleProviderStatus(provider)"
                  >
                    <Power class="h-3.5 w-3.5" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-7 w-7 text-muted-foreground/70 hover:text-destructive"
                    title="删除提供商"
                    @click="handleDeleteProvider(provider)"
                  >
                    <Trash2 class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>

      <!-- 移动端卡片列表 -->
      <div
        v-if="!loading && filteredProviders.length > 0"
        class="xl:hidden divide-y divide-border/40"
      >
        <div
          v-for="provider in paginatedProviders"
          :key="provider.id"
          class="p-4 space-y-3 hover:bg-muted/20 transition-colors cursor-pointer"
          @click="openProviderDrawer(provider.id)"
        >
          <!-- 第一行：名称 + 状态 + 操作 -->
          <div class="flex items-start justify-between gap-3">
            <div class="flex-1 min-w-0">
              <div class="flex items-center gap-2">
                <span class="font-medium text-foreground truncate">{{ provider.display_name }}</span>
                <Badge
                  :variant="provider.is_active ? 'success' : 'secondary'"
                  class="text-xs shrink-0"
                >
                  {{ provider.is_active ? '活跃' : '停用' }}
                </Badge>
              </div>
              <span class="text-xs text-muted-foreground/70 font-mono">{{ provider.name }}</span>
            </div>
            <div
              class="flex items-center gap-0.5 shrink-0"
              @click.stop
            >
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                @click="openEditProviderDialog(provider)"
              >
                <Edit class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                @click="toggleProviderStatus(provider)"
              >
                <Power class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                @click="handleDeleteProvider(provider)"
              >
                <Trash2 class="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>

          <!-- 第二行：计费类型 + 资源统计 -->
          <div class="flex flex-wrap items-center gap-3 text-xs">
            <Badge
              variant="outline"
              class="text-xs font-normal border-border/50"
            >
              {{ formatBillingType(provider.billing_type || 'pay_as_you_go') }}
            </Badge>
            <span class="text-muted-foreground">
              端点 {{ provider.active_endpoints }}/{{ provider.total_endpoints }}
            </span>
            <span class="text-muted-foreground">
              密钥 {{ provider.active_keys }}/{{ provider.total_keys }}
            </span>
            <span class="text-muted-foreground">
              模型 {{ provider.active_models }}/{{ provider.total_models }}
            </span>
          </div>

          <!-- 第三行：端点健康 -->
          <div
            v-if="provider.endpoint_health_details && provider.endpoint_health_details.length > 0"
            class="flex flex-wrap gap-1.5"
          >
            <span
              v-for="endpoint in sortEndpoints(provider.endpoint_health_details)"
              :key="endpoint.api_format"
              class="inline-flex items-center gap-1 px-2 py-0.5 rounded-md border text-[10px] font-medium tracking-wide uppercase leading-none"
              :class="getEndpointTagClass(endpoint, provider)"
            >
              <span
                class="w-1.5 h-1.5 rounded-full"
                :class="getEndpointDotColor(endpoint, provider)"
              />
              {{ endpoint.api_format }}
            </span>
          </div>

          <!-- 第四行：配额/限流 -->
          <div
            v-if="provider.billing_type === 'monthly_quota' || rpmUsage(provider)"
            class="flex items-center gap-3 text-xs text-muted-foreground"
          >
            <span v-if="provider.billing_type === 'monthly_quota'">
              配额: <span
                class="font-semibold"
                :class="getQuotaUsedColorClass(provider)"
              >${{ (provider.monthly_used_usd ?? 0).toFixed(2) }}</span> / ${{ (provider.monthly_quota_usd ?? 0).toFixed(2) }}
            </span>
            <span v-if="rpmUsage(provider)">
              RPM: {{ rpmUsage(provider) }}
            </span>
          </div>
        </div>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="!loading && filteredProviders.length > 0"
        :current="currentPage"
        :total="filteredProviders.length"
        :page-size="pageSize"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>
  </div>

  <!-- 对话框 -->
  <ProviderFormDialog
    v-model="providerDialogOpen"
    :provider="providerToEdit"
    @provider-created="handleProviderAdded"
    @provider-updated="handleProviderUpdated"
  />

  <PriorityManagementDialog
    v-model="priorityDialogOpen"
    :providers="providers"
    @saved="handlePrioritySaved"
  />

  <ProviderDetailDrawer
    :open="providerDrawerOpen"
    :provider-id="selectedProviderId"
    @update:open="providerDrawerOpen = $event"
    @edit="openEditProviderDialog"
    @toggle-status="toggleProviderStatus"
    @refresh="loadProviders"
  />
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import {
  Plus,
  Search,
  Edit,
  Eye,
  Trash2,
  ChevronDown,
  Power
} from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Card from '@/components/ui/card.vue'
import Input from '@/components/ui/input.vue'
import Table from '@/components/ui/table.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableRow from '@/components/ui/table-row.vue'
import TableHead from '@/components/ui/table-head.vue'
import TableCell from '@/components/ui/table-cell.vue'
import Pagination from '@/components/ui/pagination.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import { ProviderFormDialog, PriorityManagementDialog } from '@/features/providers/components'
import ProviderDetailDrawer from '@/features/providers/components/ProviderDetailDrawer.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useRowClick } from '@/composables/useRowClick'
import {
  getProvidersSummary,
  deleteProvider,
  updateProvider,
  type ProviderWithEndpointsSummary
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import { formatBillingType } from '@/utils/format'

const { error: showError, success: showSuccess } = useToast()
const { confirmDanger } = useConfirm()

// 状态
const loading = ref(false)
const providers = ref<ProviderWithEndpointsSummary[]>([])
const providerDialogOpen = ref(false)
const providerToEdit = ref<ProviderWithEndpointsSummary | null>(null)
const priorityDialogOpen = ref(false)
const priorityMode = ref<'provider' | 'global_key'>('provider')
const providerDrawerOpen = ref(false)
const selectedProviderId = ref<string | null>(null)

// 搜索
const searchQuery = ref('')

// 分页
const currentPage = ref(1)
const pageSize = ref(20)

// 优先级模式配置
const priorityModeConfig = computed(() => {
  return {
    label: priorityMode.value === 'global_key' ? '全局 Key 优先' : '提供商优先'
  }
})

// 筛选后的提供商列表
const filteredProviders = computed(() => {
  let result = [...providers.value]

  // 搜索筛选（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value.trim()) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    result = result.filter(p => {
      const searchableText = `${p.display_name} ${p.name}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  // 排序
  return result.sort((a, b) => {
    // 1. 优先显示活跃的提供商
    if (a.is_active !== b.is_active) {
      return a.is_active ? -1 : 1
    }
    // 2. 按优先级排序
    if (a.provider_priority !== b.provider_priority) {
      return a.provider_priority - b.provider_priority
    }
    // 3. 按名称排序
    return a.display_name.localeCompare(b.display_name)
  })
})

// 分页
const paginatedProviders = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  const end = start + pageSize.value
  return filteredProviders.value.slice(start, end)
})

// 搜索时重置分页
watch(searchQuery, () => {
  currentPage.value = 1
})

// 加载优先级模式
async function loadPriorityMode() {
  try {
    const response = await adminApi.getSystemConfig('provider_priority_mode')
    if (response.value) {
      priorityMode.value = response.value as 'provider' | 'global_key'
    }
  } catch {
    priorityMode.value = 'provider'
  }
}

// 加载提供商列表
async function loadProviders() {
  loading.value = true
  try {
    providers.value = await getProvidersSummary()
  } catch (err: any) {
    showError(err.response?.data?.detail || '加载提供商列表失败', '错误')
  } finally {
    loading.value = false
  }
}


// 格式化官网显示
function formatWebsiteDisplay(url: string): string {
  try {
    const urlObj = new URL(url)
    return urlObj.hostname.replace(/^www\./, '')
  } catch {
    return url
  }
}

// 端点排序
function sortEndpoints(endpoints: any[]) {
  return [...endpoints].sort((a, b) => {
    const order = ['CLAUDE', 'OPENAI', 'CLAUDE_COMPATIBLE', 'OPENAI_COMPATIBLE', 'GEMINI', 'GEMINI_COMPATIBLE']
    return order.indexOf(a.api_format) - order.indexOf(b.api_format)
  })
}

// 判断端点是否可用（有 key）
function isEndpointAvailable(endpoint: any, _provider: ProviderWithEndpointsSummary): boolean {
  // 检查该端点是否有活跃的密钥
  return (endpoint.active_keys ?? 0) > 0
}

// 端点标签样式
function getEndpointTagClass(endpoint: any, provider: ProviderWithEndpointsSummary): string {
  if (!isEndpointAvailable(endpoint, provider)) {
    return 'border-red-300/50 bg-red-50/50 text-red-600/80 dark:border-red-500/30 dark:bg-red-500/10 dark:text-red-400/80'
  }
  return 'border-border/40 bg-muted/20 text-foreground/70'
}

// 端点圆点颜色
function getEndpointDotColor(endpoint: any, provider: ProviderWithEndpointsSummary): string {
  if (!isEndpointAvailable(endpoint, provider)) {
    return 'bg-red-400'
  }
  const score = endpoint.health_score
  if (score === undefined || score === null) {
    return 'bg-muted-foreground/40'
  }
  if (score >= 0.8) {
    return 'bg-green-500'
  }
  if (score >= 0.5) {
    return 'bg-amber-500'
  }
  return 'bg-red-500'
}

// 端点提示文本
function getEndpointTooltip(endpoint: any, provider: ProviderWithEndpointsSummary): string {
  if (provider.active_keys === 0) {
    return `${endpoint.api_format}: 无可用密钥`
  }
  const score = endpoint.health_score
  if (score === undefined || score === null) {
    return `${endpoint.api_format}: 暂无健康数据`
  }
  return `${endpoint.api_format}: 健康度 ${(score * 100).toFixed(0)}%`
}

// 配额已用颜色（根据使用比例）
function getQuotaUsedColorClass(provider: ProviderWithEndpointsSummary): string {
  const used = provider.monthly_used_usd ?? 0
  const quota = provider.monthly_quota_usd ?? 0
  if (quota <= 0) return 'text-foreground'
  const ratio = used / quota
  if (ratio >= 0.9) return 'text-red-600 dark:text-red-400'
  if (ratio >= 0.7) return 'text-amber-600 dark:text-amber-400'
  return 'text-foreground'
}

function rpmUsage(provider: ProviderWithEndpointsSummary): string | null {
  const rpmLimit = provider.rpm_limit
  const rpmUsed = provider.rpm_used ?? 0

  if (rpmLimit === null || rpmLimit === undefined) {
    return rpmUsed > 0 ? `${rpmUsed}` : null
  }

  if (rpmLimit === 0) {
    return '已完全禁止'
  }

  return `${rpmUsed} / ${rpmLimit}`
}

// 使用复用的行点击逻辑
const { handleMouseDown, shouldTriggerRowClick } = useRowClick()

// 处理行点击 - 只在非选中文本时打开抽屉
function handleRowClick(event: MouseEvent, providerId: string) {
  if (!shouldTriggerRowClick(event)) return
  openProviderDrawer(providerId)
}

// 打开添加提供商对话框
function openAddProviderDialog() {
  providerToEdit.value = null
  providerDialogOpen.value = true
}

// 打开优先级管理对话框
function openPriorityDialog() {
  priorityDialogOpen.value = true
}

// 打开提供商详情抽屉
function openProviderDrawer(providerId: string) {
  selectedProviderId.value = providerId
  providerDrawerOpen.value = true
}

// 打开编辑提供商对话框
function openEditProviderDialog(provider: ProviderWithEndpointsSummary) {
  providerToEdit.value = provider
  providerDialogOpen.value = true
}

// 处理提供商编辑完成
function handleProviderUpdated() {
  loadProviders()
}

// 优先级保存成功回调
async function handlePrioritySaved() {
  await loadProviders()
  await loadPriorityMode()
}

// 处理提供商添加
function handleProviderAdded() {
  loadProviders()
}

// 删除提供商
async function handleDeleteProvider(provider: ProviderWithEndpointsSummary) {
  const confirmed = await confirmDanger(
    '删除提供商',
    `确定要删除提供商 "${provider.display_name}" 吗？\n\n这将同时删除其所有端点、密钥和配置。此操作不可恢复！`
  )

  if (!confirmed) return

  try {
    await deleteProvider(provider.id)
    showSuccess('提供商已删除')
    loadProviders()
  } catch (err: any) {
    showError(err.response?.data?.detail || '删除提供商失败', '错误')
  }
}

// 切换提供商状态
async function toggleProviderStatus(provider: ProviderWithEndpointsSummary) {
  try {
    const newStatus = !provider.is_active
    await updateProvider(provider.id, { is_active: newStatus })

    // 更新抽屉内部的 provider 对象
    provider.is_active = newStatus

    // 同时更新主页面 providers 数组中的对象，实现无感更新
    const targetProvider = providers.value.find(p => p.id === provider.id)
    if (targetProvider) {
      targetProvider.is_active = newStatus
    }

    showSuccess(newStatus ? '提供商已启用' : '提供商已停用')
  } catch (err: any) {
    showError(err.response?.data?.detail || '操作失败', '错误')
  }
}

onMounted(() => {
  loadProviders()
  loadPriorityMode()
})
</script>
