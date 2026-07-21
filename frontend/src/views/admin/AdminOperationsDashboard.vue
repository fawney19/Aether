<template>
  <div class="space-y-5 px-4 pb-8 sm:px-6 lg:px-0">
    <div class="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
      <div>
        <h1 class="text-lg font-semibold">
          运维总览
        </h1>
        <p class="text-xs text-muted-foreground">
          统一查看流量、吞吐、延迟、错误、上游健康、缓存与审计风险
        </p>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <span class="text-xs text-muted-foreground">
          更新 {{ lastUpdatedLabel }}
        </span>
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8 shrink-0"
          :class="autoRefresh ? 'text-primary' : ''"
          :title="autoRefresh ? '点击关闭自动刷新' : '点击开启自动刷新'"
          :aria-label="autoRefresh ? '关闭自动刷新' : '开启自动刷新'"
          :aria-pressed="autoRefresh"
          @click="toggleAutoRefresh"
        >
          <RefreshCw
            class="h-3.5 w-3.5"
            :class="autoRefresh || refreshing ? 'animate-spin' : ''"
          />
        </Button>
        <TimeRangePicker
          v-model="timeRange"
          :allow-hourly="true"
        />
      </div>
    </div>

    <div
      v-if="loadWarning"
      class="rounded-lg border border-yellow-300/70 bg-yellow-50/80 px-3 py-2 text-xs text-yellow-900 dark:border-yellow-900/60 dark:bg-yellow-950/30 dark:text-yellow-100"
    >
      {{ loadWarning }}
    </div>

    <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4 2xl:grid-cols-8">
      <Card
        v-for="card in kpiCards"
        :key="card.title"
        class="min-h-[118px] p-4"
      >
        <div class="flex items-start justify-between gap-3">
          <div class="min-w-0">
            <p class="truncate text-xs text-muted-foreground">
              {{ card.title }}
            </p>
            <div
              class="mt-2 truncate text-2xl font-semibold tabular-nums"
              :class="card.valueClass"
            >
              {{ card.value }}
            </div>
          </div>
          <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-border/60 bg-muted/30">
            <component
              :is="card.icon"
              class="h-4 w-4"
              :class="card.iconClass"
            />
          </div>
        </div>
        <p class="mt-3 line-clamp-2 text-xs text-muted-foreground">
          {{ card.hint }}
        </p>
      </Card>
    </div>

    <div class="grid grid-cols-1 gap-4 xl:grid-cols-3">
      <Card class="overflow-hidden xl:col-span-2">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <h2 class="text-sm font-semibold">
                流量与吞吐
              </h2>
              <p class="text-xs text-muted-foreground">
                请求、Token 与费用趋势，按当前时间窗口聚合
              </p>
            </div>
            <Badge variant="outline">
              {{ timeRange.granularity || 'day' }}
            </Badge>
          </div>
        </div>
        <div class="p-4">
          <div
            v-if="trendLoading"
            class="py-12"
          >
            <LoadingState message="加载流量趋势中" />
          </div>
          <div
            v-else
            class="h-[302px]"
          >
            <LineChart
              :data="trafficChartData"
              :options="trafficChartOptions"
            />
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                实时并发
              </h2>
              <p class="text-xs text-muted-foreground">
                网关、全局锁与代理通道
              </p>
            </div>
            <Badge :variant="distributedGateVariant">
              {{ distributedGateText }}
            </Badge>
          </div>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3 text-sm">
            <MetricCell
              label="本机处理中"
              :value="formatMetricNumber(gatewayMetrics?.local.inFlight)"
            />
            <MetricCell
              label="本机可接入"
              :value="formatMetricNumber(gatewayMetrics?.local.availablePermits)"
            />
            <MetricCell
              label="全局处理中"
              :value="formatMetricNumber(gatewayMetrics?.distributed.inFlight)"
            />
            <MetricCell
              label="全局可接入"
              :value="formatMetricNumber(gatewayMetrics?.distributed.availablePermits)"
            />
            <MetricCell
              label="代理活跃流"
              :value="formatMetricNumber(currentActiveStreams)"
            />
            <MetricCell
              label="代理连接"
              :value="formatMetricNumber(currentProxyConnections)"
            />
          </div>
          <div class="rounded-lg border border-border/60 bg-background/45 px-3 py-3">
            <div class="flex items-center justify-between gap-3 text-xs">
              <span class="text-muted-foreground">队列利用率</span>
              <span class="font-medium tabular-nums">{{ tunnelQueueUtilizationText }}</span>
            </div>
            <div class="mt-2 h-2 overflow-hidden rounded-full bg-muted">
              <div
                class="h-full rounded-full bg-primary"
                :style="{ width: tunnelQueueUtilizationWidth }"
              />
            </div>
            <div class="mt-3 grid grid-cols-2 gap-2 text-xs text-muted-foreground">
              <span>拒绝 {{ formatMetricNumber(tunnelQueueRejectedTotal) }}</span>
              <span>选择压力 {{ formatMetricNumber(tunnelSelectionPressureTotal) }}</span>
            </div>
          </div>
        </div>
      </Card>
    </div>

    <div class="grid grid-cols-1 gap-4 xl:grid-cols-4 2xl:grid-cols-5">
      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                Admission
              </h2>
              <p class="text-xs text-muted-foreground">
                本机、分布式、候选与上游执行 Gate
              </p>
            </div>
            <Badge :variant="admissionStatusVariant">
              {{ admissionStatusText }}
            </Badge>
          </div>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3">
            <MetricCell
              label="本机 Gate"
              :value="localGateUtilizationText"
              :value-class="capacityPercentClass(localGateUtilization)"
            />
            <MetricCell
              label="分布式 Gate"
              :value="distributedGateUtilizationText"
              :value-class="capacityPercentClass(distributedGateUtilization)"
            />
            <MetricCell
              label="候选规划"
              :value="candidatePlanningGateUtilizationText"
              :value-class="capacityPercentClass(candidatePlanningGateUtilization)"
            />
            <MetricCell
              label="上游执行"
              :value="upstreamExecutionGateUtilizationText"
              :value-class="capacityPercentClass(upstreamExecutionGateUtilization)"
            />
          </div>
          <div class="grid grid-cols-2 gap-2 text-xs text-muted-foreground">
            <span>本机拒绝 {{ formatMetricNumber(gatewayMetrics?.local.rejectedTotal) }}</span>
            <span>上游拒绝 {{ formatMetricNumber(gatewayMetrics?.upstreamExecution.rejectedTotal) }}</span>
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                DB Pool
              </h2>
              <p class="text-xs text-muted-foreground">
                前台路径共享连接预算
              </p>
            </div>
            <Badge :variant="databasePoolStatusVariant">
              {{ databasePoolStatusText }}
            </Badge>
          </div>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3">
            <MetricCell
              label="使用率"
              :value="databasePoolUsageText"
              :value-class="capacityPercentClass(databasePoolUsagePercent)"
            />
            <MetricCell
              label="占用 / 上限"
              :value="databasePoolCheckedOutText"
            />
            <MetricCell
              label="空闲连接"
              :value="formatMetricNumber(gatewayMetrics?.databasePool.idle)"
            />
            <MetricCell
              label="预留空闲"
              :value="formatMetricNumber(gatewayMetrics?.databasePool.idleReserve)"
            />
            <MetricCell
              label="PG 等待"
              :value="postgresWaitingText"
              :value-class="counterRiskClass(gatewayMetrics?.postgres.waitingConnections)"
            />
            <MetricCell
              label="PG 锁等待"
              :value="formatMetricNumber(gatewayMetrics?.postgres.lockWaitingConnections)"
              :value-class="counterRiskClass(gatewayMetrics?.postgres.lockWaitingConnections)"
            />
            <MetricCell
              label="最长查询"
              :value="postgresOldestActiveQueryText"
              :value-class="latencyToneClass(gatewayMetrics?.postgres.oldestActiveQueryAgeMs)"
            />
            <MetricCell
              label="最长事务"
              :value="postgresOldestTransactionText"
              :value-class="latencyToneClass(gatewayMetrics?.postgres.oldestTransactionAgeMs)"
            />
            <MetricCell
              label="PG 命中"
              :value="postgresCacheHitText"
              :value-class="postgresCacheHitClass"
            />
            <MetricCell
              label="临时写"
              :value="formatBytes(gatewayMetrics?.postgres.tempBytesTotal)"
            />
            <MetricCell
              label="WAL"
              :value="postgresWalText"
              :value-class="postgresWalClass"
            />
            <MetricCell
              label="Checkpoint"
              :value="postgresCheckpointText"
              :value-class="postgresCheckpointClass"
            />
            <MetricCell
              label="Top SQL"
              :value="postgresTopStatementText"
              :value-class="postgresStatementClass"
            />
            <MetricCell
              label="PG 回滚"
              :value="formatMetricNumber(gatewayMetrics?.postgres.xactRollbackTotal)"
              :value-class="counterRiskClass(gatewayMetrics?.postgres.xactRollbackTotal)"
            />
          </div>
          <div class="text-xs text-muted-foreground">
            Driver {{ gatewayMetrics?.databasePool.driver || '-' }} · Pool size {{ formatMetricNumber(gatewayMetrics?.databasePool.size) }} · PG active {{ formatMetricNumber(gatewayMetrics?.postgres.activeConnections) }} · idle tx {{ formatMetricNumber(gatewayMetrics?.postgres.idleInTransactionConnections) }} · deadlocks {{ formatMetricNumber(gatewayMetrics?.postgres.deadlocksTotal) }} · top calls {{ formatMetricNumber(gatewayMetrics?.postgres.statementTopCallsTotal) }}
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                Gateway 进程
              </h2>
              <p class="text-xs text-muted-foreground">
                CPU、RSS、FD 与 TCP 连接边界
              </p>
            </div>
            <Badge :variant="gatewayProcessStatusVariant">
              {{ gatewayProcessStatusText }}
            </Badge>
          </div>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3">
            <MetricCell
              label="进程 CPU"
              :value="formatBasisPointsPercent(gatewayMetrics?.process.processCpuUsageBasisPoints)"
              :value-class="resourceToneClass(gatewayProcessCpuPercent, 70, 90)"
            />
            <MetricCell
              label="RSS"
              :value="formatBytes(gatewayMetrics?.process.processMemoryBytes)"
              :value-class="resourceToneClass(gatewayProcessMemoryPercent, 70, 90)"
            />
            <MetricCell
              label="Heap allocated"
              :value="formatBytes(gatewayMetrics?.allocator.allocatedBytes)"
            />
            <MetricCell
              label="Heap resident"
              :value="formatBytes(gatewayMetrics?.allocator.residentBytes)"
            />
            <MetricCell
              label="Heap active"
              :value="formatBasisPointsPercent(gatewayAllocatorActivePercentBasisPoints)"
              :value-class="resourceToneClass(gatewayAllocatorActivePercent, 125, 200)"
            />
            <MetricCell
              label="线程"
              :value="formatMetricNumber(gatewayMetrics?.process.processThreads)"
            />
            <MetricCell
              label="后台任务"
              :value="gatewayBackgroundTaskText"
            />
            <MetricCell
              label="Tokio 任务"
              :value="formatMetricNumber(gatewayMetrics?.tokioRuntime.aliveTasks)"
            />
            <MetricCell
              label="Runtime 队列"
              :value="formatMetricNumber(gatewayMetrics?.tokioRuntime.globalQueueDepth)"
            />
            <MetricCell
              label="任务退出"
              :value="formatMetricNumber(gatewayBackgroundTaskUnexpectedExits)"
              :value-class="counterRiskClass(gatewayBackgroundTaskUnexpectedExits)"
            />
            <MetricCell
              label="FD 使用"
              :value="gatewayProcessFdText"
              :value-class="resourceToneClass(gatewayProcessFdPercent, 60, 70)"
            />
            <MetricCell
              label="Socket FD"
              :value="gatewayProcessSocketFdText"
            />
            <MetricCell
              label="TCP 连接"
              :value="gatewayProcessTcpText"
            />
            <MetricCell
              label="CLOSE_WAIT"
              :value="formatMetricNumber(gatewayMetrics?.process.processTcpCloseWaitConnections)"
              :value-class="counterRiskClass(gatewayMetrics?.process.processTcpCloseWaitConnections)"
            />
            <MetricCell
              label="系统内存"
              :value="formatBasisPointsPercent(gatewayMetrics?.process.systemMemoryUsageBasisPoints)"
              :value-class="resourceToneClass(gatewaySystemMemoryPercent, 75, 90)"
            />
            <MetricCell
              label="网卡流量"
              :value="gatewayNetworkTrafficText"
            />
          </div>
          <div class="text-xs text-muted-foreground">
            运行 {{ gatewayProcessUptimeText }} · 虚拟内存 {{ formatBytes(gatewayMetrics?.process.processVirtualMemoryBytes) }} · TCP {{ gatewayTcpStateText }} · allocator {{ gatewayAllocatorStatusText }} · supervisor {{ formatMetricNumber(gatewayMetrics?.backgroundTasks.supervisedTotal) }} · Tokio workers {{ formatMetricNumber(gatewayMetrics?.tokioRuntime.workers) }}
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                Usage 与队列
              </h2>
              <p class="text-xs text-muted-foreground">
                账务写入和 request candidate 削峰
              </p>
            </div>
            <Badge :variant="queueStatusVariant">
              {{ queueStatusText }}
            </Badge>
          </div>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3">
            <MetricCell
              label="Usage Worker"
              :value="usageWorkerText"
            />
            <MetricCell
              label="Usage ACK"
              :value="formatMetricNumber(gatewayMetrics?.usageRuntime.workerAckedEntriesTotal)"
            />
            <MetricCell
              label="Usage Lag"
              :value="formatMetricNumber(gatewayMetrics?.usageQueue.groupLag)"
              :value-class="counterRiskClass(gatewayMetrics?.usageQueue.groupLag)"
            />
            <MetricCell
              label="Usage DLQ"
              :value="formatMetricNumber(gatewayMetrics?.usageQueue.dlqLength)"
              :value-class="counterRiskClass(gatewayMetrics?.usageQueue.dlqLength)"
            />
            <MetricCell
              label="Outbox 待处理"
              :value="formatMetricNumber(gatewayMetrics?.usageCounter.pendingRows)"
              :value-class="counterRiskClass(gatewayMetrics?.usageCounter.pendingRows)"
            />
            <MetricCell
              label="Outbox 已刷"
              :value="formatMetricNumber(gatewayMetrics?.usageCounter.flushRowsClaimedTotal)"
            />
            <MetricCell
              label="最老待处理"
              :value="usageCounterOldestPendingAgeText"
              :value-class="usageCounterOldestPendingAgeClass"
            />
            <MetricCell
              label="Outbox 失败"
              :value="formatMetricNumber(usageCounterOutboxFailures)"
              :value-class="counterRiskClass(usageCounterOutboxFailures)"
            />
            <MetricCell
              label="Terminal 失败"
              :value="formatMetricNumber(gatewayMetrics?.usageRuntime.terminalEnqueueFailedTotal)"
              :value-class="counterRiskClass(gatewayMetrics?.usageRuntime.terminalEnqueueFailedTotal)"
            />
            <MetricCell
              label="Worker 异常"
              :value="formatMetricNumber(usageRuntimeWorkerFaults)"
              :value-class="counterRiskClass(usageRuntimeWorkerFaults)"
            />
            <MetricCell
              label="Candidate 水位"
              :value="candidateQueueUtilizationText"
              :value-class="capacityPercentClass(candidateQueueUtilization)"
            />
            <MetricCell
              label="Candidate 待刷"
              :value="formatMetricNumber(gatewayMetrics?.requestCandidateQueue.pendingDepth)"
            />
            <MetricCell
              label="Candidate 已刷"
              :value="formatMetricNumber(gatewayMetrics?.requestCandidateQueue.flushedTotal)"
            />
            <MetricCell
              label="Candidate 合并"
              :value="formatMetricNumber(gatewayMetrics?.requestCandidateQueue.compactedTotal)"
            />
          </div>
          <div class="rounded-lg border border-border/60 bg-background/45 px-3 py-3">
            <div class="flex items-center justify-between gap-3 text-xs">
              <span class="text-muted-foreground">Candidate depth</span>
              <span class="font-medium tabular-nums">{{ candidateQueueDepthText }}</span>
            </div>
            <div class="mt-2 h-2 overflow-hidden rounded-full bg-muted">
              <div
                class="h-full rounded-full bg-primary"
                :style="{ width: candidateQueueUtilizationWidth }"
              />
            </div>
            <div class="mt-3 grid grid-cols-2 gap-2 text-xs text-muted-foreground">
              <span>Outbox processed {{ formatMetricNumber(gatewayMetrics?.usageCounter.processedRows) }}</span>
              <span>Outbox health {{ usageCounterHealthText }}</span>
              <span>Flush batches {{ formatMetricNumber(gatewayMetrics?.usageCounter.flushBatchesTotal) }}</span>
              <span>Flush targets {{ formatMetricNumber(gatewayMetrics?.usageCounter.flushTargetsTotal) }}</span>
              <span>Worker read {{ formatMetricNumber(gatewayMetrics?.usageRuntime.workerReadEntriesTotal) }}</span>
              <span>Worker reclaim {{ formatMetricNumber(gatewayMetrics?.usageRuntime.workerReclaimedEntriesTotal) }}</span>
              <span>Usage pending {{ formatMetricNumber(gatewayMetrics?.usageQueue.groupPending) }}</span>
              <span>Pending idle {{ usageQueueOldestPendingIdleText }}</span>
              <span>Drop {{ formatMetricNumber(gatewayMetrics?.requestCandidateQueue.droppedTotal) }}</span>
              <span>Fallback {{ formatMetricNumber(gatewayMetrics?.requestCandidateQueue.syncFallbackTotal) }}</span>
              <span>Flush batches {{ formatMetricNumber(gatewayMetrics?.requestCandidateQueue.flushBatchesTotal) }}</span>
              <span>SQL ops {{ formatMetricNumber(gatewayMetrics?.requestCandidateQueue.flushSqlOpsTotal) }}</span>
            </div>
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                Upstream Target
              </h2>
              <p class="text-xs text-muted-foreground">
                单 target 饱和隔离和选择压力
              </p>
            </div>
            <Badge :variant="upstreamTargetStatusVariant">
              {{ upstreamTargetStatusText }}
            </Badge>
          </div>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3">
            <MetricCell
              label="活跃 Target"
              :value="formatMetricNumber(gatewayMetrics?.upstreamTargets.activeTargets)"
            />
            <MetricCell
              label="单 Target 上限"
              :value="formatMetricNumber(gatewayMetrics?.upstreamTargets.limit)"
            />
            <MetricCell
              label="饱和累计"
              :value="formatMetricNumber(gatewayMetrics?.upstreamTargets.saturatedTotal)"
              :value-class="counterRiskClass(gatewayMetrics?.upstreamTargets.saturatedTotal)"
            />
            <MetricCell
              label="拒绝累计"
              :value="formatMetricNumber(gatewayMetrics?.upstreamTargets.rejectedTotal)"
              :value-class="counterRiskClass(gatewayMetrics?.upstreamTargets.rejectedTotal)"
            />
          </div>
          <div class="space-y-2">
            <div
              v-for="target in upstreamTargetRows"
              :key="target.target"
              class="rounded-lg border border-border/50 bg-background/45 px-3 py-2 text-xs"
            >
              <div class="flex items-center justify-between gap-3">
                <span class="min-w-0 truncate font-medium">{{ target.target }}</span>
                <span class="shrink-0 tabular-nums">{{ formatMetricNumber(target.inFlight) }} in-flight</span>
              </div>
              <div class="mt-1 flex items-center justify-between gap-3 text-muted-foreground">
                <span>available {{ formatMetricNumber(target.availablePermits) }}</span>
                <span>saturated {{ formatMetricNumber(target.saturatedTotal) }}</span>
              </div>
            </div>
            <div
              v-if="upstreamTargetRows.length === 0"
              class="rounded-lg border border-dashed border-border/60 px-3 py-6 text-center text-xs text-muted-foreground"
            >
              暂无 target 样本
            </div>
          </div>
        </div>
      </Card>
    </div>

    <Card class="overflow-hidden">
      <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
        <div class="flex items-center justify-between gap-3">
          <div>
            <h2 class="text-sm font-semibold">
              关键路径 Stage
            </h2>
            <p class="text-xs text-muted-foreground">
              入口排队、候选规划、上游 Gate 和流式总耗时
            </p>
          </div>
          <Badge variant="outline">
            Gateway latency
          </Badge>
        </div>
      </div>
      <div class="overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Stage</TableHead>
              <TableHead class="text-right">
                样本
              </TableHead>
              <TableHead class="text-right">
                平均
              </TableHead>
              <TableHead class="text-right">
                最大
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="stage in stageLatencyRows"
              :key="stage.stage"
            >
              <TableCell>
                <div class="text-sm font-medium">
                  {{ stage.label }}
                </div>
                <div class="text-xs text-muted-foreground">
                  {{ stage.stage }}
                </div>
              </TableCell>
              <TableCell class="text-right tabular-nums">
                {{ formatMetricNumber(stage.count) }}
              </TableCell>
              <TableCell
                class="text-right tabular-nums"
                :class="latencyToneClass(stage.avgMs)"
              >
                {{ formatMs(stage.avgMs) }}
              </TableCell>
              <TableCell
                class="text-right tabular-nums"
                :class="latencyToneClass(stage.maxMs)"
              >
                {{ formatMs(stage.maxMs) }}
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </Card>

    <div class="grid grid-cols-1 gap-4 xl:grid-cols-3">
      <Card class="overflow-hidden xl:col-span-2">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <h2 class="text-sm font-semibold">
                延迟与首字
              </h2>
              <p class="text-xs text-muted-foreground">
                P50/P90/P99 请求耗时与 TTFT 趋势
              </p>
            </div>
            <RouterLink
              to="/admin/performance-analysis"
              class="text-xs font-medium text-primary hover:underline"
            >
              查看性能分析
            </RouterLink>
          </div>
        </div>
        <div class="p-4">
          <div
            v-if="percentileLoading"
            class="py-12"
          >
            <LoadingState message="加载延迟百分位中" />
          </div>
          <div
            v-else
            class="h-[288px]"
          >
            <LineChart
              :data="latencyChartData"
              :options="latencyChartOptions"
            />
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <h2 class="text-sm font-semibold">
            SLA 与错误
          </h2>
          <p class="text-xs text-muted-foreground">
            请求错误、上游错误与熔断风险
          </p>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3">
            <MetricCell
              label="成功率"
              :value="formatPercent(providerPerformance?.summary.success_rate)"
              :value-class="slaValueClass"
            />
            <MetricCell
              label="错误率"
              :value="formatErrorRate(providerPerformance?.summary.success_rate)"
              :value-class="errorRateValueClass"
            />
            <MetricCell
              label="已分类错误"
              :value="formatMetricNumber(classifiedErrorCount)"
            />
            <MetricCell
              label="熔断打开"
              :value="formatMetricNumber(resilienceStatus?.error_statistics.open_circuit_breakers)"
            />
          </div>

          <div>
            <div class="mb-2 flex items-center justify-between gap-3">
              <span class="text-xs font-medium text-muted-foreground">错误分类</span>
              <RouterLink
                to="/admin/audit-logs"
                class="text-xs font-medium text-primary hover:underline"
              >
                审计
              </RouterLink>
            </div>
            <div class="h-[154px]">
              <DoughnutChart
                :data="errorDistributionChartData"
                :options="errorDistributionOptions"
              />
            </div>
          </div>
        </div>
      </Card>
    </div>

    <div class="grid grid-cols-1 gap-4 xl:grid-cols-3">
      <Card class="overflow-hidden xl:col-span-2">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
            <div>
              <h2 class="text-sm font-semibold">
                上游健康与吞吐
              </h2>
              <p class="text-xs text-muted-foreground">
                Provider 维度的 TPS、TTFT、慢请求与错误样本
              </p>
            </div>
            <RouterLink
              to="/admin/health-monitor"
              class="text-xs font-medium text-primary hover:underline"
            >
              健康监控
            </RouterLink>
          </div>
        </div>
        <div class="overflow-x-auto">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>上游</TableHead>
                <TableHead class="text-right">
                  请求
                </TableHead>
                <TableHead class="text-right">
                  成功率
                </TableHead>
                <TableHead class="text-right">
                  TPS
                </TableHead>
                <TableHead class="text-right">
                  TTFT
                </TableHead>
                <TableHead class="text-right">
                  P99
                </TableHead>
                <TableHead class="text-right">
                  慢请求
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow
                v-for="provider in providerRows"
                :key="provider.provider_id"
              >
                <TableCell>
                  <div class="max-w-[220px] truncate text-sm font-medium">
                    {{ provider.provider }}
                  </div>
                </TableCell>
                <TableCell class="text-right tabular-nums">
                  {{ formatMetricNumber(provider.request_count) }}
                </TableCell>
                <TableCell
                  class="text-right tabular-nums"
                  :class="successRateClass(provider.success_rate)"
                >
                  {{ formatPercent(provider.success_rate) }}
                </TableCell>
                <TableCell class="text-right tabular-nums">
                  {{ formatTps(provider.avg_output_tps) }}
                </TableCell>
                <TableCell class="text-right tabular-nums">
                  {{ formatMs(provider.avg_first_byte_time_ms) }}
                </TableCell>
                <TableCell class="text-right tabular-nums">
                  {{ formatMs(provider.p99_response_time_ms) }}
                </TableCell>
                <TableCell class="text-right tabular-nums">
                  {{ formatMetricNumber(provider.slow_request_count) }}
                </TableCell>
              </TableRow>
              <TableRow v-if="providerRows.length === 0">
                <TableCell
                  colspan="7"
                  class="py-8 text-center text-sm text-muted-foreground"
                >
                  当前时间窗口暂无上游样本
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                资源、数据流与缓存
              </h2>
              <p class="text-xs text-muted-foreground">
                代理节点资源、WebSocket 流量、Redis Key 分类与缓存命中
              </p>
            </div>
            <RouterLink
              to="/admin/cache-monitoring"
              class="text-xs font-medium text-primary hover:underline"
            >
              缓存页
            </RouterLink>
          </div>
        </div>
        <div class="space-y-4 p-4">
          <div class="grid grid-cols-2 gap-3">
            <MetricCell
              label="CPU"
              :value="formatPercentResource(resourceSnapshot?.avgCpuPercent)"
              :value-class="resourceToneClass(resourceSnapshot?.avgCpuPercent, 70, 90)"
            />
            <MetricCell
              label="内存"
              :value="formatPercentResource(resourceSnapshot?.avgMemoryPercent)"
              :value-class="resourceToneClass(resourceSnapshot?.avgMemoryPercent, 75, 90)"
            />
            <MetricCell
              label="WS 入站"
              :value="formatBytes(resourceSnapshot?.wsInBytes)"
            />
            <MetricCell
              label="WS 出站"
              :value="formatBytes(resourceSnapshot?.wsOutBytes)"
            />
            <MetricCell
              label="网关 TCP"
              :value="gatewayProcessTcpText"
            />
            <MetricCell
              label="网络异常"
              :value="formatMetricNumber(gatewayNetworkFaults)"
              :value-class="counterRiskClass(gatewayNetworkFaults)"
            />
            <MetricCell
              label="Redis 状态"
              :value="redisStatusText"
              :value-class="redisStatusClass"
            />
            <MetricCell
              label="Redis 内存"
              :value="redisRuntimeMemoryText"
              :value-class="capacityPercentClass(redisRuntimeMemoryPercent)"
            />
            <MetricCell
              label="Redis OPS"
              :value="redisRuntimeOpsText"
            />
            <MetricCell
              label="Redis 延迟"
              :value="redisRuntimeLatencyText"
              :value-class="latencyToneClass(gatewayMetrics?.redisRuntime.nonblockingCommandLatencyMaxMs)"
            />
            <MetricCell
              label="Redis 客户端"
              :value="redisRuntimeClientsText"
            />
            <MetricCell
              label="Redis 命令异常"
              :value="formatMetricNumber(redisRuntimeCommandFaults)"
              :value-class="counterRiskClass(redisRuntimeCommandFaults)"
            />
            <MetricCell
              label="Redis Keys"
              :value="formatMetricNumber(redisCategories?.total_keys)"
            />
            <MetricCell
              label="亲和缓存"
              :value="formatMetricNumber(cacheStats?.affinity_stats.total_affinities)"
            />
            <MetricCell
              label="命中率"
              :value="formatPercent(cacheHitRate)"
            />
          </div>

          <div class="space-y-2">
            <div
              v-for="item in redisCategoryRows"
              :key="item.key"
              class="flex items-center justify-between gap-3 rounded-lg border border-border/50 bg-background/45 px-3 py-2 text-xs"
            >
              <div class="min-w-0">
                <div class="truncate font-medium">
                  {{ item.name }}
                </div>
                <div class="truncate text-muted-foreground">
                  {{ item.pattern }}
                </div>
              </div>
              <span class="font-semibold tabular-nums">{{ formatMetricNumber(item.count) }}</span>
            </div>
          </div>
        </div>
      </Card>
    </div>

    <div class="grid grid-cols-1 gap-4 xl:grid-cols-3">
      <Card class="overflow-hidden xl:col-span-2">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h2 class="text-sm font-semibold">
                错误审计
              </h2>
              <p class="text-xs text-muted-foreground">
                最近错误、熔断事件和可疑活动入口
              </p>
            </div>
            <RouterLink
              to="/admin/usage?status=error"
              class="text-xs font-medium text-primary hover:underline"
            >
              使用记录
            </RouterLink>
          </div>
        </div>
        <div class="divide-y divide-border/50">
          <div
            v-for="error in recentErrors"
            :key="error.error_id"
            class="grid grid-cols-1 gap-2 px-4 py-3 text-sm lg:grid-cols-[180px_1fr_120px]"
          >
            <div class="font-medium">
              {{ error.error_type }}
            </div>
            <div class="min-w-0 text-xs text-muted-foreground">
              <div class="truncate">
                {{ error.context.error_message || error.operation }}
              </div>
              <div class="mt-1 truncate">
                {{ error.context.provider_name || error.context.provider_id || '-' }} / {{ error.context.model || '-' }}
              </div>
            </div>
            <div class="text-xs text-muted-foreground lg:text-right">
              {{ formatShortDate(error.timestamp) }}
            </div>
          </div>
          <div
            v-if="recentErrors.length === 0"
            class="px-4 py-8 text-center text-sm text-muted-foreground"
          >
            当前没有近期错误
          </div>
        </div>
      </Card>

      <Card class="overflow-hidden">
        <div class="border-b border-border/70 bg-muted/20 px-4 py-3">
          <h2 class="text-sm font-semibold">
            运维入口
          </h2>
          <p class="text-xs text-muted-foreground">
            常用排障视角
          </p>
        </div>
        <div class="grid grid-cols-1 gap-2 p-4">
          <RouterLink
            v-for="link in opsLinks"
            :key="link.to"
            :to="link.to"
            class="flex items-center justify-between rounded-lg border border-border/60 bg-background/45 px-3 py-3 text-sm transition-colors hover:border-primary/50 hover:bg-primary/5"
          >
            <span class="font-medium">{{ link.label }}</span>
            <component
              :is="link.icon"
              class="h-4 w-4 text-muted-foreground"
            />
          </RouterLink>
        </div>
      </Card>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, defineComponent, h, onMounted, onUnmounted, ref, watch, type Component } from 'vue'
