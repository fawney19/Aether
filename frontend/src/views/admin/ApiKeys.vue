<template>
  <div class="space-y-6 pb-8">
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 加载状态 -->
      <div
        v-if="loading"
        class="py-16 text-center space-y-4"
      >
        <Skeleton class="mx-auto h-10 w-10 rounded-full" />
        <Skeleton class="mx-auto h-4 w-32" />
      </div>

      <div v-else>
        <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
          <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
            <div class="shrink-0">
              <h3 class="text-sm sm:text-base font-semibold">
                独立余额 API Keys
              </h3>
            </div>
            <div class="flex flex-wrap items-center gap-2">
              <!-- 搜索框 -->
              <div class="relative">
                <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
                <Input
                  v-model="searchQuery"
                  type="text"
                  placeholder="搜索..."
                  class="h-8 w-28 sm:w-40 pl-8 pr-2 text-xs"
                />
              </div>

              <!-- 分隔线 -->
              <div class="hidden sm:block h-4 w-px bg-border" />

              <!-- 状态筛选 -->
              <Select
                v-model="filterStatus"
              >
                <SelectTrigger class="w-20 sm:w-28 h-8 text-xs border-border/60">
                  <SelectValue placeholder="全部状态" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="status in statusFilters"
                    :key="status.value"
                    :value="status.value"
                  >
                    {{ status.label }}
                  </SelectItem>
                </SelectContent>
              </Select>

              <!-- 余额类型筛选 -->
              <Select
                v-model="filterBalance"
              >
                <SelectTrigger class="w-20 sm:w-28 h-8 text-xs border-border/60">
                  <SelectValue placeholder="全部类型" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-for="balance in balanceFilters"
                    :key="balance.value"
                    :value="balance.value"
                  >
                    {{ balance.label }}
                  </SelectItem>
                </SelectContent>
              </Select>

              <!-- 分隔线 -->
              <div class="hidden sm:block h-4 w-px bg-border" />

              <!-- 创建独立 Key 按钮 -->
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="创建独立 Key"
                @click="openCreateDialog"
              >
                <Plus class="w-3.5 h-3.5" />
              </Button>

              <!-- 刷新按钮 -->
              <RefreshButton
                :loading="loading"
                @click="refreshApiKeys"
              />
            </div>
          </div>
        </div>

        <div class="hidden xl:block overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow class="border-b border-border/60 hover:bg-transparent">
                <TableHead class="w-[200px] h-12 font-semibold">
                  密钥信息
                </TableHead>
                <TableHead class="w-[240px] h-12 font-semibold">
                  钱包
                </TableHead>
                <TableHead class="w-[130px] h-12 font-semibold">
                  使用统计
                </TableHead>
                <TableHead class="w-[110px] h-12 font-semibold">
                  有效期
                </TableHead>
                <TableHead class="w-[140px] h-12 font-semibold">
                  最近使用
                </TableHead>
                <TableHead class="w-[180px] h-12 font-semibold">
                  状态
                </TableHead>
                <TableHead class="w-[130px] h-12 font-semibold text-center">
                  操作
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow v-if="filteredApiKeys.length === 0">
                <TableCell
                  colspan="7"
                  class="h-64 text-center"
                >
                  <div class="flex flex-col items-center justify-center space-y-4">
                    <div class="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-muted">
                      <Key class="h-8 w-8 text-muted-foreground" />
                    </div>
                    <div v-if="hasActiveFilters">
                      <h3 class="text-lg font-semibold">
                        未找到匹配的 Key
                      </h3>
                      <p class="mt-2 text-sm text-muted-foreground">
                        尝试调整筛选条件
                      </p>
                      <Button
                        variant="outline"
                        size="sm"
                        class="mt-3"
                        @click="clearFilters"
                      >
                        清除筛选
                      </Button>
                    </div>
                    <div v-else>
                      <h3 class="text-lg font-semibold">
                        暂无独立余额 Key
                      </h3>
                      <p class="mt-2 text-sm text-muted-foreground">
                        点击右上角按钮创建独立余额 Key
                      </p>
                    </div>
                  </div>
                </TableCell>
              </TableRow>
              <TableRow
                v-for="apiKey in filteredApiKeys"
                :key="apiKey.id"
                class="border-b border-border/40 hover:bg-muted/30 transition-colors"
              >
                <TableCell class="py-4">
                  <div class="space-y-1">
                    <div
                      class="text-sm font-semibold text-foreground truncate"
                      :title="apiKey.name || '未命名 Key'"
                    >
                      {{ apiKey.name || '未命名 Key' }}
                    </div>
                    <div class="flex items-center gap-1.5">
                      <code class="text-xs font-mono text-muted-foreground">
                        {{ apiKey.key_display || 'sk-****' }}
                      </code>
                      <Button
                        variant="ghost"
                        size="icon"
                        class="h-6 w-6"
                        title="复制完整密钥"
                        @click="copyKeyPrefix(apiKey)"
                      >
                        <Copy class="h-3 w-3" />
                      </Button>
                    </div>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="space-y-1.5">
                    <div class="flex items-center gap-1 text-[11px] text-muted-foreground">
                      <span>余额：</span>
                      <Badge
                        v-if="isApiKeyUnlimited(apiKey)"
                        variant="secondary"
                        class="h-5 px-1.5 py-0 text-[10px] font-medium"
                      >
                        无限额度
                      </Badge>
                      <span
                        v-else
                        class="text-sm font-semibold tabular-nums"
                        :class="isNegativeWalletAmount(getApiKeyWalletTotalBalance(apiKey)) ? 'text-rose-600' : 'text-foreground'"
                      >
                        {{ formatWalletAmount(getApiKeyWalletTotalBalance(apiKey), '-') }}
                      </span>
                    </div>
                    <div class="flex items-center gap-2 text-[11px] text-muted-foreground flex-wrap">
                      <span>
                        已消费：
                        <span class="font-medium tabular-nums text-foreground">${{ getApiKeyWalletConsumed(apiKey).toFixed(2) }}</span>
                      </span>
                    </div>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="space-y-1 text-xs">
                    <div class="text-muted-foreground">
                      请求: <span class="font-medium text-foreground">{{ (apiKey.total_requests || 0).toLocaleString() }}</span>
                    </div>
                    <div class="text-muted-foreground">
                      速率: <span class="font-medium text-foreground">{{ apiKey.rate_limit ? `${apiKey.rate_limit}/min` : '未设置' }}</span>
                    </div>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="text-xs">
                    <div
                      v-if="apiKey.expires_at"
                      class="space-y-1"
                    >
                      <div class="text-foreground">
                        {{ formatDate(apiKey.expires_at) }}
                      </div>
                      <div class="text-muted-foreground">
                        {{ getRelativeTime(apiKey.expires_at) }}
                      </div>
                    </div>
                    <div
                      v-else
                      class="text-muted-foreground"
                    >
                      永不过期
                    </div>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="text-xs">
                    <span
                      v-if="apiKey.last_used_at"
                      class="text-foreground"
                    >{{ formatDate(apiKey.last_used_at) }}</span>
                    <span
                      v-else
                      class="text-muted-foreground"
                    >暂无记录</span>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="flex flex-col items-start gap-1.5">
                    <Badge
                      :variant="apiKey.is_active ? 'success' : 'destructive'"
                      class="h-5 px-1.5 py-0 text-[10px] font-medium"
                    >
                      {{ apiKey.is_active ? '活跃' : '禁用' }}
                    </Badge>
                    <Badge
                      v-if="getApiKeyWallet(apiKey.id)"
                      :variant="walletStatusBadge(getApiKeyWalletStatus(apiKey.id))"
                      class="h-5 px-1.5 py-0 text-[10px] font-medium"
                    >
                      {{ walletStatusLabel(getApiKeyWalletStatus(apiKey.id)) }}
                    </Badge>
                  </div>
                </TableCell>
                <TableCell class="py-4">
                  <div class="flex justify-center gap-1">
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="编辑"
                      @click="editApiKey(apiKey)"
                    >
                      <SquarePen class="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="资金操作"
                      @click="openAddBalanceDialog(apiKey)"
                    >
                      <DollarSign class="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      :title="apiKey.is_active ? '禁用' : '启用'"
                      @click="toggleApiKey(apiKey)"
                    >
                      <Power class="h-4 w-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="删除"
                      @click="deleteApiKey(apiKey)"
                    >
                      <Trash2 class="h-4 w-4" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>

        <div class="xl:hidden bg-muted/[0.14] p-3 sm:p-4">
          <div
            v-if="filteredApiKeys.length === 0"
            class="rounded-2xl border border-dashed border-border/60 bg-card/70 px-6 py-10 text-center"
          >
            <Key class="mx-auto mb-3 h-12 w-12 text-muted-foreground/50" />
            <p class="text-sm font-medium text-foreground">
              {{ hasActiveFilters ? '未找到匹配的 Key' : '暂无独立余额 Key' }}
            </p>
            <p
              v-if="hasActiveFilters"
              class="mt-1 text-xs text-muted-foreground"
            >
              尝试调整筛选条件
            </p>
          </div>

          <div
            v-else
            class="space-y-3.5"
          >
            <div
              v-for="apiKey in filteredApiKeys"
              :key="apiKey.id"
              class="rounded-2xl border border-border/60 bg-card/95 p-4 shadow-[0_10px_26px_-22px_hsl(var(--foreground))]"
            >
              <div class="space-y-4">
                <div class="flex items-start gap-3">
                  <div class="min-w-0 flex-1 space-y-2">
                    <div class="flex items-center gap-2">
                      <code class="inline-flex max-w-[190px] sm:max-w-[240px] truncate rounded-lg bg-muted px-3 py-1.5 text-[11px] font-mono font-semibold text-foreground/90">
                        {{ apiKey.key_display || 'sk-****' }}
                      </code>
                      <Button
                        variant="ghost"
                        size="icon"
                        class="h-7 w-7 flex-shrink-0 hover:bg-muted"
                        title="复制完整密钥"
                        @click="copyKeyPrefix(apiKey)"
                      >
                        <Copy class="h-3.5 w-3.5" />
                      </Button>
                    </div>
                    <div
                      class="truncate text-sm font-semibold text-foreground"
                      :class="{ 'text-muted-foreground': !apiKey.name }"
                      :title="apiKey.name || '未命名 Key'"
                    >
                      {{ apiKey.name || '未命名 Key' }}
                    </div>
                  </div>
                </div>

                <div class="flex flex-wrap items-center gap-1.5">
                  <Badge
                    :variant="apiKey.is_active ? 'success' : 'destructive'"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    {{ apiKey.is_active ? '活跃' : '禁用' }}
                  </Badge>
                  <Badge
                    v-if="getApiKeyWallet(apiKey.id)"
                    :variant="walletStatusBadge(getApiKeyWalletStatus(apiKey.id))"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    {{ walletStatusLabel(getApiKeyWalletStatus(apiKey.id)) }}
                  </Badge>
                  <Badge
                    v-if="apiKey.auto_delete_on_expiry"
                    variant="secondary"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    过期自动删除
                  </Badge>
                </div>

                <div class="rounded-xl border border-border/60 bg-muted/40 p-3.5">
                  <div class="flex items-start justify-between gap-3">
                    <div class="space-y-1">
                      <p class="text-[11px] text-muted-foreground">
                        余额：
                      </p>
                      <Badge
                        v-if="isApiKeyUnlimited(apiKey)"
                        variant="secondary"
                        class="h-5 px-1.5 py-0 text-[10px] font-medium"
                      >
                        无限额度
                      </Badge>
                      <p
                        v-else
                        class="text-base font-semibold tabular-nums leading-none"
                        :class="isNegativeWalletAmount(getApiKeyWalletTotalBalance(apiKey)) ? 'text-rose-600' : 'text-foreground'"
                      >
                        {{ formatWalletAmount(getApiKeyWalletTotalBalance(apiKey), '-') }}
                      </p>
                    </div>
                    <div class="text-right">
                      <p class="text-[11px] text-muted-foreground">
                        已消费：
                      </p>
                      <p class="text-sm font-medium tabular-nums text-foreground">
                        ${{ getApiKeyWalletConsumed(apiKey).toFixed(2) }}
                      </p>
                    </div>
                  </div>
                </div>

                <div class="grid grid-cols-2 gap-2.5 text-xs">
                  <div class="rounded-lg border border-border/50 bg-background/70 p-2.5">
                    <div class="mb-1 text-muted-foreground">
                      速率限制
                    </div>
                    <div class="font-semibold text-foreground">
                      {{ apiKey.rate_limit ? `${apiKey.rate_limit}/min` : '未设置' }}
                    </div>
                  </div>
                  <div class="rounded-lg border border-border/50 bg-background/70 p-2.5">
                    <div class="mb-1 text-muted-foreground">
                      请求次数
                    </div>
                    <div class="font-semibold text-foreground">
                      {{ (apiKey.total_requests || 0).toLocaleString() }}
                    </div>
                  </div>
                  <div class="col-span-2 rounded-lg border border-border/50 bg-background/70 p-2.5">
                    <div class="mb-1 text-muted-foreground">
                      有效期
                    </div>
                    <div class="font-semibold text-foreground">
                      {{ apiKey.expires_at ? formatDate(apiKey.expires_at) : '永不过期' }}
                    </div>
                    <div
                      v-if="apiKey.expires_at"
                      class="text-[11px] text-muted-foreground"
                    >
                      {{ getRelativeTime(apiKey.expires_at) }}
                    </div>
                  </div>
                </div>

                <div class="rounded-lg bg-muted/35 p-2.5 text-[11px] text-muted-foreground">
                  <div class="flex items-center justify-between gap-2">
                    <span>创建</span>
                    <span class="font-medium text-foreground">{{ formatDate(apiKey.created_at) }}</span>
                  </div>
                  <div class="mt-1 flex items-center justify-between gap-2">
                    <span>最近使用</span>
                    <span
                      v-if="apiKey.last_used_at"
                      class="font-medium text-foreground"
                    >{{ formatDate(apiKey.last_used_at) }}</span>
                    <span v-else>暂无记录</span>
                  </div>
                  <div
                    v-if="apiKey.expires_at"
                    class="mt-1 flex items-center justify-between gap-2"
                  >
                    <span>过期后</span>
                    <span>{{ apiKey.auto_delete_on_expiry ? '自动删除' : '仅禁用' }}</span>
                  </div>
                </div>

                <div class="grid grid-cols-2 gap-2 pt-0.5">
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 text-xs"
                    @click="editApiKey(apiKey)"
                  >
                    <SquarePen class="mr-1.5 h-3.5 w-3.5" />
                    编辑
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 text-xs"
                    @click="openAddBalanceDialog(apiKey)"
                  >
                    <DollarSign class="mr-1.5 h-3.5 w-3.5" />
                    资金
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    class="h-8 text-xs"
                    @click="toggleApiKey(apiKey)"
                  >
                    <Power class="mr-1.5 h-3.5 w-3.5" />
                    {{ apiKey.is_active ? '禁用' : '启用' }}
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    class="col-span-2 h-8 border-rose-200 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                    @click="deleteApiKey(apiKey)"
                  >
                    <Trash2 class="mr-1.5 h-3.5 w-3.5" />
                    删除
                  </Button>
                </div>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="!loading && apiKeys.length > 0"
        :current="currentPage"
        :total="total"
        :page-size="limit"
        :show-page-size-selector="false"
        @update:current="handlePageChange"
      />
    </Card>

    <!-- 创建/编辑独立Key对话框 -->
    <StandaloneKeyFormDialog
      ref="keyFormDialogRef"
      :open="showKeyFormDialog"
      :api-key="editingKeyData"
      @close="closeKeyFormDialog"
      @submit="handleKeyFormSubmit"
    />

    <!-- 新 Key 显示对话框 -->
    <Dialog
      v-model="showNewKeyDialog"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-emerald-100 dark:bg-emerald-900/30 flex-shrink-0">
              <CheckCircle class="h-5 w-5 text-emerald-600 dark:text-emerald-400" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                创建成功
              </h3>
              <p class="text-xs text-muted-foreground">
                请妥善保管, 切勿泄露给他人.
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label class="text-sm font-medium">API Key</Label>
          <div class="flex items-center gap-2">
            <Input
              ref="keyInput"
              type="text"
              :value="newKeyValue"
              readonly
              class="flex-1 font-mono text-sm bg-muted/50 h-11"
              @click="selectKey"
            />
            <Button
              class="h-11"
              @click="copyKey"
            >
              复制
            </Button>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          class="h-10 px-5"
          @click="closeNewKeyDialog"
        >
          确定
        </Button>
      </template>
    </Dialog>

    <WalletOpsDrawer
      :open="showWalletActionDrawer"
      :wallet="walletActionTarget?.wallet || null"
      :owner-name="walletActionTarget?.apiKey.name || walletActionTarget?.apiKey.key_display || '未命名 Key'"
      :owner-subtitle="walletActionTarget?.apiKey.key_display || walletActionTarget?.apiKey.username || ''"
      context-label="独立密钥钱包"
      accent="blue"
      :show-refunds="false"
      @close="closeWalletActionDrawer"
      @changed="handleWalletDrawerChanged"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useClipboard } from '@/composables/useClipboard'
