<template>
  <div class="space-y-6 pb-8">
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和筛选器 -->
      <div class="px-4 sm:px-6 py-3.5 border-b border-border/60">
        <!-- 移动端 -->
        <div class="flex flex-col gap-3 sm:hidden">
          <div class="flex items-center justify-between">
            <h3 class="text-base font-semibold">
              代理节点
            </h3>
            <div class="flex items-center gap-2">
              <Button
                size="sm"
                variant="outline"
                class="h-7 text-xs"
                @click="showBatchUpgradeDialog = true"
              >
                升级
              </Button>
              <Button
                size="sm"
                class="h-7 text-xs"
                @click="showAddDialog = true"
              >
                <Plus class="w-3 h-3 mr-1" />
                添加
              </Button>
              <RefreshButton
                :loading="store.loading"
                @click="refresh"
              />
            </div>
          </div>
          <div class="flex items-center gap-2">
            <div class="relative flex-1">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                v-model="searchQuery"
                type="text"
                placeholder="搜索..."
                class="w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
              />
            </div>
            <Select v-model="filterStatus">
              <SelectTrigger class="w-24 h-8 text-xs border-border/60">
                <SelectValue placeholder="状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部
                </SelectItem>
                <SelectItem value="online">
                  在线
                </SelectItem>
                <SelectItem value="offline">
                  离线
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <!-- 桌面端 -->
        <div class="hidden sm:flex items-center justify-between gap-4">
          <h3 class="text-base font-semibold">
            代理节点
          </h3>
          <div class="flex items-center gap-2">
            <div class="relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                v-model="searchQuery"
                type="text"
                placeholder="搜索..."
                class="w-48 pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
              />
            </div>
            <div class="h-4 w-px bg-border" />
            <Select v-model="filterStatus">
              <SelectTrigger class="w-28 h-8 text-xs border-border/60">
                <SelectValue placeholder="全部状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部状态
                </SelectItem>
                <SelectItem value="online">
                  在线
                </SelectItem>
                <SelectItem value="offline">
                  离线
                </SelectItem>
              </SelectContent>
            </Select>
            <div class="h-4 w-px bg-border" />
            <Button
              variant="outline"
              size="sm"
              class="h-8 text-xs"
              @click="showBatchUpgradeDialog = true"
            >
              批量升级
            </Button>
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="手动添加"
              @click="showAddDialog = true"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>
            <RefreshButton
              :loading="store.loading"
              @click="refresh"
            />
          </div>
        </div>
      </div>

      <!-- 桌面端表格 -->
      <div class="hidden xl:block overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow class="border-b border-border/60 hover:bg-transparent">
              <TableHead class="w-[160px] h-12 font-semibold">
                名称
              </TableHead>
              <TableHead class="w-[180px] h-12 font-semibold">
                地址
              </TableHead>
              <TableHead class="w-[100px] h-12 font-semibold">
                区域
              </TableHead>
              <TableHead class="w-[90px] h-12 font-semibold text-center">
                状态
              </TableHead>
              <TableHead class="w-[100px] h-12 font-semibold text-center">
                总请求
              </TableHead>
              <TableHead class="w-[100px] h-12 font-semibold text-center">
                失败率
              </TableHead>
              <TableHead class="w-[100px] h-12 font-semibold text-center">
                延迟
              </TableHead>
              <TableHead class="w-[120px] h-12 font-semibold text-center">
                版本
              </TableHead>
              <TableHead class="w-[160px] h-12 font-semibold">
                最后心跳
              </TableHead>
              <TableHead class="w-[80px] h-12 font-semibold text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="node in paginatedNodes"
              :key="node.id"
              class="border-b border-border/40 hover:bg-muted/30 transition-colors"
            >
              <TableCell class="py-4">
                <div class="flex items-center gap-1.5">
                  <span class="text-sm font-semibold">{{ node.name }}</span>
                  <Badge
                    v-if="node.is_manual"
                    variant="outline"
                    class="text-[10px] px-1.5 py-0"
                  >
                    手动
                  </Badge>
                  <Badge
                    v-if="node.tunnel_mode"
                    variant="outline"
                    class="text-[10px] px-1.5 py-0"
                  >
                    Tunnel
                  </Badge>
                  <HardwareTooltip :node="node" />
                </div>
              </TableCell>
              <TableCell class="py-4">
                <code class="text-xs text-muted-foreground">{{ nodeAddress(node) }}</code>
              </TableCell>
              <TableCell class="py-4">
                <span class="text-sm text-muted-foreground">{{ formatRegion(node.region) }}</span>
              </TableCell>
              <TableCell class="py-4 text-center">
                <Badge
                  :variant="statusVariant(node.status)"
                  class="font-medium px-2.5 py-0.5 text-xs"
                >
                  {{ statusLabel(node.status) }}
                </Badge>
              </TableCell>
              <TableCell class="py-4 text-center">
                <span class="text-sm tabular-nums">{{ formatNumber(node.total_requests) }}</span>
              </TableCell>
              <TableCell class="py-4 text-center">
                <span
                  class="text-sm tabular-nums"
                  :class="failureRate(node) > 5 ? 'text-destructive font-medium' : ''"
                >{{ formatFailureRate(node) }}</span>
              </TableCell>
              <TableCell class="py-4 text-center">
                <span class="text-sm tabular-nums">{{ node.avg_latency_ms != null ? `${node.avg_latency_ms.toFixed(0)}ms` : '-' }}</span>
              </TableCell>
              <TableCell class="py-4 text-center">
                <span class="text-sm tabular-nums">{{ node.is_manual ? '-' : nodeProxyVersion(node) }}</span>
              </TableCell>
              <TableCell class="py-4">
                <span class="text-xs text-muted-foreground">{{ formatTime(node.last_heartbeat_at) }}</span>
              </TableCell>
              <TableCell class="py-4 text-center">
                <div class="flex items-center justify-center gap-0.5">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="testingNodes.has(node.id) ? '测试中...' : '测试连通性'"
                    :disabled="testingNodes.has(node.id)"
                    @click="handleTest(node)"
                  >
                    <Loader2
                      v-if="testingNodes.has(node.id)"
                      class="h-4 w-4 animate-spin"
                    />
                    <Activity
                      v-else
                      class="h-4 w-4"
                    />
                  </Button>
                  <Button
                    v-if="node.is_manual"
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="编辑"
                    @click="handleEdit(node)"
                  >
                    <SquarePen class="h-4 w-4" />
                  </Button>
                  <Button
                    v-if="!node.is_manual"
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="远程配置"
                    @click="handleConfig(node)"
                  >
                    <Settings class="h-4 w-4" />
                  </Button>
                  <Button
                    v-if="!node.is_manual"
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="连接事件"
                    @click="handleViewEvents(node)"
                  >
                    <History class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="删除"
                    @click="handleDelete(node)"
                  >
                    <Trash2 class="h-4 w-4" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
            <TableRow v-if="paginatedNodes.length === 0">
              <TableCell
                colspan="10"
                class="py-12 text-center text-muted-foreground text-sm"
              >
                {{ store.loading ? '加载中...' : '暂无代理节点' }}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>

      <!-- 移动端卡片列表 -->
      <div class="xl:hidden divide-y divide-border/40">
        <div
          v-for="node in paginatedNodes"
          :key="node.id"
          class="p-4 sm:p-5"
        >
          <div class="flex items-start justify-between mb-2">
            <div>
              <div class="flex items-center gap-1.5">
                <span class="font-semibold text-sm">{{ node.name }}</span>
                <Badge
                  v-if="node.is_manual"
                  variant="outline"
                  class="text-[10px] px-1.5 py-0"
                >
                  手动
                </Badge>
                <Badge
                  v-if="node.tunnel_mode"
                  variant="outline"
                  class="text-[10px] px-1.5 py-0"
                >
                  Tunnel
                </Badge>
                <HardwareTooltip :node="node" />
              </div>
              <code class="text-xs text-muted-foreground">{{ nodeAddress(node) }}</code>
              <div
                v-if="!node.is_manual"
                class="text-[11px] text-muted-foreground mt-1"
              >
                版本: {{ nodeProxyVersion(node) }}
              </div>
            </div>
            <Badge
              :variant="statusVariant(node.status)"
              class="text-xs"
            >
              {{ statusLabel(node.status) }}
            </Badge>
          </div>
          <div class="grid grid-cols-4 gap-2 text-xs text-muted-foreground mb-3">
            <div>
              <span class="block text-foreground/60">区域</span>
              <span>{{ formatRegion(node.region) }}</span>
            </div>
            <div>
              <span class="block text-foreground/60">总请求</span>
              <span class="tabular-nums">{{ formatNumber(node.total_requests) }}</span>
            </div>
            <div>
              <span class="block text-foreground/60">失败率</span>
              <span
                class="tabular-nums"
                :class="failureRate(node) > 5 ? 'text-destructive font-medium' : ''"
              >{{ formatFailureRate(node) }}</span>
            </div>
            <div>
              <span class="block text-foreground/60">延迟</span>
              <span class="tabular-nums">{{ node.avg_latency_ms != null ? `${node.avg_latency_ms.toFixed(0)}ms` : '-' }}</span>
            </div>
          </div>
          <div class="flex items-center justify-between">
            <span class="text-xs text-muted-foreground">{{ formatTime(node.last_heartbeat_at) }}</span>
            <div class="flex items-center gap-1">
              <Button
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs"
                :disabled="testingNodes.has(node.id)"
                @click="handleTest(node)"
              >
                <Loader2
                  v-if="testingNodes.has(node.id)"
                  class="h-3 w-3 mr-1 animate-spin"
                />
                <Activity
                  v-else
                  class="h-3 w-3 mr-1"
                />
                {{ testingNodes.has(node.id) ? '测试中' : '测试' }}
              </Button>
              <Button
                v-if="node.is_manual"
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs"
                @click="handleEdit(node)"
              >
                <SquarePen class="h-3 w-3 mr-1" />
                编辑
              </Button>
              <Button
                v-if="!node.is_manual"
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs"
                @click="handleConfig(node)"
              >
                <Settings class="h-3 w-3 mr-1" />
                配置
              </Button>
              <Button
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs"
                @click="handleDelete(node)"
              >
                <Trash2 class="h-3 w-3 mr-1" />
                删除
              </Button>
            </div>
          </div>
        </div>
        <div
          v-if="paginatedNodes.length === 0"
          class="p-8 text-center text-muted-foreground text-sm"
        >
          {{ store.loading ? '加载中...' : '暂无代理节点' }}
        </div>
      </div>

      <!-- 分页 -->
      <Pagination
        :current="currentPage"
        :total="filteredNodes.length"
        :page-size="pageSize"
        cache-key="proxy-nodes-page-size"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>
    <!-- 手动添加/编辑代理节点对话框 -->
    <Dialog
      :model-value="showAddDialog"
      :title="editingNode ? '编辑代理节点' : '手动添加代理节点'"
      :description="editingNode ? '修改手动代理节点的配置' : '手动配置的代理节点，用于无法部署 aether-proxy 的场景'"
      :icon="editingNode ? SquarePen : Plus"
      size="md"
      @update:model-value="handleDialogClose"
    >
      <form
        class="space-y-4"
        @submit.prevent="handleAddManualNode"
      >
        <div class="space-y-1.5">
          <Label>名称 *</Label>
          <Input
            v-model="addForm.name"
            placeholder="例如: 美西 VPN 代理"
          />
        </div>
        <div class="space-y-1.5">
          <Label>代理地址 *</Label>
          <Input
            v-model="addForm.proxy_url"
            placeholder="http://proxy:port 或 socks5://proxy:port"
          />
        </div>
        <div class="grid grid-cols-2 gap-3">
          <div class="space-y-1.5">
            <Label>用户名</Label>
            <Input
              v-model="addForm.username"
              placeholder="可选"
              autocomplete="off"
              data-form-type="other"
              data-lpignore="true"
              data-1p-ignore="true"
            />
          </div>
          <div class="space-y-1.5">
            <Label>密码</Label>
            <Input
              v-model="addForm.password"
              type="password"
              placeholder="可选"
              autocomplete="new-password"
              data-form-type="other"
              data-lpignore="true"
              data-1p-ignore="true"
            />
          </div>
        </div>
        <div class="space-y-1.5">
          <Label>区域</Label>
          <Input
            v-model="addForm.region"
            placeholder="可选，例如: US-West"
          />
        </div>
      </form>

      <template #footer>
        <div class="flex items-center justify-between w-full">
          <Button
            variant="outline"
            :disabled="testingUrl || !addForm.proxy_url"
            @click="handleTestUrl"
          >
            {{ testingUrl ? '测试中...' : '测试' }}
          </Button>
          <div class="flex items-center gap-2">
            <Button
              variant="outline"
              @click="handleDialogClose(false)"
            >
              取消
            </Button>
            <Button
              :disabled="addingNode || !addForm.name || !addForm.proxy_url"
              @click="editingNode ? handleUpdateManualNode() : handleAddManualNode()"
            >
              {{ addingNode ? (editingNode ? '保存中...' : '添加中...') : (editingNode ? '保存' : '添加') }}
            </Button>
          </div>
        </div>
      </template>
    </Dialog>

    <!-- 远程配置对话框 (aether-proxy 节点) -->
    <Dialog
      :model-value="showConfigDialog"
      title="远程配置"
      description="修改后将在下次心跳时自动下发到 aether-proxy 节点"
      :icon="Settings"
      size="md"
      @update:model-value="handleConfigDialogClose"
    >
      <form
        class="space-y-4"
        @submit.prevent
      >
        <div class="space-y-1.5">
          <Label>允许的端口</Label>
          <Input
            v-model="configForm.allowed_ports"
            placeholder="80, 443, 8080, 8443"
          />
          <p class="text-xs text-muted-foreground">
            逗号分隔的目标端口白名单
          </p>
        </div>
        <div class="space-y-1.5">
          <Label>日志级别</Label>
          <Select v-model="configForm.log_level">
            <SelectTrigger><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="trace">
                trace
              </SelectItem>
              <SelectItem value="debug">
                debug
              </SelectItem>
              <SelectItem value="info">
                info
              </SelectItem>
              <SelectItem value="warn">
                warn
              </SelectItem>
              <SelectItem value="error">
                error
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-1.5">
            <Label>心跳间隔 (秒)</Label>
            <Input
              v-model="configForm.heartbeat_interval"
              type="number"
              min="5"
              max="600"
            />
          </div>
        </div>
        <div class="space-y-1.5">
          <Label>升级到版本</Label>
          <Input
            v-model="configForm.upgrade_to"
            placeholder="例如 0.2.3"
          />
          <p class="text-xs text-muted-foreground">
            留空可清除已有升级指令
          </p>
        </div>
        <div
          v-if="configNode"
          class="text-xs text-muted-foreground"
        >
          配置版本: v{{ configNode.config_version }}
        </div>
      </form>
      <template #footer>
        <Button
          variant="outline"
          @click="handleConfigDialogClose(false)"
        >
          取消
        </Button>
        <Button
          :disabled="savingConfig"
          @click="handleSaveConfig"
        >
          {{ savingConfig ? '保存中...' : '保存' }}
        </Button>
      </template>
    </Dialog>

    <!-- 批量升级对话框 -->
    <Dialog
      :model-value="showBatchUpgradeDialog"
      title="批量升级"
      description="向所有在线 tunnel 节点下发升级版本指令"
      :icon="Settings"
      size="sm"
      @update:model-value="(open: boolean) => { if (!open) { showBatchUpgradeDialog = false; batchUpgradeVersion = '' } }"
    >
      <form
        class="space-y-4"
        @submit.prevent="handleBatchUpgrade"
      >
        <div class="space-y-1.5">
          <Label>目标版本</Label>
          <Input
            v-model="batchUpgradeVersion"
            placeholder="例如 0.2.3"
          />
        </div>
      </form>
      <template #footer>
        <Button
          variant="outline"
          @click="showBatchUpgradeDialog = false; batchUpgradeVersion = ''"
        >
          取消
        </Button>
        <Button
          :disabled="batchUpgrading || !batchUpgradeVersion.trim()"
          @click="handleBatchUpgrade"
        >
          {{ batchUpgrading ? '下发中...' : '确认下发' }}
        </Button>
      </template>
    </Dialog>

    <!-- 连接事件对话框 -->
    <Dialog
      :open="showEventsDialog"
      title="连接事件"
      :description="eventsNode ? `${eventsNode.name} 的连接历史` : ''"
      size="lg"
      @update:open="(v: boolean) => { if (!v) { showEventsDialog = false; eventsNode = null; nodeEvents = [] } }"
    >
      <div class="space-y-3">
        <!-- 可靠性指标摘要 -->
        <div
          v-if="eventsNode"
          class="grid grid-cols-3 gap-3 text-sm"
        >
          <div class="bg-muted/40 rounded-lg px-3 py-2 text-center">
            <span class="block text-foreground/60 text-xs">失败请求</span>
            <span class="tabular-nums font-medium">{{ formatNumber(eventsNode.failed_requests || 0) }}</span>
          </div>
          <div class="bg-muted/40 rounded-lg px-3 py-2 text-center">
            <span class="block text-foreground/60 text-xs">DNS 失败</span>
            <span class="tabular-nums font-medium">{{ formatNumber(eventsNode.dns_failures || 0) }}</span>
          </div>
          <div class="bg-muted/40 rounded-lg px-3 py-2 text-center">
            <span class="block text-foreground/60 text-xs">流错误</span>
            <span class="tabular-nums font-medium">{{ formatNumber(eventsNode.stream_errors || 0) }}</span>
          </div>
        </div>

        <!-- 事件列表 -->
        <div
          v-if="loadingEvents"
          class="py-8 text-center text-muted-foreground text-sm"
        >
          加载中...
        </div>
        <div
          v-else-if="nodeEvents.length === 0"
          class="py-8 text-center text-muted-foreground text-sm"
        >
          暂无连接事件记录
        </div>
        <div
          v-else
          class="max-h-80 overflow-y-auto space-y-1.5"
        >
          <div
            v-for="event in nodeEvents"
            :key="event.id"
            class="flex items-center gap-2 px-3 py-2 rounded-lg bg-muted/30 text-sm"
          >
            <Badge
              :variant="eventTypeVariant(event.event_type)"
              class="text-[10px] px-1.5 py-0 shrink-0"
            >
              {{ eventTypeLabel(event.event_type) }}
            </Badge>
            <span class="text-muted-foreground truncate flex-1">{{ event.detail || '-' }}</span>
            <span class="text-xs text-muted-foreground/70 tabular-nums shrink-0">{{ formatTime(event.created_at) }}</span>
          </div>
        </div>
      </div>
      <template #footer>
        <Button
          variant="outline"
          @click="showEventsDialog = false; eventsNode = null; nodeEvents = []"
        >
          关闭
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { proxyNodesApi, type ProxyNode, type ProxyNodeRemoteConfig, type ProxyNodeEvent } from '@/api/proxy-nodes'

