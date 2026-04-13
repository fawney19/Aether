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
	          <div class="flex flex-wrap items-center gap-2">
	            <div class="relative min-w-0 basis-full">
	              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
	              <Input
	                v-model="searchQuery"
	                type="text"
	                placeholder="搜索..."
	                class="w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
	              />
	            </div>
	            <div class="min-w-0 flex-1">
	              <Select v-model="filterStatus">
	                <SelectTrigger class="w-full h-8 text-xs border-border/60">
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
	            <div class="min-w-0 flex-1">
	              <Select
	                v-model="filterRolloutStage"
	                :disabled="!store.rollout"
	              >
	                <SelectTrigger class="w-full h-8 text-xs border-border/60">
	                  <SelectValue placeholder="阶段" />
	                </SelectTrigger>
	                <SelectContent>
	                  <SelectItem
	                    v-for="option in rolloutStageOptions"
	                    :key="option.value"
	                    :value="option.value"
	                  >
	                    {{ option.label }}
	                  </SelectItem>
	                </SelectContent>
	              </Select>
	            </div>
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
	            <Select
	              v-model="filterRolloutStage"
	              :disabled="!store.rollout"
	            >
	              <SelectTrigger class="w-36 h-8 text-xs border-border/60">
	                <SelectValue placeholder="全部阶段" />
	              </SelectTrigger>
	              <SelectContent>
	                <SelectItem
	                  v-for="option in rolloutStageOptions"
	                  :key="option.value"
	                  :value="option.value"
	                >
	                  {{ option.label }}
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

      <div
        v-if="store.rollout"
        class="px-4 sm:px-6 py-4 border-b border-border/60 bg-muted/15"
      >
        <div class="flex flex-col gap-4 xl:flex-row xl:items-start xl:justify-between">
          <div class="space-y-3">
	            <div class="flex flex-wrap items-center gap-2">
	              <Badge
	                :variant="store.rollout.blocked ? 'warning' : 'success'"
	                class="text-[11px] px-2 py-0.5"
              >
                {{ store.rollout.blocked ? 'Rollout 等待确认' : 'Rollout 推进中' }}
              </Badge>
	              <span class="text-sm font-semibold">目标版本 {{ store.rollout.version }}</span>
	              <span class="text-xs text-muted-foreground">
	                每波 {{ store.rollout.batch_size }} 个节点 · 冷却 {{ formatDurationShort(store.rollout.cooldown_secs) }}
	              </span>
	              <Badge
	                v-if="filterRolloutStage !== 'all'"
	                variant="outline"
	                class="text-[11px] px-2 py-0.5"
	              >
	                筛选: {{ activeRolloutStageLabel }}
	              </Badge>
	              <Button
	                v-if="filterRolloutStage !== 'all'"
	                variant="ghost"
	                size="sm"
	                class="h-7 px-2 text-[11px]"
	                @click="filterRolloutStage = 'all'"
	              >
	                清除阶段筛选
	              </Button>
	            </div>
            <div class="grid grid-cols-2 gap-3 sm:grid-cols-5">
              <div class="rounded-lg border border-border/50 bg-background/80 px-3 py-2">
                <div class="text-[11px] text-muted-foreground">
                  已完成
                </div>
                <div class="mt-1 text-sm font-semibold tabular-nums">
                  {{ store.rollout.completed_node_ids.length }}
                </div>
              </div>
              <div class="rounded-lg border border-border/50 bg-background/80 px-3 py-2">
                <div class="text-[11px] text-muted-foreground">
                  等待确认
                </div>
                <div class="mt-1 text-sm font-semibold tabular-nums">
                  {{ store.rollout.pending_node_ids.length }}
                </div>
              </div>
              <div class="rounded-lg border border-border/50 bg-background/80 px-3 py-2">
                <div class="text-[11px] text-muted-foreground">
                  冲突节点
                </div>
                <div class="mt-1 text-sm font-semibold tabular-nums">
                  {{ store.rollout.conflict_node_ids.length }}
                </div>
              </div>
              <div class="rounded-lg border border-border/50 bg-background/80 px-3 py-2">
                <div class="text-[11px] text-muted-foreground">
                  已跳过
                </div>
                <div class="mt-1 text-sm font-semibold tabular-nums">
                  {{ store.rollout.skipped_node_ids.length }}
                </div>
              </div>
              <div class="rounded-lg border border-border/50 bg-background/80 px-3 py-2">
                <div class="text-[11px] text-muted-foreground">
                  在线可升级
                </div>
                <div class="mt-1 text-sm font-semibold tabular-nums">
                  {{ store.rollout.online_eligible_total }}
                </div>
              </div>
            </div>
            <div
              v-if="rolloutPendingPreview.length > 0"
              class="space-y-2"
            >
              <div class="text-xs font-medium text-foreground/80">
                当前卡点
              </div>
              <div class="flex flex-wrap gap-2">
                <div
                  v-for="entry in rolloutPendingPreview"
                  :key="entry.node_id"
                  class="flex items-center gap-2 rounded-full border border-border/60 bg-background/90 px-3 py-1 text-xs"
                >
                  <span class="font-medium">{{ entry.name }}</span>
                  <Badge
                    :variant="entry.variant"
                    class="px-1.5 py-0 text-[10px]"
                  >
                    {{ entry.label }}
                  </Badge>
                  <Button
                    v-if="canRetryUpgradeNode(entry.node)"
                    variant="ghost"
                    size="sm"
                    class="h-6 px-2 text-[11px]"
                    :disabled="isUpgradeNodeActioning(entry.node_id)"
                    @click="handleRetryUpgradeNode(entry.node)"
                  >
                    <Loader2
                      v-if="isUpgradeNodeActioning(entry.node_id)"
                      class="h-3 w-3 mr-1 animate-spin"
                    />
                    <RotateCcw
                      v-else
                      class="h-3 w-3 mr-1"
                    />
                    重试
                  </Button>
                  <Button
                    v-if="canSkipUpgradeNode(entry.node)"
                    variant="ghost"
                    size="sm"
                    class="h-6 px-2 text-[11px]"
                    :disabled="isUpgradeNodeActioning(entry.node_id)"
                    @click="handleSkipUpgradeNode(entry.node)"
                  >
                    <Loader2
                      v-if="isUpgradeNodeActioning(entry.node_id)"
                      class="h-3 w-3 mr-1 animate-spin"
                    />
                    <SkipForward
                      v-else
                      class="h-3 w-3 mr-1"
                    />
                    跳过
                  </Button>
                </div>
              </div>
              <p
                v-if="hiddenPendingNodeCount > 0"
                class="text-xs text-muted-foreground"
              >
                还有 {{ hiddenPendingNodeCount }} 个节点处于等待状态。
              </p>
            </div>
            <div
              v-if="rolloutConflictPreview.length > 0"
              class="space-y-2"
            >
              <div class="text-xs font-medium text-foreground/80">
                冲突节点
              </div>
              <div class="flex flex-wrap gap-2">
                <div
                  v-for="entry in rolloutConflictPreview"
                  :key="entry.node_id"
                  class="flex items-center gap-2 rounded-full border border-border/60 bg-background/90 px-3 py-1 text-xs"
                >
                  <span class="font-medium">{{ entry.name }}</span>
                  <Badge
                    variant="destructive"
                    class="px-1.5 py-0 text-[10px]"
                  >
                    版本冲突
                  </Badge>
                  <Button
                    v-if="canRetryUpgradeNode(entry.node)"
                    variant="ghost"
                    size="sm"
                    class="h-6 px-2 text-[11px]"
                    :disabled="isUpgradeNodeActioning(entry.node_id)"
                    @click="handleRetryUpgradeNode(entry.node)"
                  >
                    <Loader2
                      v-if="isUpgradeNodeActioning(entry.node_id)"
                      class="h-3 w-3 mr-1 animate-spin"
                    />
                    <RotateCcw
                      v-else
                      class="h-3 w-3 mr-1"
                    />
                    对齐版本
                  </Button>
                  <Button
                    v-if="canSkipUpgradeNode(entry.node)"
                    variant="ghost"
                    size="sm"
                    class="h-6 px-2 text-[11px]"
                    :disabled="isUpgradeNodeActioning(entry.node_id)"
                    @click="handleSkipUpgradeNode(entry.node)"
                  >
                    <Loader2
                      v-if="isUpgradeNodeActioning(entry.node_id)"
                      class="h-3 w-3 mr-1 animate-spin"
                    />
                    <SkipForward
                      v-else
                      class="h-3 w-3 mr-1"
                    />
                    跳过
                  </Button>
                </div>
              </div>
              <p
                v-if="hiddenConflictNodeCount > 0"
                class="text-xs text-muted-foreground"
              >
                还有 {{ hiddenConflictNodeCount }} 个节点存在版本冲突。
              </p>
            </div>
            <div
              v-if="rolloutSkippedPreview.length > 0"
              class="space-y-2"
            >
              <div class="text-xs font-medium text-foreground/80">
                已跳过节点
              </div>
              <div class="flex flex-wrap gap-2">
                <div
                  v-for="entry in rolloutSkippedPreview"
                  :key="entry.node_id"
                  class="flex items-center gap-2 rounded-full border border-border/60 bg-background/90 px-3 py-1 text-xs"
                >
                  <span class="font-medium">{{ entry.name }}</span>
                  <Badge
                    variant="outline"
                    class="px-1.5 py-0 text-[10px]"
                  >
                    已跳过
                  </Badge>
                  <Button
                    v-if="canRetryUpgradeNode(entry.node)"
                    variant="ghost"
                    size="sm"
                    class="h-6 px-2 text-[11px]"
                    :disabled="isUpgradeNodeActioning(entry.node_id)"
                    @click="handleRetryUpgradeNode(entry.node)"
                  >
                    <Loader2
                      v-if="isUpgradeNodeActioning(entry.node_id)"
                      class="h-3 w-3 mr-1 animate-spin"
                    />
                    <RotateCcw
                      v-else
                      class="h-3 w-3 mr-1"
                    />
                    恢复
                  </Button>
                </div>
              </div>
              <p
                v-if="hiddenSkippedNodeCount > 0"
                class="text-xs text-muted-foreground"
              >
                还有 {{ hiddenSkippedNodeCount }} 个节点已被人工跳过。
              </p>
            </div>
          </div>
          <div class="space-y-1 text-xs text-muted-foreground xl:text-right">
            <div class="flex items-center justify-start gap-2 xl:justify-end">
              <Button
                v-if="store.rollout.skipped_node_ids.length > 0"
                variant="outline"
                size="sm"
                class="h-8 text-xs"
                :disabled="restoringSkippedUpgradeNodes || clearingUpgradeConflicts || cancellingUpgradeRollout"
                @click="handleRestoreSkippedUpgradeNodes"
              >
                {{ restoringSkippedUpgradeNodes ? '恢复中...' : '恢复已跳过' }}
              </Button>
              <Button
                v-if="store.rollout.conflict_node_ids.length > 0"
                variant="outline"
                size="sm"
                class="h-8 text-xs"
                :disabled="clearingUpgradeConflicts || cancellingUpgradeRollout || restoringSkippedUpgradeNodes"
                @click="handleClearUpgradeConflicts"
              >
                {{ clearingUpgradeConflicts ? '清理中...' : '清空冲突' }}
              </Button>
              <Button
                variant="destructive"
                size="sm"
                class="h-8 text-xs"
                :disabled="cancellingUpgradeRollout || clearingUpgradeConflicts || restoringSkippedUpgradeNodes"
                @click="handleCancelUpgradeRollout"
              >
                {{ cancellingUpgradeRollout ? '取消中...' : '取消 Rollout' }}
              </Button>
            </div>
            <div>开始时间: {{ formatTime(store.rollout.started_at) }}</div>
            <div>最近放量: {{ formatTime(store.rollout.last_dispatched_at) }}</div>
            <div>最近更新: {{ formatTime(store.rollout.updated_at) }}</div>
            <div v-if="store.rollout.probe">
              主动探测: GET {{ store.rollout.probe.url }} · {{ store.rollout.probe.timeout_secs }}s
            </div>
            <div v-else>
              主动探测: 未配置，等待真实流量确认
            </div>
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
                  <Badge
                    v-if="nodeSchedulingBadge(node)"
                    :variant="nodeSchedulingBadge(node)!.variant"
                    class="text-[10px] px-1.5 py-0"
                  >
                    {{ nodeSchedulingBadge(node)!.label }}
                  </Badge>
                  <Badge
                    v-if="nodeRolloutBadge(node)"
                    :variant="nodeRolloutBadge(node)!.variant"
                    class="text-[10px] px-1.5 py-0"
                  >
                    {{ nodeRolloutBadge(node)!.label }}
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
                    v-if="canRetryUpgradeNode(node)"
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="isUpgradeNodeActioning(node.id) ? '处理中...' : '重试当前节点升级'"
                    :disabled="isUpgradeNodeActioning(node.id)"
                    @click="handleRetryUpgradeNode(node)"
                  >
                    <Loader2
                      v-if="isUpgradeNodeActioning(node.id)"
                      class="h-4 w-4 animate-spin"
                    />
                    <RotateCcw
                      v-else
                      class="h-4 w-4"
                    />
                  </Button>
                  <Button
                    v-if="canSkipUpgradeNode(node)"
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="isUpgradeNodeActioning(node.id) ? '处理中...' : '跳过当前节点'"
                    :disabled="isUpgradeNodeActioning(node.id)"
                    @click="handleSkipUpgradeNode(node)"
                  >
                    <Loader2
                      v-if="isUpgradeNodeActioning(node.id)"
                      class="h-4 w-4 animate-spin"
                    />
                    <SkipForward
                      v-else
                      class="h-4 w-4"
                    />
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
                <Badge
                  v-if="nodeSchedulingBadge(node)"
                  :variant="nodeSchedulingBadge(node)!.variant"
                  class="text-[10px] px-1.5 py-0"
                >
                  {{ nodeSchedulingBadge(node)!.label }}
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
              <div
                v-if="nodeRolloutBadge(node)"
                class="mt-1"
              >
                <Badge
                  :variant="nodeRolloutBadge(node)!.variant"
                  class="text-[10px] px-1.5 py-0"
                >
                  {{ nodeRolloutBadge(node)!.label }}
                </Badge>
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
                v-if="canRetryUpgradeNode(node)"
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs"
                :disabled="isUpgradeNodeActioning(node.id)"
                @click="handleRetryUpgradeNode(node)"
              >
                <Loader2
                  v-if="isUpgradeNodeActioning(node.id)"
                  class="h-3 w-3 mr-1 animate-spin"
                />
                <RotateCcw
                  v-else
                  class="h-3 w-3 mr-1"
                />
                重试
              </Button>
              <Button
                v-if="canSkipUpgradeNode(node)"
                variant="ghost"
                size="sm"
                class="h-7 px-2 text-xs"
                :disabled="isUpgradeNodeActioning(node.id)"
                @click="handleSkipUpgradeNode(node)"
              >
                <Loader2
                  v-if="isUpgradeNodeActioning(node.id)"
                  class="h-3 w-3 mr-1 animate-spin"
                />
                <SkipForward
                  v-else
                  class="h-3 w-3 mr-1"
                />
                跳过
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
              type="text"
              masked
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
          <div class="space-y-1.5">
            <Label>接单状态</Label>
            <Select v-model="configForm.scheduling_state">
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent>
                <SelectItem value="active">
                  active
                </SelectItem>
                <SelectItem value="draining">
                  draining
                </SelectItem>
                <SelectItem value="cordoned">
                  cordoned
                </SelectItem>
              </SelectContent>
            </Select>
            <p class="text-xs text-muted-foreground">
              draining/cordoned 都会停止新隧道请求；draining 用于排空，cordoned 用于人工隔离。
            </p>
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
      @update:model-value="(open: boolean) => { if (!open) { resetBatchUpgradeDialog() } }"
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
        <div class="space-y-1.5">
          <Label>升级健康探测 URL</Label>
          <Input
            v-model="batchUpgradeProbeUrl"
            placeholder="可选，例如 https://example.com/health"
          />
          <p class="text-xs text-muted-foreground">
            留空则只等待真实流量或手动确认；填写后 gateway 会通过 tunnel 主动发起 GET 探测。
          </p>
        </div>
        <div class="space-y-1.5">
          <Label>探测超时（秒）</Label>
          <Input
            v-model="batchUpgradeProbeTimeout"
            type="number"
            min="5"
            max="60"
            inputmode="numeric"
            placeholder="默认 10"
          />
          <p class="text-xs text-muted-foreground">
            仅在填写探测 URL 时生效，允许范围 5 到 60 秒。
          </p>
        </div>
      </form>
      <template #footer>
        <Button
          variant="outline"
          @click="resetBatchUpgradeDialog()"
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
import {
  proxyNodesApi,
  type ProxyNode,
  type ProxyNodeEvent,
  type ProxyNodeRemoteConfig,
  type ProxyNodeSchedulingState,
  type ProxyNodeUpgradeRolloutTrackedNode,
} from '@/api/proxy-nodes'

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