import { adminApi, type AdminApiKey, type CreateStandaloneApiKeyRequest } from '@/api/admin'
import { adminWalletApi, type AdminWallet } from '@/api/admin-wallets'
import { walletStatusBadge, walletStatusLabel } from '@/utils/walletDisplay'
import WalletOpsDrawer from '@/features/wallet/components/WalletOpsDrawer.vue'

import {
  Dialog,
  Card,
  Button,
  Badge,
  Input,
  Skeleton,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Pagination,
  RefreshButton,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Label
} from '@/components/ui'

import {
  Plus,
  Key,
  Trash2,
  Power,
  DollarSign,
  Copy,
  CheckCircle,
  SquarePen,
  Search
} from 'lucide-vue-next'

import { StandaloneKeyFormDialog, type StandaloneKeyFormData } from '@/features/api-keys'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

const { success, error } = useToast()
const { confirmDanger } = useConfirm()
const { copyToClipboard } = useClipboard()

const apiKeys = ref<AdminApiKey[]>([])
const apiKeyWalletMap = ref<Record<string, AdminWallet>>({})
const loading = ref(false)
const total = ref(0)
const currentPage = ref(1)
const limit = ref(100)
const showNewKeyDialog = ref(false)
const newKeyValue = ref('')
const keyInput = ref<HTMLInputElement>()