import {
  Card,
  Button,
  Badge,
  Input,
  Label,
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
  RefreshButton,
  Dialog,
} from '@/components/ui'

import { Search, Trash2, Plus, SquarePen, Activity, Loader2, Settings, History } from 'lucide-vue-next'
import { parseApiError } from '@/utils/errorParser'
import { formatRegion } from '@/utils/region'
import HardwareTooltip from './components/HardwareTooltip.vue'

const { success, error: toastError } = useToast()
const { confirmDanger } = useConfirm()
const store = useProxyNodesStore()

const searchQuery = ref('')
const filterStatus = ref('all')
const currentPage = ref(1)
const pageSize = ref(20)

// 手动添加/编辑对话框
const showAddDialog = ref(false)
const addingNode = ref(false)
const editingNode = ref<ProxyNode | null>(null)
const addForm = ref({
  name: '',
  proxy_url: '',
  username: '',
  password: '',
  region: '',
})

// 远程配置对话框 (aether-proxy 节点)
const showConfigDialog = ref(false)
const savingConfig = ref(false)
const configNode = ref<ProxyNode | null>(null)
const configForm = ref({
  allowed_ports: '',
  log_level: 'info',
  heartbeat_interval: '30',
  upgrade_to: '',
})
const showBatchUpgradeDialog = ref(false)
const batchUpgradeVersion = ref('')
const batchUpgrading = ref(false)

