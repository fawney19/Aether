<template>
  <div class="space-y-6 pb-8">
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- Header -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
        <!-- Mobile -->
        <div class="flex flex-col gap-3 sm:hidden">
          <div class="flex items-center justify-between">
            <h3 class="text-base font-semibold">
              号池管理
            </h3>
            <div class="flex items-center gap-1.5">
              <Button
                v-if="selectedProviderId"
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="批量导入"
                @click="showImportDialog = true"
              >
                <Upload class="w-3.5 h-3.5" />
              </Button>
              <Button
                v-if="selectedProviderId"
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="号池配置"
                @click="showConfigDialog = true"
              >
                <Settings class="w-3.5 h-3.5" />
              </Button>
              <RefreshButton
                :loading="overviewLoading || keysLoading"
                @click="refresh"
              />
            </div>
          </div>
          <!-- Filters (mobile) -->
          <div class="flex items-center gap-2">
            <Select v-model="selectedProviderIdProxy">
              <SelectTrigger class="flex-1 h-8 text-xs border-border/60">
                <SelectValue placeholder="选择 Provider" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="item in poolProviders"
                  :key="item.provider_id"
                  :value="item.provider_id"
                >
                  {{ item.provider_name }}
                  <span class="text-muted-foreground ml-1">({{ item.total_keys }})</span>
                </SelectItem>
              </SelectContent>
            </Select>
            <Select v-model="statusFilter">
              <SelectTrigger class="w-24 h-8 text-xs border-border/60">
                <SelectValue placeholder="状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部
                </SelectItem>
                <SelectItem value="active">
                  活跃
                </SelectItem>
                <SelectItem value="cooldown">
                  冷却中
                </SelectItem>
                <SelectItem value="inactive">
                  已禁用
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div
            v-if="selectedProviderId"
            class="relative"
          >
            <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
            <Input
              v-model="searchQuery"
              type="text"
              placeholder="搜索账号..."
              class="w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
            />
          </div>
        </div>

        <!-- Desktop -->
        <div class="hidden sm:flex items-center justify-between gap-4">
          <h3 class="text-base font-semibold">
            号池管理
          </h3>
          <div class="flex items-center gap-2">
            <Select v-model="selectedProviderIdProxy">
              <SelectTrigger class="w-44 h-8 text-xs border-border/60">
                <SelectValue placeholder="选择 Provider" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="item in poolProviders"
                  :key="item.provider_id"
                  :value="item.provider_id"
                >
                  {{ item.provider_name }}
                  <span class="text-muted-foreground ml-1">({{ item.total_keys }})</span>
                </SelectItem>
              </SelectContent>
            </Select>
            <div class="h-4 w-px bg-border" />
            <div
              v-if="selectedProviderId"
              class="relative"
            >
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                v-model="searchQuery"
                type="text"
                placeholder="搜索账号..."
                class="w-40 pl-8 pr-2 h-8 text-xs bg-background/50 border-border/60"
              />
            </div>
            <Select v-model="statusFilter">
              <SelectTrigger class="w-28 h-8 text-xs border-border/60">
                <SelectValue placeholder="全部状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部状态
                </SelectItem>
                <SelectItem value="active">
                  活跃
                </SelectItem>
                <SelectItem value="cooldown">
                  冷却中
                </SelectItem>
                <SelectItem value="inactive">
                  已禁用
                </SelectItem>
              </SelectContent>
            </Select>
            <div
              v-if="selectedProviderId"
              class="h-4 w-px bg-border"
            />
            <Button
              v-if="selectedProviderId"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="批量导入"
              @click="showImportDialog = true"
            >
              <Upload class="w-3.5 h-3.5" />
            </Button>
            <Button
              v-if="selectedProviderId"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="号池配置"
              @click="showConfigDialog = true"
            >
              <Settings class="w-3.5 h-3.5" />
            </Button>
            <RefreshButton
              :loading="overviewLoading || keysLoading"
              @click="refresh"
            />
          </div>
        </div>
      </div>

      <!-- Loading (initial) -->
      <div
        v-if="overviewLoading"
        class="flex items-center justify-center py-16"
      >
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
      </div>

      <!-- No providers -->
      <div
        v-else-if="poolProviders.length === 0"
        class="flex flex-col items-center justify-center py-16 text-center"
      >
        <div class="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-muted">
          <Database class="h-8 w-8 text-muted-foreground" />
        </div>
        <p class="text-sm text-muted-foreground mt-4">
          暂无 Provider
        </p>
        <p class="text-xs text-muted-foreground mt-1">
          请先添加 Provider
        </p>
      </div>

      <!-- No provider selected -->
      <div
        v-else-if="!selectedProviderId"
        class="flex flex-col items-center justify-center py-16 text-center"
      >
        <div class="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-muted">
          <Database class="h-8 w-8 text-muted-foreground" />
        </div>
        <p class="text-sm text-muted-foreground mt-4">
          请选择一个 Provider 查看账号
        </p>
      </div>

      <!-- Loading keys -->
      <div
        v-else-if="keysLoading && keyPage.keys.length === 0"
        class="flex items-center justify-center py-16"
      >
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
      </div>

      <template v-else>
        <!-- Batch action bar -->
        <div
          v-if="selectedKeys.size > 0"
          class="flex items-center gap-2 px-4 sm:px-6 py-2.5 bg-muted/40 border-b border-border/40"
        >
          <span class="text-xs font-medium text-muted-foreground mr-1">
            已选 {{ selectedKeys.size }} 个
          </span>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-xs"
            @click="batchAction('enable')"
          >
            启用
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-xs"
            @click="batchAction('disable')"
          >
            禁用
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-xs"
            @click="batchAction('clear_cooldown')"
          >
            清除冷却
          </Button>
          <Button
            variant="outline"
            size="sm"
            class="h-7 text-xs"
            @click="batchAction('reset_cost')"
          >
            重置成本
          </Button>
          <Button
            variant="destructive"
            size="sm"
            class="h-7 text-xs"
            @click="batchAction('delete')"
          >
            删除
          </Button>
        </div>

        <!-- Desktop table -->
        <div
          v-if="keyPage.keys.length > 0"
          class="hidden xl:block overflow-x-auto"
        >
          <Table>
            <TableHeader>
              <TableRow class="border-b border-border/60 hover:bg-transparent">
                <TableHead class="w-10 h-12">
                  <input
                    type="checkbox"
                    :checked="allSelected"
                    class="rounded"
                    @change="toggleSelectAll"
                  >
                </TableHead>
                <TableHead class="font-semibold">
                  名称
                </TableHead>
                <TableHead
                  v-if="showAccountQuotaColumn"
                  class="w-64 font-semibold"
                >
                  配额
                </TableHead>
                <TableHead class="w-20 font-semibold">
                  状态
                </TableHead>
                <TableHead class="w-32 font-semibold">
                  冷却
                </TableHead>
                <TableHead class="w-40 font-semibold">
                  成本
                </TableHead>
                <TableHead class="w-16 font-semibold text-center">
                  会话
                </TableHead>
                <TableHead class="w-24 font-semibold">
                  最后使用
                </TableHead>
                <TableHead class="w-20 font-semibold text-center">
                  操作
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow
                v-for="key in keyPage.keys"
                :key="key.key_id"
                class="border-b border-border/40 last:border-b-0 hover:bg-muted/30 transition-colors"
                :class="{ 'opacity-50': !key.is_active }"
              >
                <TableCell class="py-3">
                  <input
                    type="checkbox"
                    :checked="selectedKeys.has(key.key_id)"
                    class="rounded"
                    @change="toggleSelect(key.key_id)"
                  >
                </TableCell>
                <TableCell class="py-3">
                  <div class="flex items-center gap-1.5 min-w-0">
                    <span class="text-sm truncate max-w-[200px] block">
                      {{ key.key_name || '未命名' }}
                    </span>
                    <Badge
                      v-if="shouldShowPlanTypeBadge(key)"
                      variant="outline"
                      class="text-[10px] px-1.5 py-0 shrink-0"
                      :class="getOAuthPlanTypeClass(key.oauth_plan_type)"
                    >
                      {{ formatPoolOAuthPlanType(key.oauth_plan_type) }}
                    </Badge>
                  </div>
                </TableCell>
                <TableCell
                  v-if="showAccountQuotaColumn"
                  class="py-3"
                >
                  <div
                    v-if="quotaProgressMap[key.key_id]?.length"
                    class="flex items-stretch gap-2 overflow-x-auto pb-0.5"
                  >
                    <div
                      v-for="(item, idx) in quotaProgressMap[key.key_id]"
                      :key="`${key.key_id}-quota-${idx}`"
                      class="min-w-[140px] max-w-[180px] shrink-0"
                    >
                      <div class="flex items-center justify-between text-[10px] mb-0.5">
                        <span class="text-muted-foreground">{{ item.label }}</span>
                        <span :class="getQuotaRemainingClassByRemaining(item.remainingPercent)">
                          {{ item.remainingPercent.toFixed(1) }}%
                        </span>
                      </div>
                      <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                        <div
                          class="absolute left-0 top-0 h-full transition-all duration-300"
                          :class="getQuotaRemainingBarColorByRemaining(item.remainingPercent)"
                          :style="{ width: `${item.remainingPercent}%` }"
                        />
                      </div>
                      <div
                        v-if="item.detail"
                        class="text-[9px] text-muted-foreground/70 mt-0.5 truncate"
                      >
                        {{ item.detail }}
                      </div>
                    </div>
                  </div>
                  <span
                    v-else-if="key.account_quota"
                    :class="getQuotaTextClass(key.account_quota)"
                  >
                    {{ key.account_quota }}
                  </span>
                  <span
                    v-else
                    class="text-xs text-muted-foreground"
                  >-</span>
                </TableCell>
                <TableCell class="py-3">
                  <Badge
                    :variant="key.is_active ? (key.cooldown_reason ? 'destructive' : 'default') : 'secondary'"
                    class="text-[10px]"
                  >
                    {{ key.is_active ? (key.cooldown_reason ? '冷却' : '活跃') : '禁用' }}
                  </Badge>
                </TableCell>
                <TableCell class="py-3">
                  <template v-if="key.cooldown_reason">
                    <div class="flex items-center gap-1">
                      <span class="text-xs text-destructive">
                        {{ formatCooldownReason(key.cooldown_reason) }}
                      </span>
                      <span
                        v-if="key.cooldown_ttl_seconds"
                        class="text-[10px] text-muted-foreground"
                      >
                        {{ formatTTL(key.cooldown_ttl_seconds) }}
                      </span>
                    </div>
                  </template>
                  <span
                    v-else
                    class="text-xs text-muted-foreground"
                  >-</span>
                </TableCell>
                <TableCell class="py-3">
                  <div
                    v-if="key.cost_limit != null"
                    class="flex items-center gap-2"
                  >
                    <div class="flex-1 h-1.5 bg-border rounded-full overflow-hidden max-w-[80px]">
                      <div
                        class="h-full transition-all duration-300 rounded-full"
                        :class="getCostBarColor(key.cost_window_usage, key.cost_limit)"
                        :style="{ width: `${Math.min((key.cost_window_usage / key.cost_limit) * 100, 100)}%` }"
                      />
                    </div>
                    <span class="text-[10px] tabular-nums text-muted-foreground whitespace-nowrap">
                      {{ formatTokens(key.cost_window_usage) }}/{{ formatTokens(key.cost_limit) }}
                    </span>
                  </div>
                  <span
                    v-else-if="key.cost_window_usage > 0"
                    class="text-[10px] tabular-nums text-muted-foreground"
                  >
                    {{ formatTokens(key.cost_window_usage) }}
                  </span>
                  <span
                    v-else
                    class="text-xs text-muted-foreground"
                  >-</span>
                </TableCell>
                <TableCell class="py-3 text-center">
                  <span
                    v-if="key.sticky_sessions > 0"
                    class="text-xs tabular-nums"
                  >
                    {{ key.sticky_sessions }}
                  </span>
                  <span
                    v-else
                    class="text-xs text-muted-foreground"
                  >-</span>
                </TableCell>
                <TableCell class="py-3">
                  <span class="text-[10px] text-muted-foreground whitespace-nowrap">
                    {{ key.last_used_at ? formatRelativeTime(key.last_used_at) : '-' }}
                  </span>
                </TableCell>
                <TableCell class="py-3">
                  <div class="flex justify-center gap-0.5">
                    <Button
                      v-if="key.cooldown_reason"
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7 text-muted-foreground hover:text-green-600"
                      title="清除冷却"
                      @click="clearCooldown(key.key_id)"
                    >
                      <RefreshCw class="w-3.5 h-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7 text-muted-foreground hover:text-foreground"
                      :title="key.is_active ? '禁用' : '启用'"
                      @click="toggleKeyActive(key)"
                    >
                      <component
                        :is="key.is_active ? Ban : Check"
                        class="w-3.5 h-3.5"
                      />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>

        <!-- Mobile card list -->
        <div
          v-if="keyPage.keys.length > 0"
          class="xl:hidden divide-y divide-border/40"
        >
          <div
            v-for="key in keyPage.keys"
            :key="key.key_id"
            class="p-4 sm:p-5 hover:bg-muted/30 transition-colors"
            :class="{ 'opacity-50': !key.is_active }"
          >
            <div class="flex items-center gap-3">
              <input
                type="checkbox"
                :checked="selectedKeys.has(key.key_id)"
                class="rounded shrink-0"
                @change="toggleSelect(key.key_id)"
              >
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-sm font-medium truncate">
                    {{ key.key_name || '未命名' }}
                  </span>
                  <Badge
                    v-if="shouldShowPlanTypeBadge(key)"
                    variant="outline"
                    class="text-[10px] px-1.5 py-0 shrink-0"
                    :class="getOAuthPlanTypeClass(key.oauth_plan_type)"
                  >
                    {{ formatPoolOAuthPlanType(key.oauth_plan_type) }}
                  </Badge>
                  <Badge
                    :variant="key.is_active ? (key.cooldown_reason ? 'destructive' : 'default') : 'secondary'"
                    class="text-[10px] shrink-0"
                  >
                    {{ key.is_active ? (key.cooldown_reason ? '冷却' : '活跃') : '禁用' }}
                  </Badge>
                </div>
              </div>
              <div class="flex items-center gap-0.5 shrink-0">
                <Button
                  v-if="key.cooldown_reason"
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-muted-foreground hover:text-green-600"
                  title="清除冷却"
                  @click="clearCooldown(key.key_id)"
                >
                  <RefreshCw class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-muted-foreground hover:text-foreground"
                  :title="key.is_active ? '禁用' : '启用'"
                  @click="toggleKeyActive(key)"
                >
                  <component
                    :is="key.is_active ? Ban : Check"
                    class="w-3.5 h-3.5"
                  />
                </Button>
              </div>
            </div>
            <div
              class="mt-2.5 ml-7 grid gap-2"
              :class="showAccountQuotaColumn ? 'grid-cols-4' : 'grid-cols-3'"
            >
              <div class="p-2 bg-muted/50 rounded-lg text-xs">
                <div class="text-muted-foreground mb-0.5">
                  冷却
                </div>
                <div
                  v-if="key.cooldown_reason"
                  class="font-medium text-destructive text-[11px]"
                >
                  {{ formatCooldownReason(key.cooldown_reason) }}
                  <span
                    v-if="key.cooldown_ttl_seconds"
                    class="text-muted-foreground font-normal"
                  >
                    {{ formatTTL(key.cooldown_ttl_seconds) }}
                  </span>
                </div>
                <div
                  v-else
                  class="text-muted-foreground"
                >
                  -
                </div>
              </div>
              <div class="p-2 bg-muted/50 rounded-lg text-xs">
                <div class="text-muted-foreground mb-0.5">
                  成本
                </div>
                <div
                  v-if="key.cost_limit != null"
                  class="font-medium tabular-nums text-[11px]"
                >
                  {{ formatTokens(key.cost_window_usage) }}/{{ formatTokens(key.cost_limit) }}
                </div>
                <div
                  v-else-if="key.cost_window_usage > 0"
                  class="tabular-nums text-[11px]"
                >
                  {{ formatTokens(key.cost_window_usage) }}
                </div>
                <div
                  v-else
                  class="text-muted-foreground"
                >
                  -
                </div>
              </div>
              <div class="p-2 bg-muted/50 rounded-lg text-xs">
                <div class="text-muted-foreground mb-0.5">
                  最后使用
                </div>
                <div class="text-[11px]">
                  {{ key.last_used_at ? formatRelativeTime(key.last_used_at) : '-' }}
                </div>
              </div>
              <div
                v-if="showAccountQuotaColumn"
                class="p-2 bg-muted/50 rounded-lg text-xs"
              >
                <div class="text-muted-foreground mb-0.5">
                  配额
                </div>
                <div
                  v-if="quotaProgressMap[key.key_id]?.length"
                  class="flex items-stretch gap-2 overflow-x-auto pb-0.5"
                >
                  <div
                    v-for="(item, idx) in quotaProgressMap[key.key_id]"
                    :key="`${key.key_id}-quota-mobile-${idx}`"
                    class="min-w-[120px] max-w-[160px] shrink-0"
                  >
                    <div class="flex items-center justify-between text-[10px] mb-0.5">
                      <span class="text-muted-foreground">{{ item.label }}</span>
                      <span :class="getQuotaRemainingClassByRemaining(item.remainingPercent)">
                        {{ item.remainingPercent.toFixed(1) }}%
                      </span>
                    </div>
                    <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                      <div
                        class="absolute left-0 top-0 h-full transition-all duration-300"
                        :class="getQuotaRemainingBarColorByRemaining(item.remainingPercent)"
                        :style="{ width: `${item.remainingPercent}%` }"
                      />
                    </div>
                    <div
                      v-if="item.detail"
                      class="text-[9px] text-muted-foreground/70 mt-0.5 truncate"
                    >
                      {{ item.detail }}
                    </div>
                  </div>
                </div>
                <div
                  v-else-if="key.account_quota"
                  :class="getQuotaTextClass(key.account_quota)"
                >
                  {{ key.account_quota }}
                </div>
                <div
                  v-else
                  class="text-muted-foreground"
                >
                  -
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Empty keys -->
        <div
          v-if="keyPage.keys.length === 0 && !keysLoading"
          class="flex flex-col items-center justify-center py-16 text-center"
        >
          <div class="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-muted">
            <KeyRound class="h-8 w-8 text-muted-foreground" />
          </div>
          <p class="text-sm text-muted-foreground mt-4">
            暂无账号
          </p>
          <Button
            variant="outline"
            size="sm"
            class="mt-3"
            @click="showImportDialog = true"
          >
            <Upload class="w-3.5 h-3.5 mr-1.5" />
            批量导入
          </Button>
        </div>

        <!-- Pagination -->
        <Pagination
          v-if="keyPage.keys.length > 0"
          :current="currentPage"
          :total="keyPage.total"
          :page-size="pageSize"
          cache-key="pool-keys-page-size"
          @update:current="currentPage = $event"
          @update:page-size="pageSize = $event"
        />
      </template>
    </Card>

    <!-- Dialogs -->
    <PoolImportDialog
      v-if="selectedProviderId"
      v-model="showImportDialog"
      :provider-id="selectedProviderId"
      @imported="loadKeys"
    />
    <PoolConfigDialog
      v-if="selectedProviderId"
      v-model="showConfigDialog"
      :provider-id="selectedProviderId"
      :provider-type="selectedProviderData?.provider_type"
      :current-config="selectedProviderConfig"
      :current-claude-config="selectedProviderData?.claude_code_advanced"
      @saved="loadOverview"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue'