import { Search, Trash2, Plus, SquarePen, Activity, Loader2, Settings, History, RotateCcw, SkipForward } from 'lucide-vue-next'
import { parseApiError } from '@/utils/errorParser'
import { formatRegion } from '@/utils/region'
import HardwareTooltip from './components/HardwareTooltip.vue'

const { success, error: toastError } = useToast()
const { confirmDanger, confirmWarning } = useConfirm()
const store = useProxyNodesStore()

const searchQuery = ref('')
const filterStatus = ref('all')
const filterRolloutStage = ref<RolloutStageFilterValue>('all')
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
  scheduling_state: 'active' as ProxyNodeSchedulingState,
  upgrade_to: '',
})
const showBatchUpgradeDialog = ref(false)
const batchUpgradeVersion = ref('')
const batchUpgradeProbeUrl = ref('')
const batchUpgradeProbeTimeout = ref('10')
const batchUpgrading = ref(false)
const clearingUpgradeConflicts = ref(false)
const cancellingUpgradeRollout = ref(false)
const restoringSkippedUpgradeNodes = ref(false)
const actioningUpgradeNodes = ref(new Set<string>())

// 连接事件对话框
const showEventsDialog = ref(false)
const eventsNode = ref<ProxyNode | null>(null)
const nodeEvents = ref<ProxyNodeEvent[]>([])
const loadingEvents = ref(false)