// 连接事件对话框
const showEventsDialog = ref(false)
const eventsNode = ref<ProxyNode | null>(null)
const nodeEvents = ref<ProxyNodeEvent[]>([])
const loadingEvents = ref(false)

// 测试连通性
const testingNodes = ref(new Set<string>())
const testingUrl = ref(false)

const filteredNodes = computed(() => {
  let filtered = [...store.nodes]

  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    filtered = filtered.filter(node => {
      const text = `${node.name} ${node.ip} ${node.region || ''}`.toLowerCase()
      return keywords.every(kw => text.includes(kw))
    })
  }

  if (filterStatus.value !== 'all') {
    filtered = filtered.filter(node => node.status === filterStatus.value)
  }

  return filtered
})

const paginatedNodes = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredNodes.value.slice(start, start + pageSize.value)
})

watch([searchQuery, filterStatus], () => {
  currentPage.value = 1
})

onMounted(async () => {
  await store.fetchNodes()
})

async function refresh() {
  await store.fetchNodes()
}

async function handleTestUrl() {
  if (!addForm.value.proxy_url || testingUrl.value) return
  testingUrl.value = true
  try {
    const result = await proxyNodesApi.testProxyUrl({
      proxy_url: addForm.value.proxy_url,
      username: addForm.value.username || undefined,
      password: addForm.value.password || undefined,
    })
    if (result.success) {
      const parts = [`延迟: ${result.latency_ms}ms`]
      if (result.exit_ip) parts.push(`出口IP: ${result.exit_ip}`)
      success(`连通性测试通过，${parts.join('，')}`)
    } else {
      toastError(`连通性测试失败: ${result.error || '未知错误'}`)
    }
  } catch (err: unknown) {
    toastError(parseApiError(err, '测试请求失败'))
  } finally {
    testingUrl.value = false
  }
}