import { Search, Upload, Settings, RefreshCw, Ban, Check, Database, KeyRound } from 'lucide-vue-next'

import {
  Card,
  Badge,
  Button,
  Input,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Pagination,
} from '@/components/ui'
import RefreshButton from '@/components/ui/refresh-button.vue'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { formatOAuthPlanType, getOAuthPlanTypeClass } from '@/utils/oauthPlanType'
import {
  getPoolOverview,
  listPoolKeys,
  clearPoolCooldown,
  batchActionPoolKeys,
} from '@/api/endpoints/pool'
import type {
  PoolOverviewItem,
  PoolKeyDetail,
  PoolKeysPageResponse,
} from '@/api/endpoints/pool'
import type { PoolAdvancedConfig, ProviderWithEndpointsSummary } from '@/api/endpoints/types/provider'
import { getProvider } from '@/api/endpoints'
import PoolImportDialog from '@/features/pool/components/PoolImportDialog.vue'
import PoolConfigDialog from '@/features/pool/components/PoolConfigDialog.vue'

const { success, error: showError } = useToast()

// --- Overview ---
const poolProviders = ref<PoolOverviewItem[]>([])
const overviewLoading = ref(true)

async function loadOverview() {
  overviewLoading.value = true
  try {
    const res = await getPoolOverview()
    const enabledProviders = res.items.filter(item => item.pool_enabled)
    poolProviders.value = enabledProviders

    if (enabledProviders.length === 0) {
      selectedProviderId.value = null
      selectedProviderData.value = null
      selectedKeys.value.clear()
      keyPage.value = { total: 0, page: 1, page_size: pageSize.value, keys: [] }
      return
    }

    const hasValidSelectedProvider = !!selectedProviderId.value
      && enabledProviders.some(item => item.provider_id === selectedProviderId.value)

    // 默认选中第一个开启号池模式的 Provider；若当前选中项失效则自动纠正
    if (!hasValidSelectedProvider) {
      await selectProvider(enabledProviders[0].provider_id)
    }
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    overviewLoading.value = false
  }
}