// 测试连通性
const testingNodes = ref(new Set<string>())
const testingUrl = ref(false)

const rolloutTrackedNodeById = computed(() => new Map(
  (store.rollout?.tracked_nodes ?? []).map(node => [node.node_id, node] as const)
))

const rolloutConflictNodeIds = computed(() => new Set(store.rollout?.conflict_node_ids ?? []))
const rolloutCompletedNodeIds = computed(() => new Set(store.rollout?.completed_node_ids ?? []))
const rolloutSkippedNodeIds = computed(() => new Set(store.rollout?.skipped_node_ids ?? []))

const rolloutPendingPreview = computed(() => {
  if (!store.rollout) return []
  return store.rollout.pending_node_ids
    .map(nodeId => {
      const node = store.nodes.find(candidate => candidate.id === nodeId)
      if (!node) return null
      const badge = nodeRolloutBadge(node)
      return {
        node_id: node.id,
        name: node.name,
        node,
        label: badge?.label ?? '等待确认',
        variant: badge?.variant ?? 'secondary',
      }
    })
    .filter((entry): entry is {
      node_id: string
      name: string
      node: ProxyNode
      label: string
      variant: 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark'
    } => entry !== null)
    .slice(0, 6)
})

const hiddenPendingNodeCount = computed(() =>
  Math.max(0, (store.rollout?.pending_node_ids.length ?? 0) - rolloutPendingPreview.value.length)
)