function handleEdit(node: ProxyNode) {
  editingNode.value = node
  addForm.value = {
    name: node.name,
    proxy_url: node.proxy_url || '',
    username: node.proxy_username || '',
    password: '', // 不回填密码（已脱敏）
    region: node.region || '',
  }
  showAddDialog.value = true
}

function handleDialogClose(open: boolean) {
  if (!open) {
    showAddDialog.value = false
    editingNode.value = null
    addForm.value = { name: '', proxy_url: '', username: '', password: '', region: '' }
  }
}

async function handleUpdateManualNode() {
  if (!editingNode.value || !addForm.value.name || !addForm.value.proxy_url) return

  addingNode.value = true
  try {
    await proxyNodesApi.updateManualNode(editingNode.value.id, {
      name: addForm.value.name,
      proxy_url: addForm.value.proxy_url,
      username: addForm.value.username || undefined,
      // 空密码不发送（保留原值）
      password: addForm.value.password || undefined,
      region: addForm.value.region || undefined,
    })
    success('代理节点已更新')
    handleDialogClose(false)
    await store.fetchNodes()
  } catch (err: unknown) {
    toastError(parseApiError(err, '更新失败'))
  } finally {
    addingNode.value = false
  }
}

async function handleAddManualNode() {
  if (!addForm.value.name || !addForm.value.proxy_url) return

  addingNode.value = true
  try {
    await store.createManualNode({
      name: addForm.value.name,
      proxy_url: addForm.value.proxy_url,
      username: addForm.value.username || undefined,
      password: addForm.value.password || undefined,
      region: addForm.value.region || undefined,
    })
    success('代理节点已添加')
    handleDialogClose(false)
  } catch (err: unknown) {
    toastError(parseApiError(err, '添加失败'))
  } finally {
    addingNode.value = false
  }
}