import { RouterLink } from 'vue-router'
import type { ChartData, ChartOptions } from 'chart.js'
import {
  Activity,
  AlertTriangle,
  BarChart3,
  CircleDollarSign,
  Database,
  Gauge,
  ListChecks,
  RefreshCw,
  ShieldCheck,
  Timer,
  Zap,
} from 'lucide-vue-next'
import { Badge, Button, Card, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui'
import { LoadingState, TimeRangePicker } from '@/components/common'
import LineChart from '@/components/charts/LineChart.vue'
import DoughnutChart from '@/components/charts/DoughnutChart.vue'
import { adminApi, type ErrorDistributionItem, type PercentileItem, type ProviderPerformanceResponse } from '@/api/admin'
import { cacheApi, redisCacheApi, type CacheStats, type RedisCacheCategoriesResponse } from '@/api/cache'
import { monitoringApi, type AdminMonitoringRecentError, type AdminMonitoringResilienceStatus, type GatewayMetricsSummary } from '@/api/monitoring'
import { proxyNodesApi, type ProxyNode, type ProxyNodeMetricsResponse } from '@/api/proxy-nodes'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import type { DateRangeParams } from '@/features/usage/types'
import { formatByteSize, formatCurrency, formatNumber, formatTokens } from '@/utils/format'
import { log } from '@/utils/logger'

interface MetricCellProps {
  label: string
  value: string
  valueClass?: string
}

const MetricCell = defineComponent<MetricCellProps>({
  name: 'MetricCell',
  props: {
    label: { type: String, required: true },
    value: { type: String, required: true },
    valueClass: { type: String, default: '' },
  },
  setup(props) {
    return () => h('div', { class: 'rounded-lg border border-border/60 bg-background/45 px-3 py-3' }, [
      h('div', { class: 'text-xs text-muted-foreground' }, props.label),
      h('div', { class: ['mt-1 truncate text-lg font-semibold tabular-nums', props.valueClass] }, props.value),
    ])
  },
})

const DEFAULT_SLOW_THRESHOLD_MS = 10_000

interface ResourceSnapshot {
  totalNodes: number
  onlineNodes: number
  avgCpuPercent: number | null
  avgMemoryPercent: number | null
  wsInBytes: number | null
  wsOutBytes: number | null
}

const timeRange = ref<DateRangeParams>({
  ...getDateRangeFromPeriod('today'),
  granularity: 'hour',
})
const timeSeries = ref<Array<Record<string, unknown>>>([])
const percentiles = ref<PercentileItem[]>([])
const providerPerformance = ref<ProviderPerformanceResponse | null>(null)
const errorDistribution = ref<ErrorDistributionItem[]>([])
const errorDistributionLoaded = ref(false)
const resilienceStatus = ref<AdminMonitoringResilienceStatus | null>(null)
const gatewayMetrics = ref<GatewayMetricsSummary | null>(null)
const cacheStats = ref<CacheStats | null>(null)
const redisCategories = ref<RedisCacheCategoriesResponse | null>(null)
const resourceSnapshot = ref<ResourceSnapshot | null>(null)
const lastUpdatedAt = ref<string | null>(null)
const analyticsWarning = ref<string | null>(null)
const realtimeWarning = ref<string | null>(null)
const loadWarning = computed(() =>
  [analyticsWarning.value, realtimeWarning.value].filter(Boolean).join('；') || null
)
const refreshing = ref(false)
const autoRefresh = ref(false)
const trendLoading = ref(false)
const percentileLoading = ref(false)
const AUTO_REFRESH_INTERVAL = 10_000
let requestId = 0
let refreshPromise: Promise<void> | null = null
let autoRefreshTimer: ReturnType<typeof setInterval> | null = null
let analyticsGeneration = 0

const timeRangeParams = computed(() => ({
  start_date: timeRange.value.start_date,
  end_date: timeRange.value.end_date,
  preset: timeRange.value.preset,
  timezone: timeRange.value.timezone,
  tz_offset_minutes: timeRange.value.tz_offset_minutes,
  granularity: timeRange.value.granularity || 'day',
}))

function numeric(value: unknown): number {
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : 0
  }
  return 0
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  return value as Record<string, unknown>
}