const rolloutConflictPreview = computed(() => {
  if (!store.rollout) return []
  return store.rollout.conflict_node_ids
    .map(nodeId => {
      const node = store.nodes.find(candidate => candidate.id === nodeId)
      if (!node) return null
      return {
        node_id: node.id,
        name: node.name,
        node,
      }
    })
    .filter((entry): entry is {
      node_id: string
      name: string
      node: ProxyNode
    } => entry !== null)
    .slice(0, 6)
})

const hiddenConflictNodeCount = computed(() =>
  Math.max(0, (store.rollout?.conflict_node_ids.length ?? 0) - rolloutConflictPreview.value.length)
)

const rolloutSkippedPreview = computed(() => {
  if (!store.rollout) return []
  return store.rollout.skipped_node_ids
    .map(nodeId => {
      const node = store.nodes.find(candidate => candidate.id === nodeId)
      if (!node) return null
      return {
        node_id: node.id,
        name: node.name,
        node,
      }
    })
    .filter((entry): entry is {
      node_id: string
      name: string
      node: ProxyNode
    } => entry !== null)
    .slice(0, 6)
})

const hiddenSkippedNodeCount = computed(() =>
  Math.max(0, (store.rollout?.skipped_node_ids.length ?? 0) - rolloutSkippedPreview.value.length)
)