function handleConfig(node: ProxyNode) {
  configNode.value = node
  const rc: ProxyNodeRemoteConfig = node.remote_config ?? {}
  configForm.value = {
    allowed_ports: rc.allowed_ports?.join(', ') || '',
    log_level: rc.log_level || 'info',
    heartbeat_interval: String(rc.heartbeat_interval || node.heartbeat_interval || 30),
    upgrade_to: rc.upgrade_to || '',
  }
  showConfigDialog.value = true
}

function handleConfigDialogClose(open: boolean) {
  if (!open) {
    showConfigDialog.value = false
    configNode.value = null
  }
}

async function handleSaveConfig() {
  if (!configNode.value) return
  savingConfig.value = true
  try {
    const data: Partial<ProxyNodeRemoteConfig> = {}
    const portsInput = configForm.value.allowed_ports.trim()
    if (portsInput) {
      data.allowed_ports = portsInput
        .split(',')
        .map((s: string) => parseInt(s.trim()))
        .filter((n: number) => !isNaN(n) && n >= 1 && n <= 65535)
    } else if (configNode.value.remote_config?.allowed_ports) {
      // 输入清空 → 显式发送空数组以清除已有端口白名单
      data.allowed_ports = []
    }
    if (configForm.value.log_level) {
      data.log_level = configForm.value.log_level
    }
    const hb = parseInt(configForm.value.heartbeat_interval)
    if (!isNaN(hb) && hb >= 5) {
      data.heartbeat_interval = hb
    }
    const targetVersion = configForm.value.upgrade_to.trim()
    if (targetVersion) {
      data.upgrade_to = targetVersion
    } else if (configNode.value.remote_config?.upgrade_to) {
      data.upgrade_to = null
    }
    await proxyNodesApi.updateNodeConfig(configNode.value.id, data)
    success('远程配置已保存，将在下次心跳时生效')
    handleConfigDialogClose(false)
    await store.fetchNodes()
  } catch (err: unknown) {
    toastError(parseApiError(err, '保存失败'))
  } finally {
    savingConfig.value = false
  }
}