function numberField(record: Record<string, unknown> | null | undefined, key: string): number | null {
  if (!record) return null
  const value = record[key]
  if (typeof value === 'number' && Number.isFinite(value)) return value
  if (typeof value === 'string') {
    const parsed = Number(value)
    return Number.isFinite(parsed) ? parsed : null
  }
  return null
}

function averageFinite(values: Array<number | null | undefined>): number | null {
  const numbers = values.filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
  if (!numbers.length) return null
  return numbers.reduce((total, value) => total + value, 0) / numbers.length
}

function memoryUsedPercent(node: ProxyNode): number | null {
  const metadata = asRecord(node.proxy_metadata)
  const resource = asRecord(metadata?.resource_usage)
  const hardware = asRecord(node.hardware_info)
  const explicitPercent = numberField(resource, 'memory_used_percent')
  if (explicitPercent != null) return explicitPercent
  const usedBytes = numberField(resource, 'memory_used_bytes')
  const totalBytes = numberField(resource, 'memory_total_bytes')
    ?? numberField(hardware, 'memory_total_bytes')
    ?? numberField(hardware, 'total_memory_bytes')
  if (usedBytes == null || totalBytes == null || totalBytes <= 0) return null
  return usedBytes / totalBytes * 100
}

function cpuUsedPercent(node: ProxyNode): number | null {
  const metadata = asRecord(node.proxy_metadata)
  const resource = asRecord(metadata?.resource_usage)
  return numberField(resource, 'system_cpu_usage_percent')
    ?? numberField(resource, 'process_cpu_usage_percent')
}