const activeRolloutStageLabel = computed(() =>
  rolloutStageOptions.find(option => option.value === filterRolloutStage.value)?.label ?? '全部阶段'
)

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

  if (filterRolloutStage.value !== 'all') {
    filtered = filtered.filter(node => nodeMatchesRolloutStage(node, filterRolloutStage.value))
  }

  return filtered
})

const paginatedNodes = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredNodes.value.slice(start, start + pageSize.value)
})

watch([searchQuery, filterStatus, filterRolloutStage], () => {
  currentPage.value = 1
})

watch(
  () => store.rollout,
  rollout => {
    if (!rollout) {
      filterRolloutStage.value = 'all'
    }
  },
)

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
    scheduling_state: rc.scheduling_state || 'active',
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
    data.scheduling_state = configForm.value.scheduling_state
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
    const probeUrl = batchUpgradeProbeUrl.value.trim()
    const rawProbeTimeout = batchUpgradeProbeTimeout.value.trim()
    const parsedProbeTimeout = rawProbeTimeout ? parseInt(rawProbeTimeout, 10) : NaN
    const probeTimeoutSecs = probeUrl && !isNaN(parsedProbeTimeout)
      ? Math.min(60, Math.max(5, parsedProbeTimeout))
      : undefined

    const result = await proxyNodesApi.batchUpgrade(version, {
      probe_url: probeUrl || undefined,
      probe_timeout_secs: probeUrl ? probeTimeoutSecs : undefined,
    })
    if (result.blocked) {
      success(`升级 rollout 正在等待 ${result.pending_node_ids.length} 个节点确认到 ${result.version}`)
    } else {
      success(`升级指令已下发：本波 ${result.updated} 个节点，跳过 ${result.skipped} 个，已完成 ${result.completed} 个，剩余 ${result.remaining} 个`)
    }
    resetBatchUpgradeDialog()
    await store.fetchNodes()
  } catch (err: unknown) {
    toastError(parseApiError(err, '批量升级下发失败'))
  } finally {
    batchUpgrading.value = false
  }
}