// 统一的表单对话框状态
const showKeyFormDialog = ref(false)
const editingKeyData = ref<StandaloneKeyFormData | null>(null)
const keyFormDialogRef = ref<InstanceType<typeof StandaloneKeyFormDialog>>()

const EXPIRY_SOON_DAYS = 7

// 筛选相关
const searchQuery = ref('')
const filterStatus = ref<'all' | 'active' | 'inactive'>('all')
const filterBalance = ref<'all' | 'limited' | 'unlimited'>('all')

const statusFilters = [
  { value: 'all' as const, label: '全部状态' },
  { value: 'active' as const, label: '活跃' },
  { value: 'inactive' as const, label: '禁用' }
]

const balanceFilters = [
  { value: 'all' as const, label: '全部类型' },
  { value: 'limited' as const, label: '限额' },
  { value: 'unlimited' as const, label: '无限' }
]

const hasActiveFilters = computed(() => {
  return searchQuery.value !== '' || filterStatus.value !== 'all' || filterBalance.value !== 'all'
})

function clearFilters() {
  searchQuery.value = ''
  filterStatus.value = 'all'
  filterBalance.value = 'all'
}

const skip = computed(() => (currentPage.value - 1) * limit.value)

const activeKeyCount = computed(() => apiKeys.value.filter(key => key.is_active).length)
const _inactiveKeyCount = computed(() => Math.max(0, apiKeys.value.length - activeKeyCount.value))
const limitedKeyCount = computed(() => apiKeys.value.filter(isBalanceLimited).length)
const _unlimitedKeyCount = computed(() => Math.max(0, apiKeys.value.length - limitedKeyCount.value))
const _expiringSoonCount = computed(() => apiKeys.value.filter(key => isExpiringSoon(key)).length)