async function handleBatchUpgrade() {
  const version = batchUpgradeVersion.value.trim()
  if (!version || batchUpgrading.value) return
  batchUpgrading.value = true
  try {
    const result = await proxyNodesApi.batchUpgrade(version)
    success(`升级指令已下发：${result.updated} 个节点，跳过 ${result.skipped} 个`)
    showBatchUpgradeDialog.value = false
    batchUpgradeVersion.value = ''
    await store.fetchNodes()
  } catch (err: unknown) {
    toastError(parseApiError(err, '批量升级下发失败'))
  } finally {
    batchUpgrading.value = false
  }
}

async function handleDelete(node: ProxyNode) {
  const confirmed = await confirmDanger(
    `确定要删除代理节点 "${node.name}" (${node.tunnel_mode ? node.ip : `${node.ip}:${node.port}`}) 吗？`,
    '删除节点'
  )
  if (!confirmed) return

  try {
    const result = await proxyNodesApi.deleteProxyNode(node.id)
    // 同步更新 store 本地状态
    store.nodes = store.nodes.filter(n => n.id !== node.id)
    if (result.cleared_system_proxy) {
      success('代理节点已删除，系统默认代理已自动清除')
    } else {
      success('代理节点已删除')
    }
  } catch (err: unknown) {
    toastError(parseApiError(err, '删除失败'))
  }
}