async function handleCancelUpgradeRollout() {
  if (!store.rollout || cancellingUpgradeRollout.value) return
  const confirmed = await confirmDanger(
    `确定要取消当前升级 rollout 吗？这会停止后续自动放量，但不会回滚已经下发到节点的升级指令。`,
    '取消升级 Rollout',
    '取消 Rollout'
  )
  if (!confirmed) return

  cancellingUpgradeRollout.value = true
  try {
    const result = await proxyNodesApi.cancelUpgradeRollout()
    await store.fetchNodes()
    if (result.cancelled) {
      success(
        `已取消 ${result.version || '当前'} rollout，待确认节点 ${(result.pending_node_ids?.length ?? 0)} 个，冲突节点 ${(result.conflict_node_ids?.length ?? 0)} 个`
      )
    } else {
      success('当前没有活动中的 rollout')
    }
  } catch (err: unknown) {
    toastError(parseApiError(err, '取消 rollout 失败'))
  } finally {
    cancellingUpgradeRollout.value = false
  }
}

async function handleClearUpgradeConflicts() {
  if (!store.rollout || clearingUpgradeConflicts.value || store.rollout.conflict_node_ids.length === 0) return
  const confirmed = await confirmWarning(
    `确定要清空 ${store.rollout.conflict_node_ids.length} 个冲突节点上的升级目标吗？这不会回滚已经下发成功的节点，但会解除当前 rollout 的冲突阻塞。`,
    '清空冲突升级目标'
  )
  if (!confirmed) return

  clearingUpgradeConflicts.value = true
  try {
    const result = await proxyNodesApi.clearUpgradeConflicts()
    await store.fetchNodes()
    if (result.cleared > 0) {
      success(
        `已清空 ${result.cleared} 个冲突节点；本次新增下发 ${result.updated} 个节点，剩余 ${result.remaining} 个`
      )
    } else {
      success('当前没有需要清理的冲突节点')
    }
  } catch (err: unknown) {
    toastError(parseApiError(err, '清空冲突失败'))
  } finally {
    clearingUpgradeConflicts.value = false
  }
}

async function handleRestoreSkippedUpgradeNodes() {
  if (!store.rollout || restoringSkippedUpgradeNodes.value || store.rollout.skipped_node_ids.length === 0) return
  const confirmed = await confirmWarning(
    `确定要恢复 ${store.rollout.skipped_node_ids.length} 个已跳过节点吗？它们会重新进入当前 rollout 队列；如果当前没有待确认节点，gateway 会按批次重新下发升级。`,
    '恢复已跳过节点'
  )
  if (!confirmed) return

  restoringSkippedUpgradeNodes.value = true
  try {
    const result = await proxyNodesApi.restoreSkippedUpgradeNodes()
    await store.fetchNodes()
    if (result.restored > 0) {
      success(
        `已恢复 ${result.restored} 个已跳过节点；本次新增下发 ${result.updated} 个节点，剩余 ${result.remaining} 个`
      )
    } else {
      success('当前没有需要恢复的已跳过节点')
    }
  } catch (err: unknown) {
    toastError(parseApiError(err, '恢复已跳过节点失败'))
  } finally {
    restoringSkippedUpgradeNodes.value = false
  }
}