// --- Provider Selection ---
const selectedProviderId = ref<string | null>(null)
const selectedProviderData = ref<ProviderWithEndpointsSummary | null>(null)

// Proxy for Select v-model (string, not string|null)
const selectedProviderIdProxy = computed({
  get: () => selectedProviderId.value ?? '',
  set: (val: string) => {
    if (val && val !== selectedProviderId.value) {
      selectProvider(val)
    }
  },
})

const selectedProviderConfig = computed<PoolAdvancedConfig | null>(() => {
  return (selectedProviderData.value as Record<string, unknown> | null)?.pool_advanced as PoolAdvancedConfig | null ?? null
})

const selectedProviderType = computed(() => {
  const fromDetail = String(selectedProviderData.value?.provider_type || '').trim().toLowerCase()
  if (fromDetail) return fromDetail
  const fromOverview = poolProviders.value.find(item => item.provider_id === selectedProviderId.value)?.provider_type
  return String(fromOverview || '').trim().toLowerCase()
})

const showAccountQuotaColumn = computed(() => {
  return selectedProviderType.value === 'codex'
    || selectedProviderType.value === 'kiro'
    || selectedProviderType.value === 'antigravity'
})

function shouldShowPlanTypeBadge(key: PoolKeyDetail): boolean {
  if (key.auth_type !== 'oauth' || !key.oauth_plan_type) {
    return false
  }
  return selectedProviderType.value === 'codex'
    || selectedProviderType.value === 'kiro'
    || selectedProviderType.value === 'antigravity'
}