async function handleTest(node: ProxyNode) {
  if (testingNodes.value.has(node.id)) return

  testingNodes.value.add(node.id)
  try {
    const result = await proxyNodesApi.testNode(node.id)
    if (result.success) {
      const parts = [`延迟: ${result.latency_ms}ms`]
      if (result.exit_ip) parts.push(`出口IP: ${result.exit_ip}`)
      success(`连通性测试通过，${parts.join('，')}`)
    } else {
      toastError(`连通性测试失败: ${result.error || '未知错误'}`)
    }
  } catch (err: unknown) {
    toastError(parseApiError(err, '测试请求失败'))
  } finally {
    testingNodes.value.delete(node.id)
  }
}

async function handleViewEvents(node: ProxyNode) {
  eventsNode.value = node
  showEventsDialog.value = true
  loadingEvents.value = true
  try {
    const res = await proxyNodesApi.listNodeEvents(node.id, 50)
    nodeEvents.value = res.items
  } catch (err: unknown) {
    toastError(parseApiError(err, '加载事件失败'))
  } finally {
    loadingEvents.value = false
  }
}

function eventTypeLabel(type: string) {
  switch (type) {
    case 'connected': return '连接'
    case 'disconnected': return '断开'
    case 'error': return '错误'
    default: return type
  }
}