// 筛选后的 API Keys
const filteredApiKeys = computed(() => {
  let result = apiKeys.value

  // 搜索筛选（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    result = result.filter(key => {
      const searchableText = `${key.name || ''} ${key.key_display || ''} ${key.username || ''} ${key.user_email || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  // 状态筛选
  if (filterStatus.value === 'active') {
    result = result.filter(key => key.is_active)
  } else if (filterStatus.value === 'inactive') {
    result = result.filter(key => !key.is_active)
  }

  // 余额类型筛选
  if (filterBalance.value === 'limited') {
    result = result.filter(isBalanceLimited)
  } else if (filterBalance.value === 'unlimited') {
    result = result.filter(key => !isBalanceLimited(key))
  }

  return result
})

const showWalletActionDrawer = ref(false)
const walletActionTarget = ref<{ apiKey: AdminApiKey; wallet: AdminWallet } | null>(null)

onMounted(async () => {
  await refreshApiKeys()
})

async function loadApiKeys() {
  loading.value = true
  try {
    const response = await adminApi.getAllApiKeys({
      skip: skip.value,
      limit: limit.value
    })
    apiKeys.value = response.api_keys
    total.value = response.total
  } catch (err: unknown) {
    log.error('加载独立Keys失败:', err)
    error(parseApiError(err, '加载独立 Keys 失败'))
  } finally {
    loading.value = false
  }
}

async function loadApiKeyWallets() {
  try {
    const wallets = await adminWalletApi.listAllWallets()
    apiKeyWalletMap.value = wallets
      .filter((wallet) => wallet.owner_type === 'api_key' && !!wallet.api_key_id)
      .reduce<Record<string, AdminWallet>>((acc, wallet) => {
        acc[wallet.api_key_id as string] = wallet
        return acc
      }, {})
  } catch (err: unknown) {
    log.error('加载独立 Key 钱包失败:', err)
  }
}

async function refreshApiKeys() {
  // 先拉取 Key 列表，再拉钱包，避免并发请求导致新钱包映射短暂缺失。
  await loadApiKeys()
  await loadApiKeyWallets()
}

function handlePageChange(page: number) {
  currentPage.value = page
  refreshApiKeys()
}

async function toggleApiKey(apiKey: AdminApiKey) {
  try {
    const response = await adminApi.toggleApiKey(apiKey.id)
    const index = apiKeys.value.findIndex(k => k.id === apiKey.id)
    if (index !== -1) {
      apiKeys.value[index].is_active = response.is_active
    }
    success(response.message)
  } catch (err: unknown) {
    log.error('切换密钥状态失败:', err)
    error(parseApiError(err, '操作失败'))
  }
}

async function deleteApiKey(apiKey: AdminApiKey) {
  const confirmed = await confirmDanger(
    `确定要删除这个独立余额 Key 吗？\n\n${apiKey.name || apiKey.key_display || 'sk-****'}\n\n此操作无法撤销。`,
    '删除独立 Key'
  )

  if (!confirmed) return

  try {
    const response = await adminApi.deleteApiKey(apiKey.id)
    apiKeys.value = apiKeys.value.filter(k => k.id !== apiKey.id)
    total.value = total.value - 1
    delete apiKeyWalletMap.value[apiKey.id]
    success(response.message)
  } catch (err: unknown) {
    log.error('删除密钥失败:', err)
    error(parseApiError(err, '删除失败'))
  }
}

function editApiKey(apiKey: AdminApiKey) {
  // 解析过期日期为 YYYY-MM-DD 格式
  // 保留原始日期，不做时间过滤（避免编辑当天过期的 Key 时意外清空）
  let expiresAt: string | undefined = undefined

  if (apiKey.expires_at) {
    const expiresDate = new Date(apiKey.expires_at)
    expiresAt = expiresDate.toISOString().split('T')[0]
  }

  editingKeyData.value = {
    id: apiKey.id,
    name: apiKey.name || '',
    initial_balance_usd: isApiKeyUnlimited(apiKey) ? undefined : (getApiKeyWalletTotalBalance(apiKey) ?? undefined),
    unlimited_balance: isApiKeyUnlimited(apiKey),
    expires_at: expiresAt,
    rate_limit: apiKey.rate_limit ?? undefined,
    auto_delete_on_expiry: apiKey.auto_delete_on_expiry || false,
    allowed_providers: apiKey.allowed_providers || [],
    allowed_api_formats: apiKey.allowed_api_formats || [],
    allowed_models: apiKey.allowed_models || []
  }

  showKeyFormDialog.value = true
}

function getApiKeyWallet(apiKeyId: string): AdminWallet | null {
  return apiKeyWalletMap.value[apiKeyId] || null
}

function isApiKeyUnlimited(apiKey: AdminApiKey): boolean {
  const wallet = getApiKeyWallet(apiKey.id)
  return wallet?.limit_mode === 'unlimited' || wallet?.unlimited === true
}

function getApiKeyWalletTotalBalance(apiKey: AdminApiKey): number | null {
  if (isApiKeyUnlimited(apiKey)) {
    return null
  }
  const wallet = getApiKeyWallet(apiKey.id)
  return wallet ? wallet.balance : 0
}

function getApiKeyWalletConsumed(apiKey: AdminApiKey): number {
  return getApiKeyWallet(apiKey.id)?.total_consumed ?? (apiKey.total_cost_usd || 0)
}

function getApiKeyWalletStatus(apiKeyId: string): string | null {
  return getApiKeyWallet(apiKeyId)?.status ?? null
}

function formatWalletAmount(value: number | null, nullLabel = '无限制'): string {
  if (value == null) {
    return nullLabel
  }
  return `$${value.toFixed(2)}`
}

function isNegativeWalletAmount(value: number | null): boolean {
  return typeof value === 'number' && value < 0
}

function openAddBalanceDialog(apiKey: AdminApiKey) {
  const wallet = getApiKeyWallet(apiKey.id)
  if (!wallet) {
    error('该独立 Key 的钱包尚未初始化，暂时无法进行资金操作')
    return
  }

  walletActionTarget.value = {
    apiKey,
    wallet
  }
  showWalletActionDrawer.value = true
}

function closeWalletActionDrawer() {
  showWalletActionDrawer.value = false
}

async function handleWalletDrawerChanged() {
  await refreshApiKeys()
  if (!walletActionTarget.value) {
    return
  }

  const latestKey = apiKeys.value.find((item) => item.id === walletActionTarget.value?.apiKey.id)
  const latestWallet = getApiKeyWallet(walletActionTarget.value.apiKey.id)
  if (latestKey) {
    walletActionTarget.value.apiKey = latestKey
  }
  if (latestWallet) {
    walletActionTarget.value.wallet = latestWallet
  }
}

function selectKey() {
  keyInput.value?.select()
}

async function copyKey() {
  await copyToClipboard(newKeyValue.value)
}

async function copyKeyPrefix(apiKey: AdminApiKey) {
  try {
    // 调用后端 API 获取完整密钥
    const response = await adminApi.getFullApiKey(apiKey.id)
    await copyToClipboard(response.key)
  } catch (err) {
    log.error('复制密钥失败:', err)
    error('复制失败，请重试')
  }
}

function closeNewKeyDialog() {
  showNewKeyDialog.value = false
  newKeyValue.value = ''
}

function isBalanceLimited(apiKey: AdminApiKey): boolean {
  return !isApiKeyUnlimited(apiKey)
}

function isExpiringSoon(apiKey: AdminApiKey): boolean {
  if (!apiKey.expires_at) {
    return false
  }

  const expiresAt = new Date(apiKey.expires_at).getTime()
  const now = Date.now()
  const diffDays = (expiresAt - now) / (1000 * 60 * 60 * 24)
  return diffDays > 0 && diffDays <= EXPIRY_SOON_DAYS
}

function formatDate(dateString: string): string {
  return new Date(dateString).toLocaleString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
}

function getRelativeTime(dateString: string): string {
  const date = new Date(dateString)
  const now = new Date()
  const diff = date.getTime() - now.getTime()

  if (diff < 0) return '已过期'

  const days = Math.floor(diff / (1000 * 60 * 60 * 24))
  const hours = Math.floor(diff / (1000 * 60 * 60))

  if (days > 0) return `${days}天后过期`
  if (hours > 0) return `${hours}小时后过期`
  return '即将过期'
}

// ========== 统一表单对话框方法 ==========

// 打开创建对话框
function openCreateDialog() {
  editingKeyData.value = null
  showKeyFormDialog.value = true
}

// 关闭表单对话框
function closeKeyFormDialog() {
  showKeyFormDialog.value = false
  editingKeyData.value = null
}

// 统一处理表单提交
async function handleKeyFormSubmit(data: StandaloneKeyFormData) {
  // 验证过期日期（如果设置了，必须晚于今天）
  if (data.expires_at) {
    const selectedDate = new Date(data.expires_at)
    const today = new Date()
    today.setHours(0, 0, 0, 0)
    if (selectedDate <= today) {
      error('过期日期必须晚于今天')
      return
    }
  }

  keyFormDialogRef.value?.setSaving(true)
  try {
    if (data.id) {
      // 更新
      const updateData: Partial<CreateStandaloneApiKeyRequest> = {
        name: data.name || undefined,
        unlimited_balance: Boolean(data.unlimited_balance),
        rate_limit: data.rate_limit ?? null,  // undefined = 无限制，显式传 null
        expires_at: data.expires_at || null,  // undefined/空 = 永不过期
        auto_delete_on_expiry: data.auto_delete_on_expiry,
        // 空数组表示清除限制（允许全部），后端会将空数组存为 NULL
        allowed_providers: data.allowed_providers,
        allowed_api_formats: data.allowed_api_formats,
        allowed_models: data.allowed_models
      }
      const { message: _, wallet: __, ...updated } = await adminApi.updateApiKey(data.id, updateData)
      // 局部更新：合并字段，避免覆盖丢失列表已有信息
      const index = apiKeys.value.findIndex(k => k.id === data.id)
      if (index !== -1) {
        apiKeys.value[index] = {
          ...apiKeys.value[index],
          ...updated,
        }
      }
      await loadApiKeyWallets()
      success('API Key 更新成功')
    } else {
      // 创建
      const isUnlimited = Boolean(data.unlimited_balance)
      if (!isUnlimited && (!data.initial_balance_usd || data.initial_balance_usd <= 0)) {
        error('初始余额必须大于 0')
        return
      }
      const createData: CreateStandaloneApiKeyRequest = {
        name: data.name || undefined,
        initial_balance_usd: isUnlimited ? null : (data.initial_balance_usd as number),
        rate_limit: data.rate_limit ?? null,  // undefined = 无限制，显式传 null
        expires_at: data.expires_at || null,  // undefined/空 = 永不过期
        auto_delete_on_expiry: data.auto_delete_on_expiry,
        // 空数组表示不设置限制（允许全部），后端会将空数组存为 NULL
        allowed_providers: data.allowed_providers,
        allowed_api_formats: data.allowed_api_formats,
        allowed_models: data.allowed_models
      }
      const response = await adminApi.createStandaloneApiKey(createData)
      newKeyValue.value = response.key
      showNewKeyDialog.value = true
      success('独立 Key 创建成功')
      await refreshApiKeys()
    }
    closeKeyFormDialog()
  } catch (err: unknown) {
    log.error('保存独立Key失败:', err)
    error(parseApiError(err, '保存失败'))
  } finally {
    keyFormDialogRef.value?.setSaving(false)
  }
}
</script>