function formatPoolOAuthPlanType(planType?: string | null): string {
  return formatOAuthPlanType(planType, selectedProviderType.value)
}

async function selectProvider(id: string) {
  selectedProviderId.value = id
  selectedKeys.value.clear()
  currentPage.value = 1
  searchQuery.value = ''
  statusFilter.value = 'all'
  await Promise.all([loadKeys(), loadProviderData(id)])
}

async function loadProviderData(id: string) {
  try {
    selectedProviderData.value = await getProvider(id)
  } catch {
    selectedProviderData.value = null
  }
}

async function refresh() {
  await loadOverview()
  if (selectedProviderId.value) {
    await loadKeys()
  }
}

// --- Keys ---
const keyPage = ref<PoolKeysPageResponse>({ total: 0, page: 1, page_size: 50, keys: [] })
const keysLoading = ref(false)
const searchQuery = ref('')
const statusFilter = ref('all')
const currentPage = ref(1)
const pageSize = ref(50)

interface QuotaProgressItem {
  label: string
  remainingPercent: number
  detail?: string
}

const quotaProgressMap = computed<Record<string, QuotaProgressItem[]>>(() => {
  const map: Record<string, QuotaProgressItem[]> = {}
  for (const key of keyPage.value.keys) {
    map[key.key_id] = parseQuotaProgressItems(key.account_quota)
  }
  return map
})