function eventTypeVariant(type: string) {
  switch (type) {
    case 'connected': return 'success' as const
    case 'disconnected': return 'destructive' as const
    case 'error': return 'destructive' as const
    default: return 'secondary' as const
  }
}

function statusVariant(status: string) {
  switch (status) {
    case 'online': return 'success' as const
    case 'offline': return 'destructive' as const
    default: return 'secondary' as const
  }
}

function statusLabel(status: string) {
  switch (status) {
    case 'online': return '在线'
    case 'offline': return '离线'
    default: return status
  }
}

function formatNumber(n: number) {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}K`
  return String(n)
}

function formatTime(iso: string | null) {
  if (!iso) return '-'
  const d = new Date(iso)
  const now = new Date()
  const diff = (now.getTime() - d.getTime()) / 1000
  if (diff < 60) return '刚刚'
  if (diff < 3600) return `${Math.floor(diff / 60)}分钟前`
  if (diff < 86400) return `${Math.floor(diff / 3600)}小时前`
  return d.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
}

function failureRate(node: ProxyNode) {
  if (!node.total_requests) return 0
  const failed = (node.failed_requests || 0) + (node.dns_failures || 0) + (node.stream_errors || 0)
  return (failed / node.total_requests) * 100
}

function formatFailureRate(node: ProxyNode) {
  if (!node.total_requests) return '-'
  const rate = failureRate(node)
  if (rate === 0) return '0%'
  if (rate < 0.1) return '<0.1%'
  return `${rate.toFixed(1)}%`
}

function nodeAddress(node: ProxyNode) {
  if (node.is_manual) return node.proxy_url || `${node.ip}:${node.port}`
  if (node.tunnel_mode) return node.ip || 'WebSocket Tunnel'
  return `${node.ip}:${node.port}`
}

function nodeProxyVersion(node: ProxyNode) {
  const metadata = node.proxy_metadata
  if (!metadata || typeof metadata !== 'object') return '-'
  const version = (metadata as Record<string, unknown>).version
  if (typeof version !== 'string') return '-'
  const normalized = version.trim()
  return normalized || '-'
}
</script>