async function loadResourceSnapshot(): Promise<ResourceSnapshot | null> {
  const now = Math.floor(Date.now() / 1000)
  const from = now - 3600
  const [nodesResult, fleetResult] = await Promise.allSettled([
    proxyNodesApi.listProxyNodes({ limit: 200 }),
    proxyNodesApi.listFleetMetrics({ from, to: now, step: '1m' }),
  ])

  const nodes = nodesResult.status === 'fulfilled' ? nodesResult.value.items : []
  const fleet: ProxyNodeMetricsResponse | null = fleetResult.status === 'fulfilled'
    ? fleetResult.value
    : null
  const onlineNodes = nodes.filter(node => node.status === 'online' || node.tunnel_connected)

  return {
    totalNodes: nodes.length,
    onlineNodes: onlineNodes.length,
    avgCpuPercent: averageFinite(onlineNodes.map(cpuUsedPercent)),
    avgMemoryPercent: averageFinite(onlineNodes.map(memoryUsedPercent)),
    wsInBytes: fleet?.summary.ws_in_bytes_delta ?? null,
    wsOutBytes: fleet?.summary.ws_out_bytes_delta ?? null,
  }
}

function formatMetricNumber(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  if (!Number.isInteger(value)) return value.toFixed(value < 10 ? 2 : 1)
  return formatNumber(value)
}

function formatPercent(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  const ratio = value <= 1 ? value * 100 : value
  return `${ratio.toFixed(2)}%`
}

function formatErrorRate(successRate: number | null | undefined): string {
  if (successRate == null || Number.isNaN(successRate)) return '-'
  const rate = successRate <= 1 ? successRate * 100 : successRate
  return `${Math.max(0, 100 - rate).toFixed(2)}%`
}

function formatMs(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  if (value >= 1000) return `${(value / 1000).toFixed(value >= 10_000 ? 1 : 2)}s`
  return `${Math.round(value)}ms`
}

function formatTps(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  return `${value.toFixed(value < 10 ? 2 : value < 100 ? 1 : 0)} tps`
}

function formatPercentResource(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  return `${value.toFixed(value < 10 ? 1 : 0)}%`
}

function basisPointsPercent(value: number | null | undefined): number | null {
  if (value == null || Number.isNaN(value)) return null
  return value / 100
}

function formatBasisPointsPercent(value: number | null | undefined): string {
  return formatPercentResource(basisPointsPercent(value))
}

function formatBytes(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  return formatByteSize(value)
}

function formatDurationSeconds(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  if (value < 60) return `${Math.round(value)}s`
  if (value < 3600) return `${Math.round(value / 60)}m`
  if (value < 86_400) return `${(value / 3600).toFixed(value < 36_000 ? 1 : 0)}h`
  return `${(value / 86_400).toFixed(1)}d`
}

function resourceToneClass(value: number | null | undefined, warning: number, critical: number): string {
  if (value == null || Number.isNaN(value)) return ''
  if (value >= critical) return 'text-red-600 dark:text-red-400'
  if (value >= warning) return 'text-amber-600 dark:text-amber-400'
  return 'text-green-600 dark:text-green-400'
}

function capacityPercentClass(value: number | null | undefined): string {
  return resourceToneClass(value, 80, 90)
}

function counterRiskClass(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return ''
  return value > 0 ? 'text-amber-600 dark:text-amber-400' : 'text-green-600 dark:text-green-400'
}

function latencyToneClass(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return ''
  if (value >= 5_000) return 'text-red-600 dark:text-red-400'
  if (value >= 1_000) return 'text-amber-600 dark:text-amber-400'
  return ''
}