async function loadKeys() {
  if (!selectedProviderId.value) return
  keysLoading.value = true
  try {
    keyPage.value = await listPoolKeys(selectedProviderId.value, {
      page: currentPage.value,
      page_size: pageSize.value,
      search: searchQuery.value || undefined,
      status: statusFilter.value as 'all' | 'active' | 'cooldown' | 'inactive',
    })
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    keysLoading.value = false
  }
}

watch([currentPage, pageSize], () => loadKeys())
watch([searchQuery, statusFilter], () => {
  currentPage.value = 1
  loadKeys()
})

// --- Key Selection ---
const selectedKeys = ref(new Set<string>())

const allSelected = computed(() => {
  if (keyPage.value.keys.length === 0) return false
  return keyPage.value.keys.every(k => selectedKeys.value.has(k.key_id))
})

function toggleSelectAll() {
  if (allSelected.value) {
    selectedKeys.value.clear()
  } else {
    keyPage.value.keys.forEach(k => selectedKeys.value.add(k.key_id))
  }
}

function toggleSelect(id: string) {
  if (selectedKeys.value.has(id)) {
    selectedKeys.value.delete(id)
  } else {
    selectedKeys.value.add(id)
  }
}

// --- Actions ---
async function clearCooldown(keyId: string) {
  if (!selectedProviderId.value) return
  try {
    const res = await clearPoolCooldown(selectedProviderId.value, keyId)
    success(res.message)
    await loadKeys()
  } catch (err) {
    showError(parseApiError(err))
  }
}