async function handleSkipUpgradeNode(node: ProxyNode) {
  if (!canSkipUpgradeNode(node) || isUpgradeNodeActioning(node.id)) return
  const confirmed = await confirmWarning(
    `确定要在当前 rollout 中跳过节点 "${node.name}" 吗？这会把它排除出本轮升级，并清除当前 rollout 下发给它的升级目标。`,
    '跳过当前节点'
  )
  if (!confirmed) return

  setUpgradeNodeActioning(node.id, true)
  try {
    const result = await proxyNodesApi.skipUpgradeNode(node.id)
    await store.fetchNodes()
    success(
      `已跳过节点 ${node.name}；本次新增下发 ${result.updated} 个节点，剩余 ${result.remaining} 个`
    )
  } catch (err: unknown) {
    toastError(parseApiError(err, '跳过节点失败'))
  } finally {
    setUpgradeNodeActioning(node.id, false)
  }
}

async function handleRetryUpgradeNode(node: ProxyNode) {
  if (!canRetryUpgradeNode(node) || isUpgradeNodeActioning(node.id)) return

  setUpgradeNodeActioning(node.id, true)
  try {
    const result = await proxyNodesApi.retryUpgradeNode(node.id)
    await store.fetchNodes()
    success(
      `已重试节点 ${node.name}；当前待确认 ${result.pending_node_ids.length} 个，剩余 ${result.remaining} 个`
    )
  } catch (err: unknown) {
    toastError(parseApiError(err, '重试节点失败'))
  } finally {
    setUpgradeNodeActioning(node.id, false)
  }
}

function resetBatchUpgradeDialog() {
  showBatchUpgradeDialog.value = false
  batchUpgradeVersion.value = ''
  batchUpgradeProbeUrl.value = ''
  batchUpgradeProbeTimeout.value = '10'
}