function formatCapacityPercent(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return '-'
  return `${value.toFixed(value < 10 ? 1 : 0)}%`
}

function formatMetricRatio(current: number | null | undefined, max: number | null | undefined): string {
  return `${formatMetricNumber(current)} / ${formatMetricNumber(max)}`
}

function gateUtilization(gate: GatewayMetricsSummary['local'] | null | undefined): number | null {
  const inFlight = gate?.inFlight
  const available = gate?.availablePermits
  if (inFlight == null || available == null) return null
  const total = inFlight + available
  if (total <= 0) return null
  return Math.max(0, Math.min(100, inFlight / total * 100))
}

function hasCapacitySignal(...values: Array<number | null | undefined>): boolean {
  return values.some(value => typeof value === 'number' && Number.isFinite(value))
}

function formatShortDate(value?: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return '-'
  return date.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function successRateClass(value: number | null | undefined): string {
  if (value == null || Number.isNaN(value)) return ''
  const rate = value <= 1 ? value * 100 : value
  if (rate >= 95) return 'text-green-600 dark:text-green-400'
  if (rate >= 80) return 'text-amber-600 dark:text-amber-400'
  return 'text-red-600 dark:text-red-400'
}

function average(values: Array<number | null | undefined>): number | null {
  const numbers = values.filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
  if (!numbers.length) return null
  return numbers.reduce((total, value) => total + value, 0) / numbers.length
}

function sumSeries(field: string): number {
  return timeSeries.value.reduce((total, item) => total + numeric(item[field]), 0)
}

function seriesTokenTotal(item: Record<string, unknown>): number {
  return numeric(item.input_tokens)
    + numeric(item.output_tokens)
    + numeric(item.cache_creation_tokens)
    + numeric(item.cache_read_tokens)
}

async function performRefresh(refreshAnalytics: boolean) {
  const currentRequestId = ++requestId
  const currentAnalyticsGeneration = analyticsGeneration
  refreshing.value = true
  if (refreshAnalytics) {
    trendLoading.value = timeSeries.value.length === 0
    percentileLoading.value = percentiles.value.length === 0
    analyticsWarning.value = null
  }
  realtimeWarning.value = null

  const params = timeRangeParams.value
  const canCommitRealtime = () => currentRequestId === requestId
  const canCommitAnalytics = () => (
    refreshAnalytics
    && canCommitRealtime()
    && currentAnalyticsGeneration === analyticsGeneration
  )

  const timeSeriesRequest = (refreshAnalytics
    ? adminApi.getTimeSeries(params, { skipCache: true })
    : Promise.resolve(timeSeries.value)
  ).then((value) => {
    if (canCommitAnalytics()) timeSeries.value = value
    return value
  }).finally(() => {
    if (canCommitAnalytics()) trendLoading.value = false
  })

  const percentilesRequest = (refreshAnalytics
    ? adminApi.getPercentiles(params, { skipCache: true })
    : Promise.resolve(percentiles.value)
  ).then((value) => {
    if (canCommitAnalytics()) percentiles.value = value
    return value
  }).finally(() => {
    if (canCommitAnalytics()) percentileLoading.value = false
  })

  const providerPerformanceRequest = (refreshAnalytics
    ? adminApi.getProviderPerformance({
        ...params,
        granularity: params.granularity === 'hour' ? 'hour' : 'day',
        limit: 8,
        slow_threshold_ms: DEFAULT_SLOW_THRESHOLD_MS,
        include_timeline: false,
      }, { skipCache: true })
    : Promise.resolve(providerPerformance.value)
  ).then((value) => {
    if (canCommitAnalytics()) providerPerformance.value = value
    return value
  })

  const errorDistributionRequest = (refreshAnalytics
    ? adminApi.getErrorDistribution(params, { skipCache: true })
    : Promise.resolve({ distribution: errorDistribution.value })
  ).then((value) => {
    if (canCommitAnalytics()) {
      errorDistribution.value = value.distribution
      errorDistributionLoaded.value = true
    }
    return value
  })

  const resilienceRequest = monitoringApi.getResilienceStatus().then((value) => {
    if (canCommitRealtime()) resilienceStatus.value = value
    return value
  })
  const gatewayRequest = monitoringApi.getGatewayMetricsSummary().then((value) => {
    if (canCommitRealtime()) gatewayMetrics.value = value
    return value
  })
  const cacheRequest = cacheApi.getStats().then((value) => {
    if (canCommitRealtime()) cacheStats.value = value
    return value
  })
  const redisRequest = redisCacheApi.getCategories().then((value) => {
    if (canCommitRealtime()) redisCategories.value = value
    return value
  })
  const resourceRequest = loadResourceSnapshot().then((value) => {
    if (canCommitRealtime()) resourceSnapshot.value = value
    return value
  })

  const results = await Promise.allSettled([
    timeSeriesRequest,
    percentilesRequest,
    providerPerformanceRequest,
    errorDistributionRequest,
    resilienceRequest,
    gatewayRequest,
    cacheRequest,
    redisRequest,
    resourceRequest,
  ])

  if (currentRequestId !== requestId) return

  const analyticsFailed: string[] = []
  const realtimeFailed: string[] = []
  const [
    timeSeriesResult,
    percentilesResult,
    providerPerformanceResult,
    errorDistributionResult,
    resilienceResult,
    gatewayResult,
    cacheResult,
    redisResult,
  ] = results

  if (refreshAnalytics && currentAnalyticsGeneration === analyticsGeneration) {
    if (timeSeriesResult.status === 'rejected') analyticsFailed.push('流量趋势')
    if (percentilesResult.status === 'rejected') analyticsFailed.push('延迟百分位')
    if (providerPerformanceResult.status === 'rejected') analyticsFailed.push('上游性能')
    if (errorDistributionResult.status === 'rejected') analyticsFailed.push('错误分类')

    analyticsWarning.value = analyticsFailed.length
      ? `分析数据加载失败：${analyticsFailed.join('、')}`
      : null
  }

  if (resilienceResult.status === 'rejected') realtimeFailed.push('韧性状态')
  if (gatewayResult.status === 'rejected') realtimeFailed.push('网关指标')
  if (cacheResult.status === 'rejected') realtimeFailed.push('缓存统计')
  if (redisResult.status === 'rejected') realtimeFailed.push('Redis 分类')

  results.forEach((result, index) => {
    if (result.status === 'rejected') {
      log.error(`运维总览加载失败 ${index}`, result.reason)
    }
  })

  realtimeWarning.value = realtimeFailed.length
    ? `实时数据加载失败：${realtimeFailed.join('、')}`
    : null
  if (currentAnalyticsGeneration === analyticsGeneration) {
    lastUpdatedAt.value = new Date().toISOString()
  }
}

function refreshAll(): Promise<void> {
  if (refreshPromise) {
    return refreshPromise
  }

  const request = performRefresh(true).finally(() => {
    if (refreshPromise === request) {
      refreshPromise = null
    }
    refreshing.value = false
    trendLoading.value = false
    percentileLoading.value = false
  })
  refreshPromise = request
  return request
}

function stopAutoRefresh() {
  if (autoRefreshTimer) {
    clearInterval(autoRefreshTimer)
    autoRefreshTimer = null
  }
}

function toggleAutoRefresh() {
  autoRefresh.value = !autoRefresh.value
  if (!autoRefresh.value) {
    stopAutoRefresh()
    return
  }

  void refreshAll()
  autoRefreshTimer = setInterval(() => {
    void refreshAll()
  }, AUTO_REFRESH_INTERVAL)
}

const lastUpdatedLabel = computed(() => formatShortDate(lastUpdatedAt.value))

const totalRequests = computed(() => sumSeries('total_requests'))
const totalTokens = computed(() => timeSeries.value.reduce(
  (total, item) => total + seriesTokenTotal(item),
  0,
))
const totalCost = computed(() => sumSeries('total_cost'))
const classifiedErrorCount = computed(() => (
  errorDistributionLoaded.value
    ? errorDistribution.value.reduce((total, item) => total + numeric(item.count), 0)
    : null
))
const avgResponseMs = computed(() => providerPerformance.value?.summary.avg_response_time_ms ?? null)
const avgFirstByteMs = computed(() => providerPerformance.value?.summary.avg_first_byte_time_ms ?? average(percentiles.value.map(item => item.p50_first_byte_time_ms)))
const avgOutputTps = computed(() => providerPerformance.value?.summary.avg_output_tps ?? null)
const windowSeconds = computed(() => {
  const start = timeRange.value.start_date ? new Date(timeRange.value.start_date).getTime() : NaN
  const end = timeRange.value.end_date ? new Date(timeRange.value.end_date).getTime() : NaN
  if (Number.isFinite(start) && Number.isFinite(end) && end > start) {
    return (end - start) / 1000
  }
  switch (timeRange.value.preset) {
    case 'today': return Math.max(1, (Date.now() - new Date().setHours(0, 0, 0, 0)) / 1000)
    case 'yesterday': return 86_400
    case 'last7days': return 7 * 86_400
    case 'last30days': return 30 * 86_400
    case 'last90days': return 90 * 86_400
    default: return Math.max(1, timeSeries.value.length * 86_400)
  }
})
const qps = computed(() => totalRequests.value / windowSeconds.value)
const rpm = computed(() => qps.value * 60)
const tokensPerMinute = computed(() => totalTokens.value / Math.max(1, windowSeconds.value / 60))
const slaRate = computed(() => providerPerformance.value?.summary.success_rate ?? null)

const kpiCards = computed<Array<{
  title: string
  value: string
  hint: string
  icon: Component
  iconClass: string
  valueClass?: string
}>>(() => [
  {
    title: 'QPS',
    value: qps.value.toFixed(qps.value < 10 ? 2 : 1),
    hint: `RPM ${rpm.value.toFixed(rpm.value < 100 ? 1 : 0)} · 请求 ${formatMetricNumber(totalRequests.value)}`,
    icon: Activity,
    iconClass: 'text-sky-500',
  },
  {
    title: '趋势 Tokens',
    value: formatTokens(totalTokens.value),
    hint: `趋势聚合 · ${formatMetricNumber(tokensPerMinute.value)} TPM`,
    icon: Zap,
    iconClass: 'text-amber-500',
  },
  {
    title: 'SLA',
    value: formatPercent(slaRate.value),
    hint: `错误率 ${formatErrorRate(slaRate.value)}`,
    icon: ShieldCheck,
    iconClass: 'text-emerald-500',
    valueClass: successRateClass(slaRate.value),
  },
  {
    title: '请求时长',
    value: formatMs(avgResponseMs.value),
    hint: `P99 ${formatMs(providerPerformance.value?.summary.p99_response_time_ms)}`,
    icon: Gauge,
    iconClass: 'text-violet-500',
  },
  {
    title: 'TTFT',
    value: formatMs(avgFirstByteMs.value),
    hint: `P99 首字 ${formatMs(providerPerformance.value?.summary.p99_first_byte_time_ms)}`,
    icon: Timer,
    iconClass: 'text-blue-500',
  },
  {
    title: '输出 TPS',
    value: formatTps(avgOutputTps.value),
    hint: `慢请求 ${formatMetricNumber(providerPerformance.value?.summary.slow_request_count)}`,
    icon: BarChart3,
    iconClass: 'text-cyan-500',
  },
  {
    title: '上游错误',
    value: formatMetricNumber(resilienceStatus.value?.error_statistics.total_errors),
    hint: `打开熔断 ${formatMetricNumber(resilienceStatus.value?.error_statistics.open_circuit_breakers)}`,
    icon: AlertTriangle,
    iconClass: 'text-red-500',
  },
  {
    title: '费用',
    value: formatCurrency(totalCost.value),
    hint: `缓存读 ${formatTokens(cacheStats.value?.affinity_stats.cache_hits ?? 0)} 次`,
    icon: CircleDollarSign,
    iconClass: 'text-green-500',
  },
])

const trafficChartData = computed<ChartData<'line'>>(() => ({
  labels: timeSeries.value.map(item => String(item.date ?? item.bucket ?? item.time ?? '')),
  datasets: [
    {
      label: '请求',
      data: timeSeries.value.map(item => numeric(item.total_requests ?? item.requests)),
      borderColor: 'rgb(14, 165, 233)',
      backgroundColor: 'rgba(14, 165, 233, 0.12)',
      tension: 0.25,
      pointRadius: 2,
      yAxisID: 'y',
    },
    {
      label: 'Tokens',
      data: timeSeries.value.map(seriesTokenTotal),
      borderColor: 'rgb(245, 158, 11)',
      backgroundColor: 'rgba(245, 158, 11, 0.12)',
      tension: 0.25,
      pointRadius: 2,
      yAxisID: 'y1',
    },
  ],
}))

const trafficChartOptions: ChartOptions<'line'> = {
  interaction: { mode: 'index', intersect: false },
  scales: {
    y: { position: 'left' },
    y1: {
      position: 'right',
      grid: { drawOnChartArea: false },
    },
  },
}

const latencyChartData = computed<ChartData<'line'>>(() => ({
  labels: percentiles.value.map(item => item.date),
  datasets: [
    {
      label: 'P90 请求',
      data: percentiles.value.map(item => item.p90_response_time_ms ?? null),
      borderColor: 'rgb(124, 58, 237)',
      tension: 0.25,
      pointRadius: 2,
    },
    {
      label: 'P99 请求',
      data: percentiles.value.map(item => item.p99_response_time_ms ?? null),
      borderColor: 'rgb(239, 68, 68)',
      tension: 0.25,
      pointRadius: 2,
    },
    {
      label: 'P90 首字',
      data: percentiles.value.map(item => item.p90_first_byte_time_ms ?? null),
      borderColor: 'rgb(14, 165, 233)',
      tension: 0.25,
      pointRadius: 2,
    },
  ],
}))

const latencyChartOptions: ChartOptions<'line'> = {
  scales: {
    y: {
      ticks: {
        callback: value => formatMs(Number(value)),
      },
    },
  },
}

const errorDistributionChartData = computed<ChartData<'doughnut'>>(() => {
  const rows = errorDistribution.value.length
    ? errorDistribution.value
    : [{ category: '无错误', count: 1 }]
  return {
    labels: rows.map(item => item.category),
    datasets: [
      {
        data: rows.map(item => item.count),
        backgroundColor: [
          'rgba(239, 68, 68, 0.82)',
          'rgba(245, 158, 11, 0.82)',
          'rgba(14, 165, 233, 0.82)',
          'rgba(99, 102, 241, 0.82)',
          'rgba(34, 197, 94, 0.82)',
        ],
      },
    ],
  }
})

const errorDistributionOptions: ChartOptions<'doughnut'> = {
  plugins: {
    legend: {
      position: 'bottom',
    },
    tooltip: {
      callbacks: {
        label: context => `${context.label}: ${formatMetricNumber(context.raw as number)}`,
      },
    },
  },
}

const distributedGateVariant = computed<'warning' | 'outline'>(() => (
  gatewayMetrics.value?.distributed.unavailable ? 'warning' : 'outline'
))
const distributedGateText = computed(() => (
  gatewayMetrics.value?.distributed.unavailable ? '全局不可用' : '全局在线'
))
const currentActiveStreams = computed(() => gatewayMetrics.value?.tunnel.activeStreams ?? null)
const currentProxyConnections = computed(() => gatewayMetrics.value?.tunnel.proxyConnections ?? null)
const localGateUtilization = computed(() => gateUtilization(gatewayMetrics.value?.local))
const distributedGateUtilization = computed(() => gateUtilization(gatewayMetrics.value?.distributed))
const candidatePlanningGateUtilization = computed(() => gateUtilization(gatewayMetrics.value?.candidatePlanning))
const upstreamExecutionGateUtilization = computed(() => gateUtilization(gatewayMetrics.value?.upstreamExecution))
const localGateUtilizationText = computed(() => formatCapacityPercent(localGateUtilization.value))
const distributedGateUtilizationText = computed(() => formatCapacityPercent(distributedGateUtilization.value))
const candidatePlanningGateUtilizationText = computed(() => formatCapacityPercent(candidatePlanningGateUtilization.value))
const upstreamExecutionGateUtilizationText = computed(() => formatCapacityPercent(upstreamExecutionGateUtilization.value))
const admissionStatusVariant = computed<'success' | 'warning' | 'destructive' | 'outline'>(() => {
  if (!gatewayMetrics.value) return 'outline'
  if (gatewayMetrics.value.distributed.unavailable) return 'destructive'
  if (
    (localGateUtilization.value ?? 0) >= 90
    || (distributedGateUtilization.value ?? 0) >= 90
    || (candidatePlanningGateUtilization.value ?? 0) >= 90
    || (upstreamExecutionGateUtilization.value ?? 0) >= 90
  ) return 'warning'
  return 'success'
})
const admissionStatusText = computed(() => {
  if (!gatewayMetrics.value) return '未接入'
  if (gatewayMetrics.value.distributed.unavailable) return '分布式异常'
  if (admissionStatusVariant.value === 'warning') return '接近上限'
  return '正常'
})
const databasePoolUsagePercent = computed(() => {
  const basisPoints = gatewayMetrics.value?.databasePool.usageBasisPoints
  return basisPoints == null ? null : basisPoints / 100
})
const databasePoolUsageText = computed(() => formatCapacityPercent(databasePoolUsagePercent.value))
const databasePoolCheckedOutText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.databasePool.checkedOut,
  gatewayMetrics.value?.databasePool.max
))
const postgresWaitingText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.postgres.waitingConnections,
  gatewayMetrics.value?.postgres.activeConnections
))
const postgresOldestActiveQueryText = computed(() => formatMs(
  gatewayMetrics.value?.postgres.oldestActiveQueryAgeMs
))
const postgresOldestTransactionText = computed(() => formatMs(
  gatewayMetrics.value?.postgres.oldestTransactionAgeMs
))
const postgresCacheHitPercent = computed(() => (
  basisPointsPercent(gatewayMetrics.value?.postgres.blockCacheHitRateBasisPoints)
))
const postgresCacheHitText = computed(() => formatBasisPointsPercent(
  gatewayMetrics.value?.postgres.blockCacheHitRateBasisPoints
))
const postgresCacheHitClass = computed(() => {
  const value = postgresCacheHitPercent.value
  if (value == null || Number.isNaN(value)) return ''
  if (value < 90) return 'text-red-600 dark:text-red-400'
  if (value < 95) return 'text-amber-600 dark:text-amber-400'
  return 'text-green-600 dark:text-green-400'
})
function postgresOptionalText(
  available: boolean | null | undefined,
  unavailable: boolean | null | undefined,
  value: string,
): string {
  if (unavailable) return '异常'
  if (available === false) return '未接入'
  return value
}
const postgresWalText = computed(() => {
  const postgres = gatewayMetrics.value?.postgres
  return postgresOptionalText(
    postgres?.walAvailable,
    postgres?.walUnavailable,
    formatBytes(postgres?.walBytesTotal),
  )
})
const postgresWalClass = computed(() => (
  gatewayMetrics.value?.postgres.walUnavailable ? 'text-red-600 dark:text-red-400' : ''
))
const postgresCheckpointText = computed(() => {
  const postgres = gatewayMetrics.value?.postgres
  const value = `${formatMs(postgres?.checkpointWriteTimeMsTotal)} / ${formatMs(postgres?.checkpointSyncTimeMsTotal)}`
  return postgresOptionalText(postgres?.checkpointAvailable, postgres?.checkpointUnavailable, value)
})
const postgresCheckpointClass = computed(() => (
  gatewayMetrics.value?.postgres.checkpointUnavailable ? 'text-red-600 dark:text-red-400' : ''
))
const postgresTopStatementText = computed(() => {
  const postgres = gatewayMetrics.value?.postgres
  return postgresOptionalText(
    postgres?.statementAvailable,
    postgres?.statementUnavailable,
    formatMs(postgres?.statementTopMaxExecTimeMs),
  )
})
const postgresStatementClass = computed(() => {
  const postgres = gatewayMetrics.value?.postgres
  if (postgres?.statementUnavailable) return 'text-red-600 dark:text-red-400'
  return latencyToneClass(postgres?.statementTopMaxExecTimeMs)
})
const databasePoolStatusVariant = computed<'success' | 'warning' | 'destructive' | 'outline'>(() => {
  const pool = gatewayMetrics.value?.databasePool
  const postgres = gatewayMetrics.value?.postgres
  if (!pool || pool.max == null) return postgres?.unavailable ? 'destructive' : 'outline'
  if (
    pool.underMaintenancePressure
    || postgres?.unavailable
    || (postgres?.lockWaitingConnections ?? 0) > 0
    || (postgres?.idleInTransactionConnections ?? 0) > 0
    || (postgres?.oldestActiveQueryAgeMs ?? 0) >= 60_000
    || (postgres?.oldestTransactionAgeMs ?? 0) >= 60_000
    || (postgres?.walUnavailable ?? false)
    || (postgres?.checkpointUnavailable ?? false)
    || (postgres?.statementUnavailable ?? false)
    || (postgres?.statementTopMaxExecTimeMs ?? 0) >= 60_000
    || (databasePoolUsagePercent.value ?? 0) >= 90
  ) return 'destructive'
  if ((postgres?.waitingConnections ?? 0) > 0 || (databasePoolUsagePercent.value ?? 0) >= 80) return 'warning'
  return 'success'
})
const databasePoolStatusText = computed(() => {
  const pool = gatewayMetrics.value?.databasePool
  const postgres = gatewayMetrics.value?.postgres
  if (!pool || pool.max == null) return postgres?.unavailable ? 'PG 异常' : '未接入'
  if (postgres?.unavailable) return 'PG 异常'
  if ((postgres?.lockWaitingConnections ?? 0) > 0) return '锁等待'
  if ((postgres?.idleInTransactionConnections ?? 0) > 0) return '长事务'
  if (postgres?.walUnavailable) return 'WAL 异常'
  if (postgres?.checkpointUnavailable) return 'Checkpoint 异常'
  if (postgres?.statementUnavailable) return 'Top SQL 异常'
  if ((postgres?.statementTopMaxExecTimeMs ?? 0) >= 60_000) return '慢 SQL'
  if (pool.underMaintenancePressure) return '维护压力'
  if (databasePoolStatusVariant.value === 'destructive') return '高压'
  if (databasePoolStatusVariant.value === 'warning') return '偏高'
  return '正常'
})
const gatewayProcessCpuPercent = computed(() => basisPointsPercent(gatewayMetrics.value?.process.processCpuUsageBasisPoints))
const gatewaySystemCpuPercent = computed(() => basisPointsPercent(gatewayMetrics.value?.process.systemCpuUsageBasisPoints))
const gatewayProcessMemoryPercent = computed(() => basisPointsPercent(gatewayMetrics.value?.process.processMemoryBasisPoints))
const gatewaySystemMemoryPercent = computed(() => basisPointsPercent(gatewayMetrics.value?.process.systemMemoryUsageBasisPoints))
const gatewayProcessFdPercent = computed(() => basisPointsPercent(gatewayMetrics.value?.process.fdUsageBasisPoints))
const gatewayAllocatorActivePercent = computed(() => basisPointsPercent(gatewayMetrics.value?.allocator.activeToAllocatedBasisPoints))
const gatewayAllocatorActivePercentBasisPoints = computed(() => gatewayMetrics.value?.allocator.activeToAllocatedBasisPoints ?? null)
const gatewayProcessFdText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.process.openFds,
  gatewayMetrics.value?.process.fdLimit
))
const gatewayProcessSocketFdText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.process.socketFds,
  gatewayMetrics.value?.process.openFds
))
const gatewayProcessTcpText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.process.processTcpEstablishedConnections,
  gatewayMetrics.value?.process.processTcpConnections
))
const gatewayBackgroundTaskText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.backgroundTasks.active,
  gatewayMetrics.value?.backgroundTasks.supervisedTotal
))
const gatewayBackgroundTaskUnexpectedExits = computed(() => {
  const tasks = gatewayMetrics.value?.backgroundTasks
  if (!tasks) return null
  return tasks.unexpectedExitsTotal
})
const gatewayNetworkTrafficText = computed(() => {
  const process = gatewayMetrics.value?.process
  if (process?.networkReceivedBytesTotal == null && process?.networkTransmittedBytesTotal == null) return '-'
  return `RX ${formatBytes(process?.networkReceivedBytesTotal)} / TX ${formatBytes(process?.networkTransmittedBytesTotal)}`
})
const gatewayNetworkFaults = computed(() => {
  const process = gatewayMetrics.value?.process
  if (!process) return null
  return (process.networkReceiveErrorsTotal ?? 0)
    + (process.networkTransmitErrorsTotal ?? 0)
    + (process.networkReceiveDroppedTotal ?? 0)
    + (process.networkTransmitDroppedTotal ?? 0)
})
const gatewayTcpStateText = computed(() => {
  const process = gatewayMetrics.value?.process
  if (!process || process.tcpStateAvailable == null) return '-'
  if (!process.tcpStateAvailable) return '未采集'
  return `host ${formatMetricNumber(process.hostTcpEstablishedConnections)} established · ${formatMetricNumber(process.hostTcpTimeWaitConnections)} time-wait`
})
const gatewayProcessUptimeText = computed(() => formatDurationSeconds(gatewayMetrics.value?.process.processUptimeSeconds))
const gatewayAllocatorStatusText = computed(() => {
  const allocator = gatewayMetrics.value?.allocator
  if (!allocator || allocator.available == null) return '-'
  if (!allocator.available) return 'unavailable'
  return `allocated ${formatBytes(allocator.allocatedBytes)}`
})
const gatewayProcessStatusVariant = computed<'success' | 'warning' | 'destructive' | 'outline'>(() => {
  const process = gatewayMetrics.value?.process
  const backgroundTasks = gatewayMetrics.value?.backgroundTasks
  const tokioRuntime = gatewayMetrics.value?.tokioRuntime
  if (!process || !hasCapacitySignal(
    process.processCpuUsageBasisPoints,
    process.processMemoryBytes,
    process.fdUsageBasisPoints,
    process.socketFds,
    process.processTcpConnections,
    backgroundTasks?.active,
    backgroundTasks?.supervisedTotal,
    tokioRuntime?.aliveTasks,
    tokioRuntime?.globalQueueDepth,
  )) return 'outline'
  if (
    (process.processTcpCloseWaitConnections ?? 0) > 0
    || tokioRuntime?.available === false
    || (backgroundTasks?.unexpectedExitsTotal ?? 0) > 0
    || (backgroundTasks?.panickedTotal ?? 0) > 0
    || (backgroundTasks?.abortedTotal ?? 0) > 0
    || (gatewayProcessFdPercent.value ?? 0) >= 70
    || (gatewaySystemCpuPercent.value ?? 0) >= 90
    || (gatewayProcessMemoryPercent.value ?? 0) >= 90
  ) return 'destructive'
  if (
    (gatewayProcessFdPercent.value ?? 0) >= 60
    || (gatewaySystemCpuPercent.value ?? 0) >= 70
    || (gatewayProcessMemoryPercent.value ?? 0) >= 70
  ) return 'warning'
  return 'success'
})
const gatewayProcessStatusText = computed(() => {
  if (gatewayProcessStatusVariant.value === 'outline') return '未接入'
  if ((gatewayMetrics.value?.process.processTcpCloseWaitConnections ?? 0) > 0) return '连接泄漏'
  if (gatewayMetrics.value?.tokioRuntime.available === false) return 'Runtime 异常'
  if ((gatewayMetrics.value?.backgroundTasks.unexpectedExitsTotal ?? 0) > 0) return '任务退出'
  if (gatewayProcessStatusVariant.value === 'destructive') return '超阈值'
  if (gatewayProcessStatusVariant.value === 'warning') return '接近上限'
  return '正常'
})
const usageWorkerText = computed(() => {
  const usage = gatewayMetrics.value?.usageRuntime
  return formatMetricRatio(
    usage?.workerActiveCount ?? usage?.workerCount,
    usage?.workerMaxCount
  )
})
const usageRuntimeWorkerFaults = computed(() => {
  const usage = gatewayMetrics.value?.usageRuntime
  if (!usage) return null
  const values = [
    usage.workerDeadLetteredEntriesTotal,
    usage.workerProcessFailuresTotal,
    usage.workerReadFailuresTotal,
    usage.workerReclaimFailuresTotal,
  ]
  if (!values.some(value => value != null)) return null
  return values.reduce((total, value) => total + (value ?? 0), 0)
})
const usageQueueOldestPendingIdleText = computed(() => (
  formatDurationSeconds((gatewayMetrics.value?.usageQueue.oldestPendingIdleMs ?? null) == null
    ? null
    : (gatewayMetrics.value?.usageQueue.oldestPendingIdleMs ?? 0) / 1000)
))
const usageCounterOldestPendingAgeText = computed(() => (
  formatDurationSeconds(gatewayMetrics.value?.usageCounter.oldestPendingAgeSeconds)
))
const usageCounterOldestPendingAgeClass = computed(() => {
  const age = gatewayMetrics.value?.usageCounter.oldestPendingAgeSeconds
  if (age == null || Number.isNaN(age)) return ''
  if (age >= 60) return 'text-red-600 dark:text-red-400'
  if (age > 0) return 'text-amber-600 dark:text-amber-400'
  return 'text-green-600 dark:text-green-400'
})
const usageCounterHealthText = computed(() => {
  const counter = gatewayMetrics.value?.usageCounter
  if (!counter || counter.unavailable == null) return '-'
  if (counter.unavailable) return 'unavailable'
  if ((counter.oldestPendingAgeSeconds ?? 0) >= 60) return 'backlogged'
  if ((counter.pendingRows ?? 0) > 0) return 'catching-up'
  return 'ok'
})
const usageCounterOutboxFailures = computed(() => {
  const counter = gatewayMetrics.value?.usageCounter
  if (!counter) return null
  const values = [
    counter.flushFailedBatchesTotal,
    counter.cleanupFailedBatchesTotal,
  ]
  if (!values.some(value => value != null)) return null
  return values.reduce((total, value) => total + (value ?? 0), 0)
})
const candidateQueueUtilization = computed(() => {
  const depth = gatewayMetrics.value?.requestCandidateQueue.depth
  const capacity = gatewayMetrics.value?.requestCandidateQueue.capacity
  if (depth == null || capacity == null || capacity <= 0) return null
  return Math.max(0, Math.min(100, depth / capacity * 100))
})
const candidateQueueUtilizationText = computed(() => formatCapacityPercent(candidateQueueUtilization.value))
const candidateQueueUtilizationWidth = computed(() => (
  candidateQueueUtilization.value == null ? '0%' : `${candidateQueueUtilization.value}%`
))
const candidateQueueDepthText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.requestCandidateQueue.depth,
  gatewayMetrics.value?.requestCandidateQueue.capacity
))
const queueStatusVariant = computed<'success' | 'warning' | 'destructive' | 'outline'>(() => {
  const usage = gatewayMetrics.value?.usageRuntime
  const queue = gatewayMetrics.value?.requestCandidateQueue
  const usageQueue = gatewayMetrics.value?.usageQueue
  const counter = gatewayMetrics.value?.usageCounter
  const hasUsageQueueSignal = usageQueue?.unavailable != null || hasCapacitySignal(
    usageQueue?.groupPending,
    usageQueue?.groupLag,
    usageQueue?.dlqLength,
  )
  const hasUsageCounterSignal = counter?.unavailable != null || hasCapacitySignal(
    counter?.pendingRows,
    counter?.processedRows,
    counter?.oldestPendingAgeSeconds,
  )
  if (!gatewayMetrics.value || (!hasUsageQueueSignal && !hasUsageCounterSignal && !hasCapacitySignal(
    usage?.workerActiveCount,
    queue?.depth,
    queue?.capacity,
  ))) return 'outline'
  const hardFailures = (usage?.terminalEnqueueFailedTotal ?? 0)
    + (usage?.lifecycleEnqueueFailedTotal ?? 0)
    + (queue?.flushFailedTotal ?? 0)
    + (queue?.syncFallbackTotal ?? 0)
    + (usageRuntimeWorkerFaults.value ?? 0)
  if (
    hardFailures > 0
    || usageQueue?.unavailable
    || (usageQueue?.dlqLength ?? 0) > 0
    || (usageQueue?.oldestPendingIdleMs ?? 0) >= 60_000
    || counter?.unavailable
    || (usageCounterOutboxFailures.value ?? 0) > 0
    || (counter?.oldestPendingAgeSeconds ?? 0) >= 60
  ) return 'destructive'
  if (
    (candidateQueueUtilization.value ?? 0) >= 80
    || (usage?.lifecycleEnqueueDeferredDroppedTotal ?? 0) > 0
    || (usageQueue?.groupPending ?? 0) > 0
    || (usageQueue?.groupLag ?? 0) > 0
    || (counter?.pendingRows ?? 0) > 0
  ) return 'warning'
  return 'success'
})
const queueStatusText = computed(() => {
  if (queueStatusVariant.value === 'outline') return '未接入'
  if (queueStatusVariant.value === 'destructive') return '异常'
  if (queueStatusVariant.value === 'warning') return '积压'
  return '正常'
})
const upstreamTargetRows = computed(() => gatewayMetrics.value?.upstreamTargets.rows ?? [])
const upstreamTargetStatusVariant = computed<'success' | 'warning' | 'destructive' | 'outline'>(() => {
  const targets = gatewayMetrics.value?.upstreamTargets
  if (!targets || targets.activeTargets == null) return 'outline'
  if (targets.rejectedTotal > 0) return 'destructive'
  if (targets.saturatedTotal > 0) return 'warning'
  return 'success'
})
const upstreamTargetStatusText = computed(() => {
  if (upstreamTargetStatusVariant.value === 'outline') return '未接入'
  if (upstreamTargetStatusVariant.value === 'destructive') return '有拒绝'
  if (upstreamTargetStatusVariant.value === 'warning') return '有饱和'
  return '正常'
})
const stageLatencyRows = computed(() => gatewayMetrics.value?.stageLatency.rows ?? [])
const tunnelQueueRejectedTotal = computed(() => (
  (gatewayMetrics.value?.tunnel.outboundQueueRejectedFullTotal ?? 0)
  + (gatewayMetrics.value?.tunnel.outboundQueueRejectedClosedTotal ?? 0)
))
const tunnelSelectionPressureTotal = computed(() => (
  (gatewayMetrics.value?.tunnel.proxyConnectionCongestedTotal ?? 0)
  + (gatewayMetrics.value?.tunnel.selectionRetryTotal ?? 0)
  + (gatewayMetrics.value?.tunnel.selectionUnavailableTotal ?? 0)
))
const tunnelQueueUtilization = computed(() => {
  const depth = gatewayMetrics.value?.tunnel.outboundQueueDepthTotal
  const capacity = gatewayMetrics.value?.tunnel.outboundQueueCapacityTotal
  if (depth == null || capacity == null || capacity <= 0) return null
  return Math.max(0, Math.min(100, depth / capacity * 100))
})
const tunnelQueueUtilizationText = computed(() => (
  tunnelQueueUtilization.value == null ? '-' : `${Math.round(tunnelQueueUtilization.value)}%`
))
const tunnelQueueUtilizationWidth = computed(() => (
  tunnelQueueUtilization.value == null ? '0%' : `${tunnelQueueUtilization.value}%`
))