async function toggleKeyActive(key: PoolKeyDetail) {
  if (!selectedProviderId.value) return
  try {
    const action = key.is_active ? 'disable' : 'enable'
    await batchActionPoolKeys(selectedProviderId.value, {
      key_ids: [key.key_id],
      action,
    })
    // 局部更新：直接修改本地列表中的状态
    const target = keyPage.value.keys.find(k => k.key_id === key.key_id)
    if (target) {
      target.is_active = !target.is_active
    }
  } catch (err) {
    showError(parseApiError(err))
  }
}

async function batchAction(action: 'enable' | 'disable' | 'delete' | 'clear_cooldown' | 'reset_cost') {
  if (!selectedProviderId.value || selectedKeys.value.size === 0) return
  try {
    const res = await batchActionPoolKeys(selectedProviderId.value, {
      key_ids: Array.from(selectedKeys.value),
      action,
    })
    success(res.message)
    selectedKeys.value.clear()
    await loadKeys()
  } catch (err) {
    showError(parseApiError(err))
  }
}

// --- Dialogs ---
const showImportDialog = ref(false)
const showConfigDialog = ref(false)

// --- Formatting ---
const COOLDOWN_REASON_MAP: Record<string, string> = {
  rate_limited_429: '429 限流',
  forbidden_403: '403 禁止',
  overloaded_529: '529 过载',
  auth_failed_401: '401 认证失败',
  payment_required_402: '402 欠费',
  server_error_500: '500 错误',
  quota_exhausted: '额度耗尽',
}

