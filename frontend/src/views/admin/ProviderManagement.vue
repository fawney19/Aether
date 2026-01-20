<template>
  <div class="space-y-4">
    <!-- 提供商表格 -->
    <Card
      variant="default"
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
              <TableHead class="w-[18%] min-w-[140px] h-11 font-medium text-foreground/80">
                提供商信息
              </TableHead>
              <TableHead class="w-[20%] min-w-[180px] h-11 font-medium text-foreground/80">
                余额监控
              </TableHead>
              <TableHead class="w-[12%] min-w-[100px] h-11 font-medium text-foreground/80 text-center">
                资源统计
              </TableHead>
              <TableHead class="w-[24%] min-w-[260px] h-11 font-medium text-foreground/80">
                端点健康
              </TableHead>
              <TableHead class="w-[8%] min-w-[60px] h-11 font-medium text-foreground/80 text-center">
                状态
              </TableHead>
              <TableHead class="w-[18%] min-w-[120px] h-11 font-medium text-foreground/80 text-center">
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
                <div class="space-y-0.5">
                  <span class="text-sm font-medium text-foreground">{{ provider.name }}</span>
                  <a
                    v-if="provider.website"
                    :href="provider.website"
                    target="_blank"
                    rel="noopener noreferrer"
                    class="text-xs text-primary/80 hover:text-primary hover:underline truncate block max-w-[160px]"
                    :title="provider.website"
                    @click.stop
                  >
                    {{ formatWebsiteDisplay(provider.website) }}
                  </a>
                </div>
              </TableCell>
              <TableCell class="py-3.5">
                <!-- 显示从上游 API 查询的余额 -->
                <div
                  v-if="provider.ops_configured && getProviderBalance(provider.id)"
                  class="flex items-center gap-2 text-xs"
                >
                  <!-- 余额文字 -->
                  <span class="font-semibold text-foreground/90 min-w-[4.5rem] tabular-nums">
                    {{ formatBalanceDisplay(getProviderBalance(provider.id)) }}
                  </span>
                  <!-- 窗口限额 + 签到状态 + Cookie 失效警告 -->
                  <div
                    v-if="getProviderBalanceExtra(provider.id, provider.ops_architecture_id).length > 0 || getProviderCheckin(provider.id) || getProviderCookieExpired(provider.id)"
                    class="text-muted-foreground/70 space-y-0.5"
                  >
                    <!-- 限额（进度条 + 倒计时，每行一个） -->
                    <template
                      v-for="item in getProviderBalanceExtra(provider.id, provider.ops_architecture_id)"
                      :key="item.label"
                    >
                      <div
                        :title="item.tooltip"
                        class="flex items-center gap-1"
                      >
                        <span class="text-[10px] text-muted-foreground/60 w-4">{{ item.label }}</span>
                        <div class="w-12 h-1.5 bg-border rounded-full overflow-hidden">
                          <div
                            class="h-full rounded-full"
                            :class="[
                              item.percent !== undefined && item.percent >= 50 ? 'bg-green-500' :
                              item.percent !== undefined && item.percent >= 20 ? 'bg-amber-500' : 'bg-red-500'
                            ]"
                            :style="{ width: `${item.percent ?? 0}%` }"
                          />
                        </div>
                        <span class="text-[10px] text-muted-foreground/50 w-7 text-right tabular-nums">{{ item.value }}</span>
                        <span
                          v-if="item.resetsAt"
                          class="text-[10px] text-muted-foreground/40 w-14 text-right tabular-nums"
                        >{{ formatResetCountdown(item.resetsAt) }}</span>
                      </div>
                    </template>
                    <!-- Cookie 失效警告 -->
                    <div
                      v-if="getProviderCookieExpired(provider.id)"
                      class="flex items-center gap-1"
                    >
                      <span
                        class="text-[10px] text-amber-600 dark:text-amber-500"
                        :title="getProviderCookieExpired(provider.id)?.message"
                      >签到 Cookie 已失效</span>
                    </div>
                    <!-- 签到状态 -->
                    <div
                      v-else-if="getProviderCheckin(provider.id)"
                      class="flex items-center gap-1.5"
                    >
                      <span
                        v-if="getProviderCheckin(provider.id)?.success !== false"
                        class="text-[10px] text-muted-foreground/60"
                        :title="getProviderCheckin(provider.id)?.message"
                      >已签到</span>
                      <span
                        v-else
                        class="text-[10px] text-destructive/70"
                        :title="getProviderCheckin(provider.id)?.message"
                      >签到失败</span>
                    </div>
                  </div>
                </div>
                <!-- 余额查询失败时显示错误 -->
                <div
                  v-else-if="provider.ops_configured && getProviderBalanceError(provider.id)"
                  class="text-xs text-destructive/80"
                  :title="getProviderBalanceError(provider.id)?.message"
                >
                  {{ getProviderBalanceError(provider.id)?.message }}
                </div>
                <!-- 显示本地配置的月度配额 -->
                <div
                  v-else-if="provider.billing_type === 'monthly_quota'"
                  class="space-y-0.5 text-xs"
                >
                  <Badge
                    variant="outline"
                    class="text-[10px] font-normal border-border/50"
                  >
                    {{ formatBillingType(provider.billing_type) }}
                  </Badge>
                  <div class="text-muted-foreground/70 pt-0.5">
                    <span
                      class="font-semibold"
                      :class="getQuotaUsedColorClass(provider)"
                    >${{ (provider.monthly_used_usd ?? 0).toFixed(2) }}</span> / <span class="font-medium">${{ (provider.monthly_quota_usd ?? 0).toFixed(2) }}</span>
                  </div>
                </div>
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
                  class="grid grid-cols-3 gap-x-3 gap-y-2 max-w-[240px]"
                >
                  <div
                    v-for="endpoint in sortEndpoints(provider.endpoint_health_details)"
                    :key="endpoint.api_format"
                    class="flex flex-col gap-1.5"
                    :title="getEndpointTooltip(endpoint, provider)"
                  >
                    <!-- 上排：缩写 + 百分比 -->
                    <div class="flex items-center justify-between text-[10px] leading-none">
                      <span class="font-medium text-muted-foreground/80">
                        {{ API_FORMAT_SHORT[endpoint.api_format] || endpoint.api_format.substring(0,2) }}
                      </span>
                      <span class="font-medium text-muted-foreground/80">
                        {{ isEndpointAvailable(endpoint, provider) ? `${(endpoint.health_score * 100).toFixed(0)}%` : '-' }}
                      </span>
                    </div>

                    <!-- 下排：进度条 -->
                    <div class="h-1.5 w-full bg-muted/40 rounded-full overflow-hidden">
                      <div
                        class="h-full rounded-full transition-all duration-300"
                        :class="getEndpointDotColor(endpoint, provider)"
                        :style="{ width: isEndpointAvailable(endpoint, provider) ? `${Math.max(endpoint.health_score * 100, 5)}%` : '100%' }"
                      />
                    </div>
                  </div>
                </div>
                <span
                  v-else
                  class="text-xs text-muted-foreground/50"
                >暂无端点</span>
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
                    title="扩展操作配置"
                    @click="openOpsConfigDialog(provider)"
                  >
                    <KeyRound class="h-3.5 w-3.5" />
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
                <span class="font-medium text-foreground truncate">{{ provider.name }}</span>
                <Badge
                  :variant="provider.is_active ? 'success' : 'secondary'"
                  class="text-xs shrink-0"
                >
                  {{ provider.is_active ? '活跃' : '停用' }}
                </Badge>
              </div>
            </div>
            <div
              class="flex items-center gap-0.5 shrink-0"
              @click.stop
            >
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                title="查看详情"
                @click="openProviderDrawer(provider.id)"
              >
                <Eye class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                title="编辑"
                @click="openEditProviderDialog(provider)"
              >
                <Edit class="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                title="扩展操作配置"
                @click="openOpsConfigDialog(provider)"
              >
                <KeyRound class="h-3.5 w-3.5" />
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

          <!-- 第二行：计费类型 + 余额/配额 + 资源统计 -->
          <div class="flex flex-wrap items-center gap-3 text-xs">
            <Badge
              variant="outline"
              class="text-xs font-normal border-border/50"
            >
              {{ formatBillingType(provider.billing_type || 'pay_as_you_go') }}
            </Badge>
            <!-- 余额（从上游 API 查询） -->
            <span
              v-if="provider.ops_configured && getProviderBalance(provider.id)"
              class="text-muted-foreground"
            >
              余额 <span class="font-semibold text-foreground/90">{{ formatBalanceDisplay(getProviderBalance(provider.id)) }}</span>
              <!-- Cookie 失效警告 -->
              <span
                v-if="getProviderCookieExpired(provider.id)"
                class="ml-1 text-amber-600 dark:text-amber-500"
                :title="getProviderCookieExpired(provider.id)?.message"
              >签到 Cookie 已失效</span>
              <!-- 签到状态显示 -->
              <span
                v-else-if="getProviderCheckin(provider.id) && getProviderCheckin(provider.id)?.success !== false"
                class="ml-1 text-muted-foreground"
                :title="getProviderCheckin(provider.id)?.message"
              >已签到</span>
              <span
                v-else-if="getProviderCheckin(provider.id)?.success === false"
                class="ml-1 text-destructive/70"
                :title="getProviderCheckin(provider.id)?.message"
              >签到失败</span>
            </span>
            <!-- 余额查询失败时显示错误 -->
            <span
              v-else-if="provider.ops_configured && getProviderBalanceError(provider.id)"
              class="text-destructive/80"
              :title="getProviderBalanceError(provider.id)?.message"
            >
              {{ getProviderBalanceError(provider.id)?.message }}
            </span>
            <!-- 本地配额 -->
            <span
              v-else-if="provider.billing_type === 'monthly_quota'"
              class="text-muted-foreground"
            >
              配额 <span
                class="font-semibold"
                :class="getQuotaUsedColorClass(provider)"
              >${{ (provider.monthly_used_usd ?? 0).toFixed(2) }}</span>/<span class="font-medium">${{ (provider.monthly_quota_usd ?? 0).toFixed(2) }}</span>
            </span>
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
            class="grid grid-cols-3 gap-x-3 gap-y-2 max-w-[240px]"
          >
            <div
              v-for="endpoint in sortEndpoints(provider.endpoint_health_details)"
              :key="endpoint.api_format"
              class="flex flex-col gap-1.5"
              :title="getEndpointTooltip(endpoint, provider)"
            >
              <!-- 上排：缩写 + 百分比 -->
              <div class="flex items-center justify-between text-[10px] leading-none">
                <span class="font-medium text-muted-foreground/80">
                  {{ API_FORMAT_SHORT[endpoint.api_format] || endpoint.api_format.substring(0,2) }}
                </span>
                <span class="font-medium text-muted-foreground/80">
                  {{ isEndpointAvailable(endpoint, provider) ? `${(endpoint.health_score * 100).toFixed(0)}%` : '-' }}
                </span>
              </div>

              <!-- 下排：进度条 -->
              <div class="h-1.5 w-full bg-muted/40 rounded-full overflow-hidden">
                <div
                  class="h-full rounded-full transition-all duration-300"
                  :class="getEndpointDotColor(endpoint, provider)"
                  :style="{ width: isEndpointAvailable(endpoint, provider) ? `${Math.max(endpoint.health_score * 100, 5)}%` : '100%' }"
                />
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="!loading && filteredProviders.length > 0"
        :current="currentPage"
        :total="filteredProviders.length"
        :page-size="pageSize"
        cache-key="provider-management-page-size"
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

  <ProviderAuthDialog
    v-model:open="opsConfigDialogOpen"
    :provider-id="opsConfigProviderId"
    :provider-website="opsConfigProviderWebsite"
    @saved="handleOpsConfigSaved"
  />
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import {
  Plus,
  Search,
  Edit,
  Eye,
  Trash2,
  ChevronDown,
  Power,
  KeyRound
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
import { ProviderFormDialog, PriorityManagementDialog, ProviderAuthDialog } from '@/features/providers/components'
import ProviderDetailDrawer from '@/features/providers/components/ProviderDetailDrawer.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useRowClick } from '@/composables/useRowClick'
import {
  getProvidersSummary,
  deleteProvider,
  updateProvider,
  type ProviderWithEndpointsSummary,
  API_FORMAT_SHORT
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import { batchQueryBalance, type ActionResultResponse } from '@/api/providerOps'
import { formatBillingType } from '@/utils/format'
import { authTemplateRegistry, type BalanceExtraItem } from '@/features/providers/auth-templates'

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

// 扩展操作配置对话框
const opsConfigDialogOpen = ref(false)
const opsConfigProviderId = ref('')
const opsConfigProviderWebsite = ref('')

// 余额数据缓存 {providerId: ActionResultResponse}
const balanceCache = ref<Record<string, ActionResultResponse>>({})
// 余额加载请求版本计数器（用于防止竞态条件）
// 使用普通变量而非 ref，因为不需要响应式，仅用于比较请求版本
let balanceLoadVersion = 0

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
      const searchableText = `${p.name}`.toLowerCase()
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
    return a.name.localeCompare(b.name)
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
  // 清空旧的余额缓存，避免数据累积
  balanceCache.value = {}
  try {
    providers.value = await getProvidersSummary()
    // 异步加载配置了 ops 的 provider 的余额数据
    loadBalances()
  } catch (err: any) {
    showError(err.response?.data?.detail || '加载提供商列表失败', '错误')
  } finally {
    loading.value = false
  }
}

// 异步加载余额数据（使用批量接口）
async function loadBalances() {
  const currentVersion = ++balanceLoadVersion
  try {
    const opsProviderIds = providers.value
      .filter(p => p.ops_configured)
      .map(p => p.id)
    if (opsProviderIds.length === 0) return

    const results = await batchQueryBalance(opsProviderIds)

    // 检查是否有新的请求已经开始，如果有则丢弃当前结果
    if (currentVersion !== balanceLoadVersion) return

    // 将成功的结果存入缓存
    for (const [providerId, result] of Object.entries(results)) {
      if (result.status === 'success') {
        balanceCache.value[providerId] = result
      }
    }
  } catch (e) {
    console.warn('[loadBalances] 加载余额数据失败:', e)
  }
}

/**
 * 类型守卫：检查是否为 BalanceInfo（简化版）
 * 只检查余额显示所需的字段，完整的 BalanceInfo 还包含 total_granted, total_used, expires_at, extra
 */
function isBalanceInfo(data: unknown): data is { total_available: number | null; currency: string } {
  if (typeof data !== 'object' || data === null) return false
  if (!('total_available' in data) || !('currency' in data)) return false
  const d = data as Record<string, unknown>
  // total_available 必须是 number 或 null
  if (d.total_available !== null && typeof d.total_available !== 'number') return false
  // currency 必须是 string
  if (typeof d.currency !== 'string') return false
  return true
}

// 获取 provider 的余额显示
function getProviderBalance(providerId: string): { available: number | null; currency: string } | null {
  const result = balanceCache.value[providerId]
  // auth_expired 时余额数据仍有效（只是签到 Cookie 失效）
  if (!result || (result.status !== 'success' && result.status !== 'auth_expired') || !result.data) {
    return null
  }
  if (!isBalanceInfo(result.data)) {
    return null
  }
  return {
    available: result.data.total_available,
    currency: result.data.currency || 'USD'
  }
}

// 获取 provider 余额查询的错误状态
function getProviderBalanceError(providerId: string): { status: string; message: string } | null {
  const result = balanceCache.value[providerId]
  if (!result) {
    return null
  }
  // 认证失败或过期
  if (result.status === 'auth_failed' || result.status === 'auth_expired') {
    return {
      status: result.status,
      message: result.message || '认证失败'
    }
  }
  // 其他错误
  if (result.status !== 'success') {
    return {
      status: result.status,
      message: result.message || '查询失败'
    }
  }
  return null
}

// 获取 provider 的签到信息（从 extra 字段）
function getProviderCheckin(providerId: string): { success: boolean | null; message: string } | null {
  const result = balanceCache.value[providerId]
  if (!result || result.status !== 'success' || !result.data) {
    return null
  }
  const data = result.data as Record<string, any>
  const extra = data.extra
  if (!extra || extra.checkin_success === undefined) {
    return null
  }
  return {
    success: extra.checkin_success,
    message: extra.checkin_message || ''
  }
}

// 获取 provider 的 Cookie 失效状态（从 extra 字段）
function getProviderCookieExpired(providerId: string): { expired: boolean; message: string } | null {
  const result = balanceCache.value[providerId]
  if (!result || !result.data) {
    return null
  }
  // 支持 status 为 'success' 或 'auth_expired'（Cookie 失效时状态会变为 auth_expired）
  if (result.status !== 'success' && result.status !== 'auth_expired') {
    return null
  }
  const data = result.data as Record<string, any>
  const extra = data.extra
  if (!extra || !extra.cookie_expired) {
    return null
  }
  return {
    expired: true,
    message: extra.cookie_expired_message || 'Cookie 已失效'
  }
}

// 格式化余额显示
function formatBalanceDisplay(balance: { available: number | null; currency: string } | null): string {
  if (!balance || balance.available == null) {
    return '-'
  }
  const symbol = balance.currency === 'USD' ? '$' : balance.currency
  return `${symbol}${balance.available.toFixed(2)}`
}

// 格式化重置倒计时（从 Unix 时间戳）
function formatResetCountdown(resetsAt: number): string {
  // 依赖 tickCounter 触发响应式更新
  void tickCounter.value

  const now = Date.now() / 1000
  const diff = resetsAt - now

  if (diff <= 0) return '即将重置'

  const totalHours = Math.floor(diff / 3600)
  const minutes = Math.floor((diff % 3600) / 60)
  const seconds = Math.floor(diff % 60)

  const pad = (n: number) => n.toString().padStart(2, '0')

  if (totalHours > 0) {
    return `${totalHours}:${pad(minutes)}:${pad(seconds)}`
  }
  return `${minutes}:${pad(seconds)}`
}

// 获取 provider 余额的额外信息（如窗口限额）
function getProviderBalanceExtra(providerId: string, architectureId?: string): BalanceExtraItem[] {
  if (!architectureId) return []

  const result = balanceCache.value[providerId]
  // auth_expired 时余额数据仍有效（只是签到 Cookie 失效）
  if (!result || (result.status !== 'success' && result.status !== 'auth_expired') || !result.data) {
    return []
  }

  const data = result.data as Record<string, any>
  const extra = data.extra
  if (!extra) return []

  // 获取对应的模板
  const template = authTemplateRegistry.get(architectureId)
  if (!template?.formatBalanceExtra) return []

  return template.formatBalanceExtra(extra)
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
  // 检查端点是否启用，以及是否有活跃的密钥
  if (endpoint.is_active === false) {
    return false
  }
  return (endpoint.active_keys ?? 0) > 0
}

// 端点标签样式
function getEndpointTagClass(endpoint: any, provider: ProviderWithEndpointsSummary): string {
  if (!isEndpointAvailable(endpoint, provider)) {
    return 'border-red-300/50 bg-red-50/50 text-red-600/80 dark:border-red-500/30 dark:bg-red-500/10 dark:text-red-400/80'
  }
  return 'border-border/40 bg-muted/20 text-foreground/70'
}

// 进度条颜色
function getEndpointDotColor(endpoint: any, provider: ProviderWithEndpointsSummary): string {
  if (!isEndpointAvailable(endpoint, provider)) {
    return 'bg-red-400/50'
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
  if (endpoint.is_active === false) {
    return `${endpoint.api_format}: 端点已禁用`
  }
  if (endpoint.active_keys === 0) {
    // 区分：有密钥但全部禁用 vs 未配置任何密钥
    if ((endpoint.total_keys ?? 0) > 0) {
      return `${endpoint.api_format}: 无可用密钥`
    }
    return `${endpoint.api_format}: 未配置密钥`
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

// 打开扩展操作配置对话框
function openOpsConfigDialog(provider: ProviderWithEndpointsSummary) {
  opsConfigProviderId.value = provider.id
  opsConfigProviderWebsite.value = provider.website || ''
  opsConfigDialogOpen.value = true
}

// 扩展操作配置保存回调
function handleOpsConfigSaved() {
  opsConfigDialogOpen.value = false
  loadProviders()
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
    `确定要删除提供商 "${provider.name}" 吗？\n\n这将同时删除其所有端点、密钥和配置。此操作不可恢复！`
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

// 用于触发倒计时更新的响应式计数器
const tickCounter = ref(0)
let tickInterval: ReturnType<typeof setInterval> | null = null

onMounted(() => {
  loadProviders()
  loadPriorityMode()
  // 每秒更新一次倒计时
  tickInterval = setInterval(() => {
    tickCounter.value++
  }, 1000)
})

onUnmounted(() => {
  if (tickInterval) {
    clearInterval(tickInterval)
  }
})
</script>