const slaValueClass = computed(() => successRateClass(providerPerformance.value?.summary.success_rate))
const errorRateValueClass = computed(() => {
  const rate = providerPerformance.value?.summary.success_rate
  if (rate == null) return ''
  const successPercent = rate <= 1 ? rate * 100 : rate
  const errorPercent = Math.max(0, 100 - successPercent)
  if (errorPercent <= 5) return 'text-green-600 dark:text-green-400'
  if (errorPercent <= 20) return 'text-amber-600 dark:text-amber-400'
  return 'text-red-600 dark:text-red-400'
})

const providerRows = computed(() => providerPerformance.value?.providers.slice(0, 8) ?? [])
const cacheHitRate = computed(() => cacheStats.value?.affinity_stats.cache_hit_rate ?? null)
const redisStatusText = computed(() => {
  const runtime = gatewayMetrics.value?.redisRuntime
  if (runtime?.unavailable) return '异常'
  if (runtime?.enabled) return '在线'
  if (!redisCategories.value) return '-'
  return redisCategories.value.available ? '在线' : '未启用'
})
const redisStatusClass = computed(() => {
  const runtime = gatewayMetrics.value?.redisRuntime
  if (runtime?.unavailable) return 'text-red-600 dark:text-red-400'
  if (
    (redisRuntimeCommandFaults.value ?? 0) > 0
    || (redisRuntimeMemoryPercent.value ?? 0) >= 90
    || (gatewayMetrics.value?.redisRuntime.nonblockingCommandLatencyMaxMs ?? 0) >= 500
  ) {
    return 'text-red-600 dark:text-red-400'
  }
  if (
    (redisRuntimeMemoryPercent.value ?? 0) >= 80
    || (gatewayMetrics.value?.redisRuntime.nonblockingCommandLatencyMaxMs ?? 0) >= 100
  ) return 'text-amber-600 dark:text-amber-400'
  if (runtime?.enabled) return 'text-green-600 dark:text-green-400'
  if (!redisCategories.value) return ''
  return redisCategories.value.available ? 'text-green-600 dark:text-green-400' : 'text-amber-600 dark:text-amber-400'
})
const redisRuntimeMemoryPercent = computed(() => basisPointsPercent(gatewayMetrics.value?.redisRuntime.memoryUsageBasisPoints))
const redisRuntimeMemoryText = computed(() => {
  const runtime = gatewayMetrics.value?.redisRuntime
  if (runtime?.usedMemoryBytes == null) return '-'
  if ((runtime.maxmemoryBytes ?? 0) > 0) {
    return `${formatBytes(runtime.usedMemoryBytes)} / ${formatBytes(runtime.maxmemoryBytes)}`
  }
  return formatBytes(runtime.usedMemoryBytes)
})
const redisRuntimeOpsText = computed(() => {
  const ops = gatewayMetrics.value?.redisRuntime.instantaneousOpsPerSec
  if (ops == null || Number.isNaN(ops)) return '-'
  return `${formatMetricNumber(ops)}/s`
})
const redisRuntimeLatencyText = computed(() => formatMs(
  gatewayMetrics.value?.redisRuntime.nonblockingCommandLatencyMaxMs
))
const redisRuntimeClientsText = computed(() => formatMetricRatio(
  gatewayMetrics.value?.redisRuntime.connectedClients,
  gatewayMetrics.value?.redisRuntime.blockedClients
))
const redisRuntimeCommandFaults = computed(() => {
  const runtime = gatewayMetrics.value?.redisRuntime
  if (!runtime) return null
  return (runtime.laneCommandErrorsTotal ?? 0)
    + (runtime.laneCommandTimeoutsTotal ?? 0)
    + (runtime.totalErrorReplies ?? 0)
    + (runtime.rejectedConnectionsTotal ?? 0)
    + (runtime.evictedKeysTotal ?? 0)
})
const redisCategoryRows = computed(() => (
  redisCategories.value?.categories
    .slice()
    .sort((left, right) => right.count - left.count)
    .slice(0, 6) ?? []
))
const recentErrors = computed<AdminMonitoringRecentError[]>(() => resilienceStatus.value?.recent_errors.slice(0, 6) ?? [])

const opsLinks = [
  { label: '性能分析', to: '/admin/performance-analysis', icon: Gauge },
  { label: '健康监控', to: '/admin/health-monitor', icon: ShieldCheck },
  { label: '使用记录', to: '/admin/usage', icon: ListChecks },
  { label: '缓存监控', to: '/admin/cache-monitoring', icon: Database },
  { label: '审计日志', to: '/admin/audit-logs', icon: AlertTriangle },
  { label: '异步任务', to: '/admin/async-tasks', icon: RefreshCw },
]

watch(timeRange, () => {
  analyticsGeneration += 1
  timeSeries.value = []
  percentiles.value = []
  providerPerformance.value = null
  errorDistribution.value = []
  errorDistributionLoaded.value = false
  analyticsWarning.value = null
  lastUpdatedAt.value = null
  trendLoading.value = false
  percentileLoading.value = false
  if (autoRefresh.value) {
    void refreshAll()
  }
}, { deep: true })

onMounted(() => {
  void refreshAll()
})

onUnmounted(() => {
  stopAutoRefresh()
  requestId += 1
  analyticsGeneration += 1
})
</script>