async function handleDelete(node: ProxyNode) {
  const confirmed = await confirmDanger(
    `确定要删除代理节点 "${node.name}" (${node.tunnel_mode ? node.ip : `${node.ip}:${node.port}`}) 吗？`,
    '删除节点'
  )
  if (!confirmed) return

  try {
    const result = await proxyNodesApi.deleteProxyNode(node.id)
    await store.fetchNodes()
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

type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark'
type RolloutStageFilterValue =
  | 'all'
  | 'pending'
  | 'conflict'
  | 'skipped'
  | 'completed'
  | 'awaiting_version'
  | 'awaiting_traffic'
  | 'cooling_down'
  | 'unhealthy'
  | 'ready_to_finalize'

const rolloutStageOptions: Array<{ value: RolloutStageFilterValue; label: string }> = [
  { value: 'all', label: '全部阶段' },
  { value: 'pending', label: '全部卡点' },
  { value: 'awaiting_version', label: '等待版本' },
  { value: 'awaiting_traffic', label: '等待流量' },
  { value: 'cooling_down', label: '冷却中' },
  { value: 'unhealthy', label: '健康失败' },
  { value: 'ready_to_finalize', label: '等待收尾' },
  { value: 'conflict', label: '版本冲突' },
  { value: 'skipped', label: '已跳过' },
  { value: 'completed', label: '已完成' },
]

function normalizeVersion(value: string | null | undefined) {
  const trimmed = (value || '').trim().toLowerCase()
  return trimmed.startsWith('proxy-v') ? trimmed.slice('proxy-v'.length) : trimmed
}

function formatDurationShort(seconds: number | null | undefined) {
  if (seconds == null || !Number.isFinite(seconds) || seconds <= 0) return '0s'
  if (seconds < 60) return `${Math.floor(seconds)}s`
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`
  return `${Math.floor(seconds / 3600)}h`
}

function resolveTrackedNodeBadge(
  tracked: ProxyNodeUpgradeRolloutTrackedNode,
): { label: string; variant: BadgeVariant } {
  switch (tracked.state) {
    case 'awaiting_version':
      return { label: '等待版本上报', variant: 'secondary' }
    case 'awaiting_traffic':
      return { label: '等待流量确认', variant: 'warning' }
    case 'cooling_down':
      return {
        label: tracked.cooldown_remaining_secs && tracked.cooldown_remaining_secs > 0
          ? `冷却中 ${formatDurationShort(tracked.cooldown_remaining_secs)}`
          : '冷却中',
        variant: 'warning',
      }
    case 'unhealthy':
      return { label: '健康检查未过', variant: 'destructive' }
    case 'ready_to_finalize':
      return { label: '等待收尾', variant: 'success' }
    default:
      return { label: '升级中', variant: 'secondary' }
  }
}

function nodeRolloutBadge(node: ProxyNode): { label: string; variant: BadgeVariant } | null {
  const rollout = store.rollout
  if (!rollout || node.is_manual || !node.tunnel_mode) return null
  if (rolloutSkippedNodeIds.value.has(node.id)) {
    return { label: '已跳过', variant: 'outline' }
  }
  if (rolloutConflictNodeIds.value.has(node.id)) {
    return { label: '版本冲突', variant: 'destructive' }
  }
  const tracked = rolloutTrackedNodeById.value.get(node.id)
  if (tracked) {
    return resolveTrackedNodeBadge(tracked)
  }
  if (rolloutCompletedNodeIds.value.has(node.id)) {
    const nodeVersion = normalizeVersion(nodeProxyVersion(node))
    if (nodeVersion && nodeVersion === normalizeVersion(rollout.version)) {
      return { label: '已确认完成', variant: 'success' }
    }
    return { label: '目标版本', variant: 'outline' }
  }
  const targetVersion = normalizeVersion(rollout.version)
  const remoteUpgradeTarget = normalizeVersion(node.remote_config?.upgrade_to ?? null)
  if (remoteUpgradeTarget && remoteUpgradeTarget === targetVersion) {
    return { label: '已下发升级', variant: 'secondary' }
  }
  return null
}

function nodeSchedulingBadge(node: ProxyNode): { label: string; variant: BadgeVariant } | null {
  switch (node.remote_config?.scheduling_state) {
    case 'draining':
      return { label: '排空中', variant: 'warning' }
    case 'cordoned':
      return { label: '已封锁', variant: 'dark' }
    default:
      return null
  }
}

function rolloutStageOfNode(node: ProxyNode): Exclude<RolloutStageFilterValue, 'all'> | null {
  const rollout = store.rollout
  if (!rollout || node.is_manual || !node.tunnel_mode) return null
  if (rolloutSkippedNodeIds.value.has(node.id)) {
    return 'skipped'
  }
  if (rolloutConflictNodeIds.value.has(node.id)) {
    return 'conflict'
  }
  const tracked = rolloutTrackedNodeById.value.get(node.id)
  if (tracked) {
    return tracked.state
  }
  if (rolloutCompletedNodeIds.value.has(node.id)) {
    return 'completed'
  }
  if (rollout.pending_node_ids.includes(node.id)) {
    return 'pending'
  }
  const targetVersion = normalizeVersion(rollout.version)
  const remoteUpgradeTarget = normalizeVersion(node.remote_config?.upgrade_to ?? null)
  if (remoteUpgradeTarget && remoteUpgradeTarget === targetVersion) {
    return 'pending'
  }
  return null
}

function nodeMatchesRolloutStage(node: ProxyNode, stage: RolloutStageFilterValue) {
  if (stage === 'all') return true
  const resolvedStage = rolloutStageOfNode(node)
  if (!resolvedStage) return false
  if (stage === 'pending') {
    return resolvedStage === 'pending'
      || resolvedStage === 'awaiting_version'
      || resolvedStage === 'awaiting_traffic'
      || resolvedStage === 'cooling_down'
      || resolvedStage === 'unhealthy'
      || resolvedStage === 'ready_to_finalize'
  }
  return resolvedStage === stage
}

function isUpgradeNodeActioning(nodeId: string) {
  return actioningUpgradeNodes.value.has(nodeId)
}

function setUpgradeNodeActioning(nodeId: string, active: boolean) {
  const next = new Set(actioningUpgradeNodes.value)
  if (active) {
    next.add(nodeId)
  } else {
    next.delete(nodeId)
  }
  actioningUpgradeNodes.value = next
}

function canSkipUpgradeNode(node: ProxyNode) {
  const rollout = store.rollout
  if (!rollout || node.is_manual || !node.tunnel_mode) return false
  if (rolloutSkippedNodeIds.value.has(node.id) || rolloutCompletedNodeIds.value.has(node.id)) {
    return false
  }
  return rolloutConflictNodeIds.value.has(node.id)
    || rolloutTrackedNodeById.value.has(node.id)
    || rollout.pending_node_ids.includes(node.id)
}

function canRetryUpgradeNode(node: ProxyNode) {
  const rollout = store.rollout
  if (!rollout || node.is_manual || !node.tunnel_mode) return false
  if (rolloutCompletedNodeIds.value.has(node.id)) return false
  return rolloutSkippedNodeIds.value.has(node.id)
    || rolloutConflictNodeIds.value.has(node.id)
    || rolloutTrackedNodeById.value.has(node.id)
}
</script>