function formatCooldownReason(reason: string): string {
  return COOLDOWN_REASON_MAP[reason] || reason
}

function formatTTL(seconds: number): string {
  if (seconds <= 0) return ''
  const m = Math.floor(seconds / 60)
  const s = seconds % 60
  return m > 0 ? `${m}m ${s}s` : `${s}s`
}

function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`
  if (tokens >= 1_000) return `${(tokens / 1_000).toFixed(1)}K`
  return String(tokens)
}

function getCostBarColor(usage: number, limit: number): string {
  const ratio = usage / limit
  if (ratio >= 0.9) return 'bg-red-500'
  if (ratio >= 0.7) return 'bg-yellow-500'
  return 'bg-green-500'
}

function normalizeQuotaLabel(label: string): string {
  const normalized = label.trim()
  if (!normalized) return '额度'
  if (normalized.includes('周剩余')) return '周限额'
  if (normalized.includes('5H剩余')) return '5H限额'
  if (normalized.includes('最低剩余')) return '最低剩余'
  if (normalized === '剩余') return '剩余额度'
  return normalized
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0
  if (value < 0) return 0
  if (value > 100) return 100
  return value
}

function parseQuotaProgressItems(quotaText: string | null | undefined): QuotaProgressItem[] {
  if (!quotaText) return []

  const segments = quotaText
    .split('|')
    .map(s => s.trim())
    .filter(Boolean)

  const items: QuotaProgressItem[] = []
  for (const segment of segments) {
    const match = segment.match(/^(.*?)(-?\d+(?:\.\d+)?)%\s*(.*)$/)
    if (!match) continue

    const [, rawLabel, rawPercent, rawTail] = match
    const remainingPercent = clampPercent(Number(rawPercent))
    const label = normalizeQuotaLabel(rawLabel)
    const detail = rawTail.trim().replace(/^[()]+|[()]+$/g, '').trim()

    items.push({
      label,
      remainingPercent,
      detail: detail || undefined,
    })
  }

  return items
}

function getQuotaRemainingClassByRemaining(remaining: number): string {
  if (remaining <= 10) return 'text-red-600 dark:text-red-400'
  if (remaining <= 30) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-green-600 dark:text-green-400'
}

function getQuotaRemainingBarColorByRemaining(remaining: number): string {
  if (remaining <= 10) return 'bg-red-500 dark:bg-red-400'
  if (remaining <= 30) return 'bg-yellow-500 dark:bg-yellow-400'
  return 'bg-green-500 dark:bg-green-400'
}

function getQuotaTextClass(quotaText: string): string {
  if (quotaText.includes('封禁') || quotaText.includes('受限')) {
    return 'text-[11px] text-destructive leading-4'
  }
  return 'text-[11px] text-foreground/90 leading-4'
}

function formatRelativeTime(isoStr: string): string {
  const diff = (Date.now() - new Date(isoStr).getTime()) / 1000
  if (diff < 60) return '刚刚'
  if (diff < 3600) return `${Math.floor(diff / 60)}m 前`
  if (diff < 86400) return `${Math.floor(diff / 3600)}h 前`
  return `${Math.floor(diff / 86400)}d 前`
}

// --- Init ---
onMounted(async () => {
  await loadOverview()
})
</script>
