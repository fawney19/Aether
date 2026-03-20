<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <div class="border-b border-border/60 bg-muted/20 px-6 py-5">
      <div class="flex flex-col gap-4">
        <div class="flex flex-col gap-3 xl:flex-row xl:items-start xl:justify-between">
          <div class="space-y-1">
            <div class="flex items-center gap-2 flex-wrap">
              <h3 class="text-base font-semibold">
                {{ title }}
              </h3>
              <Badge
                variant="outline"
                class="text-[10px] uppercase tracking-[0.25em]"
              >
                {{ isAdmin ? 'Admin' : 'Public' }}
              </Badge>
            </div>
            <p class="text-xs text-muted-foreground">
              异常优先排序，支持状态筛选、详情诊断与自动刷新，首屏先看出问题的格式。
            </p>
          </div>

          <div class="flex flex-wrap items-center gap-3">
            <div class="text-xs text-muted-foreground">
              最后刷新 {{ generatedAt ? formatRelativeTime(generatedAt) : '未加载' }}
            </div>
            <div class="flex items-center gap-2 rounded-xl border border-border/60 bg-card/70 px-3 py-1.5">
              <Switch v-model="autoRefreshEnabled" />
              <span class="text-xs font-medium">
                {{ autoRefreshEnabled ? '自动刷新 15s' : '自动刷新已关闭' }}
              </span>
            </div>
            <Button
              v-if="isAdmin"
              variant="outline"
              size="sm"
              class="h-9 px-3 text-xs"
              :disabled="recoveringAllKeys || (healthSummary?.keys.circuit_open ?? 0) <= 0"
              @click="recoverAllConfirmOpen = true"
            >
              <Loader2
                v-if="recoveringAllKeys"
                class="mr-2 h-3.5 w-3.5 animate-spin"
              />
              恢复全部熔断 Key
            </Button>
            <RefreshButton
              :loading="loadingMonitors"
              @click="refreshData({ silent: false })"
            />
          </div>
        </div>

        <div class="grid grid-cols-2 gap-3 xl:grid-cols-6">
          <Card
            v-for="item in summaryCards"
            :key="item.label"
            variant="inset"
            class="p-3"
          >
            <div class="flex items-start justify-between gap-3">
              <div class="min-w-0">
                <div class="text-[11px] font-semibold uppercase tracking-[0.18em] text-muted-foreground">
                  {{ item.label }}
                </div>
                <div
                  class="mt-2 text-xl font-semibold"
                  :class="item.valueClass"
                >
                  {{ item.value }}
                </div>
                <div class="mt-1 text-[11px] text-muted-foreground">
                  {{ item.help }}
                </div>
              </div>
              <div
                class="rounded-xl border border-border/60 p-2"
                :class="item.iconClass"
              >
                <component
                  :is="item.icon"
                  class="h-4 w-4"
                />
              </div>
            </div>
          </Card>
        </div>

        <div class="flex flex-col gap-3 xl:flex-row xl:items-center xl:justify-between">
          <div class="grid flex-1 grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <div>
              <Label class="mb-1.5 block text-xs text-muted-foreground">回溯时间</Label>
              <Select v-model="lookbackHours">
                <SelectTrigger class="h-9 border-border/60 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="1">
                    1 小时
                  </SelectItem>
                  <SelectItem value="6">
                    6 小时
                  </SelectItem>
                  <SelectItem value="12">
                    12 小时
                  </SelectItem>
                  <SelectItem value="24">
                    24 小时
                  </SelectItem>
                  <SelectItem value="48">
                    48 小时
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div>
              <Label class="mb-1.5 block text-xs text-muted-foreground">状态筛选</Label>
              <Select v-model="statusFilter">
                <SelectTrigger class="h-9 border-border/60 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    全部状态
                  </SelectItem>
                  <SelectItem value="unhealthy">
                    仅看异常
                  </SelectItem>
                  <SelectItem value="warning">
                    仅看预警
                  </SelectItem>
                  <SelectItem value="healthy">
                    仅看健康
                  </SelectItem>
                  <SelectItem value="unknown">
                    仅看无流量
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div>
              <Label class="mb-1.5 block text-xs text-muted-foreground">格式族</Label>
              <Select v-model="familyFilter">
                <SelectTrigger class="h-9 border-border/60 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    全部格式
                  </SelectItem>
                  <SelectItem value="claude">
                    Claude
                  </SelectItem>
                  <SelectItem value="openai">
                    OpenAI
                  </SelectItem>
                  <SelectItem value="gemini">
                    Gemini
                  </SelectItem>
                  <SelectItem value="other">
                    其他
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <div>
              <Label class="mb-1.5 block text-xs text-muted-foreground">显示范围</Label>
              <div class="flex h-9 items-center justify-between rounded-xl border border-border/60 bg-card/70 px-3">
                <span class="text-xs font-medium">仅看有流量</span>
                <Switch v-model="onlyTraffic" />
              </div>
            </div>
          </div>

          <div class="flex items-center gap-2 text-xs text-muted-foreground">
            <span>显示 {{ filteredMonitors.length }} / {{ monitors.length }} 个格式</span>
            <Button
              v-if="hasActiveFilters"
              variant="ghost"
              size="sm"
              class="h-8 px-2 text-xs"
              @click="resetFilters"
            >
              重置筛选
            </Button>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-2 text-xs">
          <Badge
            variant="success"
            class="gap-1"
          >
            健康
          </Badge>
          <span class="text-muted-foreground">成功率 ≥ 95%</span>
          <Badge
            variant="warning"
            class="gap-1"
          >
            预警
          </Badge>
          <span class="text-muted-foreground">70% - 95%</span>
          <Badge
            variant="destructive"
            class="gap-1"
          >
            异常
          </Badge>
          <span class="text-muted-foreground">成功率 &lt; 70%</span>
          <Badge
            variant="outline"
            class="gap-1"
          >
            无流量
          </Badge>
          <span class="text-muted-foreground">窗口内没有请求</span>
        </div>
      </div>
    </div>

    <div class="p-6">
      <div
        v-if="showInitialLoading"
        class="flex items-center justify-center py-12"
      >
        <Loader2 class="w-6 h-6 animate-spin text-muted-foreground" />
        <span class="ml-2 text-muted-foreground">加载中...</span>
      </div>

      <div
        v-else-if="monitors.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Activity class="mb-3 h-12 w-12 opacity-30" />
        <p>暂无健康监控数据</p>
        <p class="mt-1 text-xs">
          端点尚未产生请求记录
        </p>
      </div>

      <div
        v-else-if="filteredMonitors.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <AlertTriangle class="mb-3 h-12 w-12 opacity-30" />
        <p>当前筛选条件下没有匹配项</p>
        <p class="mt-1 text-xs">
          可以放宽状态、格式族或流量条件
        </p>
        <Button
          variant="outline"
          size="sm"
          class="mt-4"
          @click="resetFilters"
        >
          重置筛选
        </Button>
      </div>

      <div
        v-else
        class="space-y-3"
      >
        <div
          v-for="monitor in filteredMonitors"
          :key="monitor.api_format"
          class="rounded-xl border p-4 transition-colors"
          :class="getMonitorCardClass(monitor)"
        >
          <div class="flex flex-col gap-3 xl:flex-row xl:items-center xl:gap-6">
            <div class="space-y-1.5 xl:w-64 xl:flex-shrink-0">
              <div class="flex items-center gap-2 flex-wrap">
                <Badge
                  variant="outline"
                  class="whitespace-nowrap font-mono text-xs"
                >
                  {{ formatApiFormat(monitor.api_format) }}
                </Badge>
                <Badge
                  :variant="getMonitorStatusVariant(monitor)"
                  class="text-xs whitespace-nowrap"
                >
                  {{ getMonitorStatusLabel(monitor) }}
                </Badge>
                <Badge
                  v-if="monitor.total_attempts > 0"
                  :variant="getSuccessRateVariant(monitor.success_rate)"
                  class="text-xs whitespace-nowrap"
                >
                  {{ formatPercent(monitor.success_rate) }}
                </Badge>
                <span
                  v-if="showProviderInfo && 'provider_count' in monitor"
                  class="text-xs text-muted-foreground xl:hidden"
                >
                  {{ monitor.provider_count }} 个提供商 / {{ monitor.key_count }} 个密钥
                </span>
              </div>

              <div
                v-if="showProviderInfo && 'provider_count' in monitor"
                class="hidden text-xs text-muted-foreground xl:block"
              >
                {{ monitor.provider_count }} 个提供商 / {{ monitor.key_count }} 个密钥
              </div>

              <div class="text-xs text-muted-foreground">
                {{ monitor.total_attempts }} 次请求
                <span class="mx-1">·</span>
                失败 {{ monitor.failed_count }}
                <span class="mx-1">·</span>
                跳过 {{ monitor.skipped_count }}
              </div>

              <div class="text-xs text-muted-foreground">
                最近事件 {{ monitor.last_event_at ? formatRelativeTime(monitor.last_event_at) : '暂无' }}
              </div>

              <div class="pt-1">
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 px-3 text-xs"
                  @click="openMonitorDetail(monitor)"
                >
                  查看详情
                </Button>
              </div>
            </div>

            <div class="min-w-0 flex-1">
              <EndpointHealthTimeline
                :monitor="monitor"
                :lookback-hours="parseInt(lookbackHours)"
              />
            </div>
          </div>
        </div>
      </div>
    </div>
  </Card>

  <Dialog
    v-model="detailDialogOpen"
    size="5xl"
    close-on-backdrop
    :title="selectedMonitor ? `${formatApiFormat(selectedMonitor.api_format)} 详情` : '监控详情'"
    :description="selectedMonitor ? detailDialogDescription : '查看最近失败、错误分布与事件样本'"
  >
    <template #header-actions>
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        aria-label="关闭详情"
        @click="detailDialogOpen = false"
      >
        <X class="h-4 w-4" />
      </Button>
    </template>

    <div
      v-if="selectedMonitor"
      class="space-y-4"
    >
      <div class="grid grid-cols-2 gap-3 xl:grid-cols-5">
        <Card
          variant="inset"
          class="p-3"
        >
          <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            当前状态
          </div>
          <div class="mt-2">
            <Badge :variant="getMonitorStatusVariant(selectedMonitor)">
              {{ getMonitorStatusLabel(selectedMonitor) }}
            </Badge>
          </div>
        </Card>

        <Card
          variant="inset"
          class="p-3"
        >
          <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            成功率
          </div>
          <div class="mt-2 text-xl font-semibold">
            {{ formatPercent(selectedMonitor.success_rate) }}
          </div>
        </Card>

        <Card
          variant="inset"
          class="p-3"
        >
          <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            请求总数
          </div>
          <div class="mt-2 text-xl font-semibold">
            {{ selectedMonitor.total_attempts }}
          </div>
        </Card>

        <Card
          variant="inset"
          class="p-3"
        >
          <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            最近失败
          </div>
          <div class="mt-2 text-xl font-semibold">
            {{ selectedFailureEvents.length }}
          </div>
        </Card>

        <Card
          variant="inset"
          class="p-3"
        >
          <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            最近事件
          </div>
          <div class="mt-2 text-sm font-medium">
            {{ selectedMonitor.last_event_at ? formatAbsoluteTime(selectedMonitor.last_event_at) : '暂无' }}
          </div>
        </Card>
      </div>

      <div
        v-if="selectedMonitorApiPath"
        class="rounded-xl border border-border/60 bg-muted/20 px-4 py-3 text-sm"
      >
        <div class="text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
          请求入口
        </div>
        <div class="mt-2 font-mono text-foreground">
          {{ selectedMonitorApiPath }}
        </div>
      </div>

      <div
        v-if="isAdmin"
        class="grid grid-cols-1 gap-4 xl:grid-cols-[minmax(0,0.85fr)_minmax(0,1.15fr)]"
      >
        <Card
          variant="inset"
          class="p-4"
        >
          <div class="mb-3 flex items-center justify-between">
            <h4 class="text-sm font-semibold">
              受影响 Provider
            </h4>
            <span class="text-xs text-muted-foreground">
              {{ selectedProviderSummaries.length }} 个
            </span>
          </div>

          <div
            v-if="loadingFormatKeys"
            class="flex items-center gap-2 py-6 text-sm text-muted-foreground"
          >
            <Loader2 class="h-4 w-4 animate-spin" />
            正在加载 Key 诊断数据...
          </div>

          <div
            v-else-if="selectedProviderSummaries.length === 0"
            class="py-6 text-sm text-muted-foreground"
          >
            当前格式下没有可诊断的 provider/key 数据。
          </div>

          <div
            v-else
            class="max-h-[420px] space-y-2 overflow-y-auto pr-1"
          >
            <div
              v-for="provider in selectedProviderSummaries"
              :key="provider.provider_id"
              class="rounded-xl border p-3 transition-colors"
              :class="detailProviderFilter === provider.provider_id
                ? 'border-primary/60 bg-primary/5'
                : 'border-border/50 bg-card/70'"
            >
              <div class="flex items-center justify-between gap-3">
                <div class="min-w-0">
                  <div class="flex items-center gap-2 flex-wrap">
                    <span class="text-sm font-medium">
                      {{ provider.provider_name }}
                    </span>
                    <Badge :variant="provider.circuitOpenKeys > 0 ? 'destructive' : provider.providerActive ? 'success' : 'outline'">
                      {{ provider.circuitOpenKeys > 0 ? `${provider.circuitOpenKeys} 熔断` : provider.providerActive ? '活跃' : '停用' }}
                    </Badge>
                  </div>
                  <div class="mt-1 text-xs text-muted-foreground">
                    {{ provider.totalKeys }} 个 Key
                    <span class="mx-1">·</span>
                    活跃 {{ provider.activeKeys }}
                    <span class="mx-1">·</span>
                    平均健康 {{ formatScore(provider.avgHealthScore) }}
                  </div>
                </div>
                <div class="text-right text-xs text-muted-foreground">
                  <div>{{ provider.requestCount }} 请求</div>
                </div>
              </div>

              <div class="mt-3 flex flex-wrap gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  :class="DETAIL_FILTER_BUTTON_CLASS"
                  @click="toggleDetailProviderFilter(provider.provider_id)"
                >
                  {{ detailProviderFilter === provider.provider_id ? '取消筛选' : '只看该 Provider' }}
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  :class="DETAIL_FILTER_BUTTON_CLASS"
                  @click="goToProviderManagement(provider.provider_id)"
                >
                  前往 Provider
                </Button>
                <Button
                  v-if="provider.poolEnabled"
                  variant="outline"
                  size="sm"
                  :class="DETAIL_FILTER_BUTTON_CLASS"
                  @click="goToPoolManagement(provider.provider_id)"
                >
                  前往号池
                </Button>
              </div>
            </div>
          </div>
        </Card>

        <Card
          variant="inset"
          class="p-4"
        >
          <div class="mb-3 flex flex-col gap-3">
            <div class="flex items-center justify-between gap-3">
              <h4 class="text-sm font-semibold">
                Key 诊断
              </h4>
              <span class="text-xs text-muted-foreground">
                {{ filteredSelectedFormatKeys.length }} / {{ selectedFormatKeys.length }} 个 Key
              </span>
            </div>

            <div class="flex flex-wrap gap-2">
              <Button
                variant="outline"
                size="sm"
                :class="[DETAIL_FILTER_BUTTON_CLASS, detailKeyScopeFilter === 'all' ? 'border-primary/60 bg-primary/5 text-primary' : '']"
                @click="detailKeyScopeFilter = 'all'"
              >
                全部
              </Button>
              <Button
                variant="outline"
                size="sm"
                :class="[DETAIL_FILTER_BUTTON_CLASS, detailKeyScopeFilter === 'circuit-open' ? 'border-primary/60 bg-primary/5 text-primary' : '']"
                @click="detailKeyScopeFilter = 'circuit-open'"
              >
                仅看熔断
              </Button>
              <Button
                variant="outline"
                size="sm"
                :class="[DETAIL_FILTER_BUTTON_CLASS, detailKeyScopeFilter === 'recoverable' ? 'border-primary/60 bg-primary/5 text-primary' : '']"
                @click="detailKeyScopeFilter = 'recoverable'"
              >
                仅看可恢复项
              </Button>
              <Button
                v-if="detailProviderFilter !== 'all' || detailKeyScopeFilter !== 'all'"
                variant="outline"
                size="sm"
                :class="DETAIL_FILTER_BUTTON_CLASS"
                @click="resetDetailFilters"
              >
                清空局部筛选
              </Button>
            </div>

            <div
              v-if="detailProviderFilter !== 'all' || detailKeyScopeFilter !== 'all'"
              class="text-xs text-muted-foreground"
            >
              <span v-if="detailProviderFilter !== 'all'">
                Provider：{{ detailProviderFilterName }}
              </span>
              <span v-if="detailProviderFilter !== 'all' && detailKeyScopeFilter !== 'all'"> · </span>
              <span v-if="detailKeyScopeFilter === 'circuit-open'">范围：仅熔断项</span>
              <span v-else-if="detailKeyScopeFilter === 'recoverable'">范围：仅可恢复项</span>
            </div>
          </div>

          <div
            v-if="loadingFormatKeys"
            class="flex items-center gap-2 py-6 text-sm text-muted-foreground"
          >
            <Loader2 class="h-4 w-4 animate-spin" />
            正在加载 Key 诊断数据...
          </div>

          <div
            v-else-if="selectedFormatKeys.length === 0"
            class="py-6 text-sm text-muted-foreground"
          >
            当前格式下没有 Key 样本。
          </div>

          <div
            v-else-if="filteredSelectedFormatKeys.length === 0"
            class="py-6 text-sm text-muted-foreground"
          >
            当前局部筛选下没有匹配的 Key。
          </div>

          <div
            v-else
            class="max-h-[420px] space-y-2 overflow-y-auto pr-1"
          >
            <div
              v-for="key in filteredSelectedFormatKeys"
              :key="key.id"
              class="rounded-xl border border-border/50 bg-card/70 p-3"
            >
              <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
                <div class="min-w-0">
                  <div class="flex items-center gap-2 flex-wrap">
                    <span class="text-sm font-medium">
                      {{ key.name }}
                    </span>
                    <Badge variant="outline">
                      {{ key.provider_name }}
                    </Badge>
                    <Badge :variant="key.circuit_breaker_open ? 'destructive' : key.is_active && key.provider_active ? 'success' : 'outline'">
                      {{ key.circuit_breaker_open ? '已熔断' : key.is_active && key.provider_active ? '可用' : '停用' }}
                    </Badge>
                    <Badge
                      v-if="shouldShowRecoverHealthAction(key)"
                      variant="warning"
                    >
                      建议恢复
                    </Badge>
                  </div>
                  <div class="mt-1 font-mono text-xs text-muted-foreground">
                    {{ key.api_key_masked }}
                  </div>
                  <div class="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                    <span>健康 {{ formatScore(key.health_score) }}</span>
                    <span v-if="key.success_rate != null">{{ formatPercent(key.success_rate) }}</span>
                    <span>{{ key.request_count }} 请求</span>
                    <span v-if="key.avg_response_time_ms != null">均延迟 {{ formatLatency(key.avg_response_time_ms) }}</span>
                  </div>
                </div>

                <div class="flex flex-wrap gap-2 sm:justify-end">
                  <Button
                    v-if="key.pool_enabled"
                    variant="outline"
                    size="sm"
                    :class="DETAIL_ACTION_BUTTON_CLASS"
                    @click="goToPoolManagement(key.provider_id, key.name)"
                  >
                    号池内查看
                  </Button>
                  <Button
                    v-if="shouldShowRecoverHealthAction(key)"
                    variant="outline"
                    size="sm"
                    :class="DETAIL_ACTION_BUTTON_CLASS"
                    :disabled="recoveringKeyId !== null"
                    @click="recoverFormatKey(key)"
                  >
                    <Loader2
                      v-if="recoveringKeyId === key.id"
                      class="mr-2 h-3.5 w-3.5 animate-spin"
                    />
                    恢复健康度
                  </Button>
                </div>
              </div>
            </div>
          </div>
        </Card>
      </div>

      <div class="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <Card
          variant="inset"
          class="p-4"
        >
          <div class="mb-3 flex items-center justify-between">
            <h4 class="text-sm font-semibold">
              最近失败
            </h4>
            <span class="text-xs text-muted-foreground">
              最近 {{ Math.min(selectedFailureEvents.length, 8) }} 条
            </span>
          </div>

          <div
            v-if="selectedFailureEvents.length === 0"
            class="py-6 text-sm text-muted-foreground"
          >
            当前窗口内没有失败事件。
          </div>

          <div
            v-else
            class="space-y-2"
          >
            <div
              v-for="event in selectedFailureEvents.slice(0, 8)"
              :key="buildEventKey(event)"
              class="rounded-xl border border-border/50 bg-card/70 p-3"
            >
              <div class="flex items-center justify-between gap-3">
                <div class="flex items-center gap-2">
                  <Badge variant="destructive">
                    失败
                  </Badge>
                  <span
                    v-if="event.status_code"
                    class="text-xs font-medium text-foreground"
                  >
                    HTTP {{ event.status_code }}
                  </span>
                </div>
                <span class="text-xs text-muted-foreground">
                  {{ formatAbsoluteTime(event.timestamp) }}
                </span>
              </div>
              <div class="mt-2 text-sm text-foreground">
                {{ event.error_type || event.error_message || '未提供错误类型' }}
              </div>
              <div class="mt-1 text-xs text-muted-foreground">
                {{ event.latency_ms != null ? `延迟 ${formatLatency(event.latency_ms)}` : '延迟未知' }}
              </div>
            </div>
          </div>
        </Card>

        <Card
          variant="inset"
          class="p-4"
        >
          <div class="mb-3">
            <h4 class="text-sm font-semibold">
              错误分布
            </h4>
          </div>

          <div class="space-y-4">
            <div>
              <div class="mb-2 text-xs uppercase tracking-[0.18em] text-muted-foreground">
                Error Type
              </div>
              <div
                v-if="selectedErrorTypes.length === 0"
                class="text-sm text-muted-foreground"
              >
                当前没有可归类的错误类型。
              </div>
              <div
                v-else
                class="flex flex-wrap gap-2"
              >
                <Badge
                  v-for="item in selectedErrorTypes"
                  :key="item.label"
                  variant="outline"
                >
                  {{ item.label }} × {{ item.count }}
                </Badge>
              </div>
            </div>

            <div>
              <div class="mb-2 text-xs uppercase tracking-[0.18em] text-muted-foreground">
                Status Code
              </div>
              <div
                v-if="selectedStatusCodes.length === 0"
                class="text-sm text-muted-foreground"
              >
                当前没有状态码样本。
              </div>
              <div
                v-else
                class="flex flex-wrap gap-2"
              >
                <Badge
                  v-for="item in selectedStatusCodes"
                  :key="item.label"
                  variant="outline"
                >
                  {{ item.label }} × {{ item.count }}
                </Badge>
              </div>
            </div>

            <div>
              <div class="mb-2 text-xs uppercase tracking-[0.18em] text-muted-foreground">
                延迟样本
              </div>
              <div class="grid grid-cols-3 gap-2 text-sm">
                <div class="rounded-lg border border-border/50 bg-card/70 px-3 py-2">
                  <div class="text-[11px] text-muted-foreground">
                    P50
                  </div>
                  <div class="mt-1 font-semibold">
                    {{ selectedLatencyStats.p50 }}
                  </div>
                </div>
                <div class="rounded-lg border border-border/50 bg-card/70 px-3 py-2">
                  <div class="text-[11px] text-muted-foreground">
                    P95
                  </div>
                  <div class="mt-1 font-semibold">
                    {{ selectedLatencyStats.p95 }}
                  </div>
                </div>
                <div class="rounded-lg border border-border/50 bg-card/70 px-3 py-2">
                  <div class="text-[11px] text-muted-foreground">
                    Max
                  </div>
                  <div class="mt-1 font-semibold">
                    {{ selectedLatencyStats.max }}
                  </div>
                </div>
              </div>
            </div>
          </div>
        </Card>
      </div>

      <Card
        variant="inset"
        class="p-4"
      >
        <div class="mb-3 flex items-center justify-between">
          <h4 class="text-sm font-semibold">
            最近活动
          </h4>
          <span class="text-xs text-muted-foreground">
            最近 {{ Math.min(selectedSortedEvents.length, 12) }} 条事件
          </span>
        </div>

        <div
          v-if="selectedSortedEvents.length === 0"
          class="py-6 text-sm text-muted-foreground"
        >
          当前没有事件样本。
        </div>

        <div
          v-else
          class="space-y-2"
        >
          <div
            v-for="event in selectedSortedEvents.slice(0, 12)"
            :key="buildEventKey(event)"
            class="flex flex-col gap-2 rounded-xl border border-border/50 bg-card/70 p-3 sm:flex-row sm:items-center sm:justify-between"
          >
            <div class="flex min-w-0 items-center gap-2 flex-wrap">
              <Badge :variant="getEventStatusVariant(event.status)">
                {{ getEventStatusLabel(event.status) }}
              </Badge>
              <span
                v-if="event.status_code"
                class="text-xs text-foreground"
              >
                HTTP {{ event.status_code }}
              </span>
              <span
                v-if="event.latency_ms != null"
                class="text-xs text-muted-foreground"
              >
                {{ formatLatency(event.latency_ms) }}
              </span>
              <span
                v-if="event.error_type"
                class="truncate text-xs text-muted-foreground"
              >
                {{ event.error_type }}
              </span>
            </div>
            <div class="text-xs text-muted-foreground">
              {{ formatAbsoluteTime(event.timestamp) }}
            </div>
          </div>
        </div>
      </Card>
    </div>
  </Dialog>

  <AlertDialog
    v-model="recoverAllConfirmOpen"
    title="恢复全部熔断 Key"
    :description="recoverAllDialogDescription"
    type="warning"
    confirm-text="立即恢复"
    :loading="recoveringAllKeys"
    @confirm="confirmRecoverAllKeys"
  />
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch, type Component } from 'vue'
import { useRouter } from 'vue-router'
import { Activity, AlertTriangle, Gauge, KeyRound, Loader2, Radio, ShieldAlert, X } from 'lucide-vue-next'
import AlertDialog from '@/components/common/AlertDialog.vue'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import Dialog from '@/components/ui/dialog/Dialog.vue'
import Label from '@/components/ui/label.vue'
import Switch from '@/components/ui/switch.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import EndpointHealthTimeline from './EndpointHealthTimeline.vue'
import { getKeysGroupedByFormat, type GroupedFormatKey } from '@/api/endpoints/keys'
import { getEndpointStatusMonitor, getHealthSummary, getPublicEndpointStatusMonitor, recoverAllKeysHealth, recoverKeyHealth } from '@/api/endpoints/health'
import type { EndpointHealthEvent, EndpointStatusMonitor, HealthSummary, PublicEndpointStatusMonitor, PublicHealthEvent } from '@/api/endpoints/types'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { formatApiFormat } from '@/api/endpoints/types/api-format'

type MonitorItem = EndpointStatusMonitor | PublicEndpointStatusMonitor
type MonitorEvent = EndpointHealthEvent | PublicHealthEvent
type MonitorStatus = 'healthy' | 'warning' | 'unhealthy' | 'unknown'
type SummaryCardItem = {
  label: string
  value: string
  help: string
  icon: Component
  valueClass: string
  iconClass: string
}
type CountItem = {
  label: string
  count: number
}
type KeyScopeFilter = 'all' | 'circuit-open' | 'recoverable'
type ProviderImpactSummary = {
  provider_id: string
  provider_name: string
  providerActive: boolean
  poolEnabled: boolean
  totalKeys: number
  activeKeys: number
  circuitOpenKeys: number
  avgHealthScore: number | null
  requestCount: number
}

const props = withDefaults(defineProps<{
  title?: string
  isAdmin?: boolean
  showProviderInfo?: boolean
}>(), {
  title: '健康监控',
  isAdmin: false,
  showProviderInfo: false
})

const AUTO_REFRESH_INTERVAL_MS = 15_000
const STATUS_SORT_ORDER: Record<MonitorStatus, number> = {
  unhealthy: 0,
  warning: 1,
  unknown: 2,
  healthy: 3
}
const DETAIL_ACTION_BUTTON_CLASS = 'h-8 px-3 text-xs'
const DETAIL_FILTER_BUTTON_CLASS = 'h-8 px-3 text-xs'

const router = useRouter()
const { error: showError, success: showSuccess, warning: showWarning } = useToast()

const loadingMonitors = ref(false)
const monitors = ref<MonitorItem[]>([])
const generatedAt = ref<string | null>(null)
const healthSummary = ref<HealthSummary | null>(null)
const lookbackHours = ref('6')
const statusFilter = ref<'all' | MonitorStatus>('all')
const familyFilter = ref<'all' | 'claude' | 'openai' | 'gemini' | 'other'>('all')
const onlyTraffic = ref(false)
const autoRefreshEnabled = ref(true)
const detailDialogOpen = ref(false)
const selectedMonitorFormat = ref<string | null>(null)
const recoverAllConfirmOpen = ref(false)
const recoveringAllKeys = ref(false)
const keysByFormat = ref<Record<string, GroupedFormatKey[]>>({})
const loadingFormatKeys = ref(false)
const recoveringKeyId = ref<string | null>(null)
const detailProviderFilter = ref<string>('all')
const detailKeyScopeFilter = ref<KeyScopeFilter>('all')

let autoRefreshTimer: ReturnType<typeof setInterval> | null = null
let loadRequestId = 0

const isAdmin = computed(() => props.isAdmin)
const showInitialLoading = computed(() => loadingMonitors.value && monitors.value.length === 0)

const unhealthyCount = computed(() =>
  monitors.value.filter(item => getMonitorStatus(item) === 'unhealthy').length
)

const warningCount = computed(() =>
  monitors.value.filter(item => getMonitorStatus(item) === 'warning').length
)

const trafficCount = computed(() =>
  monitors.value.filter(item => item.total_attempts > 0).length
)

const overallSuccessRate = computed(() => {
  const totals = monitors.value.reduce((acc, item) => {
    acc.success += item.success_count
    acc.failed += item.failed_count
    return acc
  }, { success: 0, failed: 0 })

  const denominator = totals.success + totals.failed
  if (denominator <= 0) return null
  return totals.success / denominator
})

const summaryCards = computed<SummaryCardItem[]>(() => {
  const items: SummaryCardItem[] = [
    {
      label: '异常格式',
      value: `${unhealthyCount.value}`,
      help: `共 ${monitors.value.length} 个格式`,
      icon: AlertTriangle,
      valueClass: unhealthyCount.value > 0 ? 'text-destructive' : 'text-foreground',
      iconClass: unhealthyCount.value > 0 ? 'bg-destructive/10 text-destructive' : 'bg-muted/60 text-muted-foreground'
    },
    {
      label: '预警格式',
      value: `${warningCount.value}`,
      help: '建议优先观察',
      icon: Radio,
      valueClass: warningCount.value > 0 ? 'text-yellow-600 dark:text-yellow-400' : 'text-foreground',
      iconClass: warningCount.value > 0 ? 'bg-yellow-500/10 text-yellow-600 dark:text-yellow-400' : 'bg-muted/60 text-muted-foreground'
    },
    {
      label: '有流量格式',
      value: `${trafficCount.value}`,
      help: `最近 ${lookbackHours.value} 小时`,
      icon: Activity,
      valueClass: 'text-foreground',
      iconClass: 'bg-primary/10 text-primary'
    },
    {
      label: '整体成功率',
      value: overallSuccessRate.value == null ? '--' : formatPercent(overallSuccessRate.value),
      help: '按 success / (success + failed)',
      icon: Gauge,
      valueClass: overallSuccessRate.value != null && overallSuccessRate.value < 0.95
        ? 'text-yellow-600 dark:text-yellow-400'
        : 'text-foreground',
      iconClass: 'bg-primary/10 text-primary'
    }
  ]

  if (props.isAdmin) {
    items.push(
      {
        label: '熔断 Key',
        value: `${healthSummary.value?.keys.circuit_open ?? 0}`,
        help: '已打开熔断器',
        icon: ShieldAlert,
        valueClass: (healthSummary.value?.keys.circuit_open ?? 0) > 0 ? 'text-destructive' : 'text-foreground',
        iconClass: (healthSummary.value?.keys.circuit_open ?? 0) > 0 ? 'bg-destructive/10 text-destructive' : 'bg-muted/60 text-muted-foreground'
      },
      {
        label: '活跃 Key',
        value: `${healthSummary.value?.keys.active ?? 0}`,
        help: '当前可调度密钥',
        icon: KeyRound,
        valueClass: 'text-foreground',
        iconClass: 'bg-primary/10 text-primary'
      }
    )
  }

  return items
})

const filteredMonitors = computed(() => {
  const filtered = monitors.value.filter((monitor) => {
    const status = getMonitorStatus(monitor)
    const family = getMonitorFamily(monitor.api_format)

    if (statusFilter.value !== 'all' && status !== statusFilter.value) {
      return false
    }

    if (familyFilter.value !== 'all' && family !== familyFilter.value) {
      return false
    }

    if (onlyTraffic.value && monitor.total_attempts <= 0) {
      return false
    }

    return true
  })

  return filtered.sort((left, right) => {
    const statusGap = STATUS_SORT_ORDER[getMonitorStatus(left)] - STATUS_SORT_ORDER[getMonitorStatus(right)]
    if (statusGap !== 0) return statusGap

    const leftTime = left.last_event_at ? new Date(left.last_event_at).getTime() : 0
    const rightTime = right.last_event_at ? new Date(right.last_event_at).getTime() : 0
    if (leftTime !== rightTime) return rightTime - leftTime

    if (left.total_attempts !== right.total_attempts) {
      return right.total_attempts - left.total_attempts
    }

    return left.api_format.localeCompare(right.api_format)
  })
})

const hasActiveFilters = computed(() =>
  statusFilter.value !== 'all' || familyFilter.value !== 'all' || onlyTraffic.value
)

const selectedMonitor = computed<MonitorItem | null>(() => {
  if (!selectedMonitorFormat.value) return null
  return monitors.value.find(item => item.api_format === selectedMonitorFormat.value) ?? null
})

const selectedSortedEvents = computed<MonitorEvent[]>(() => {
  const events = selectedMonitor.value?.events ?? []
  return [...events].sort((left, right) =>
    new Date(right.timestamp).getTime() - new Date(left.timestamp).getTime()
  )
})

const selectedFailureEvents = computed<MonitorEvent[]>(() =>
  selectedSortedEvents.value.filter(event => event.status === 'failed')
)

const selectedMonitorApiPath = computed(() =>
  selectedMonitor.value && 'api_path' in selectedMonitor.value ? selectedMonitor.value.api_path : null
)

const selectedFormatKeys = computed<GroupedFormatKey[]>(() => {
  if (!selectedMonitorFormat.value) return []
  const keys = keysByFormat.value[selectedMonitorFormat.value] ?? []
  return [...keys].sort((left, right) => {
    const circuitGap = Number(right.circuit_breaker_open) - Number(left.circuit_breaker_open)
    if (circuitGap !== 0) return circuitGap

    const leftHealth = left.health_score ?? 1
    const rightHealth = right.health_score ?? 1
    if (leftHealth !== rightHealth) return leftHealth - rightHealth

    if (left.request_count !== right.request_count) {
      return right.request_count - left.request_count
    }

    return left.name.localeCompare(right.name)
  })
})

const selectedProviderSummaries = computed<ProviderImpactSummary[]>(() => {
  const grouped = new Map<string, ProviderImpactSummary & { healthValues: number[] }>()

  for (const key of selectedFormatKeys.value) {
    const current = grouped.get(key.provider_id) ?? {
      provider_id: key.provider_id,
      provider_name: key.provider_name,
      providerActive: key.provider_active,
      poolEnabled: key.pool_enabled,
      totalKeys: 0,
      activeKeys: 0,
      circuitOpenKeys: 0,
      avgHealthScore: null,
      requestCount: 0,
      healthValues: []
    }

    current.totalKeys += 1
    current.requestCount += key.request_count
    current.providerActive = current.providerActive || key.provider_active
    current.poolEnabled = current.poolEnabled || key.pool_enabled

    if (key.is_active) current.activeKeys += 1
    if (key.circuit_breaker_open) current.circuitOpenKeys += 1
    if (key.health_score != null) current.healthValues.push(key.health_score)

    grouped.set(key.provider_id, current)
  }

  return Array.from(grouped.values())
    .map((provider) => ({
      provider_id: provider.provider_id,
      provider_name: provider.provider_name,
      providerActive: provider.providerActive,
      poolEnabled: provider.poolEnabled,
      totalKeys: provider.totalKeys,
      activeKeys: provider.activeKeys,
      circuitOpenKeys: provider.circuitOpenKeys,
      avgHealthScore: provider.healthValues.length > 0
        ? provider.healthValues.reduce((sum, value) => sum + value, 0) / provider.healthValues.length
        : null,
      requestCount: provider.requestCount,
    }))
    .sort((left, right) => {
      if (left.circuitOpenKeys !== right.circuitOpenKeys) {
        return right.circuitOpenKeys - left.circuitOpenKeys
      }

      const leftHealth = left.avgHealthScore ?? 1
      const rightHealth = right.avgHealthScore ?? 1
      if (leftHealth !== rightHealth) return leftHealth - rightHealth

      return right.requestCount - left.requestCount
    })
})

const detailProviderFilterName = computed(() =>
  selectedProviderSummaries.value.find(provider => provider.provider_id === detailProviderFilter.value)?.provider_name ?? '当前 Provider'
)

const filteredSelectedFormatKeys = computed<GroupedFormatKey[]>(() =>
  selectedFormatKeys.value.filter((key) => {
    if (detailProviderFilter.value !== 'all' && key.provider_id !== detailProviderFilter.value) {
      return false
    }

    if (detailKeyScopeFilter.value === 'circuit-open') {
      return key.circuit_breaker_open
    }

    if (detailKeyScopeFilter.value === 'recoverable') {
      return shouldShowRecoverHealthAction(key)
    }

    return true
  })
)

const detailDialogDescription = computed(() => {
  if (!selectedMonitor.value) return '查看最近失败、错误分布与事件样本'
  const timeText = selectedMonitor.value.last_event_at
    ? `最近事件 ${formatRelativeTime(selectedMonitor.value.last_event_at)}`
    : '当前窗口没有事件'
  return `${getMonitorStatusLabel(selectedMonitor.value)} · ${selectedMonitor.value.total_attempts} 次请求 · ${timeText}`
})

const selectedErrorTypes = computed<CountItem[]>(() =>
  buildCountItems(
    selectedFailureEvents.value
      .map(event => event.error_type?.trim())
      .filter((value): value is string => Boolean(value))
  )
)

const selectedStatusCodes = computed<CountItem[]>(() =>
  buildCountItems(
    selectedFailureEvents.value
      .map(event => event.status_code != null ? String(event.status_code) : null)
      .filter((value): value is string => Boolean(value))
  )
)

const selectedLatencyStats = computed(() => {
  const values = selectedSortedEvents.value
    .map(event => event.latency_ms)
    .filter((value): value is number => value != null)
    .sort((left, right) => left - right)

  if (values.length === 0) {
    return { p50: '--', p95: '--', max: '--' }
  }

  return {
    p50: formatLatency(pickPercentile(values, 0.5)),
    p95: formatLatency(pickPercentile(values, 0.95)),
    max: formatLatency(values[values.length - 1])
  }
})

const recoverAllDialogDescription = computed(() => {
  const circuitOpen = healthSummary.value?.keys.circuit_open ?? 0
  return [
    '这会重置所有已熔断 Key 的健康状态并关闭熔断器。',
    `当前检测到 **${circuitOpen}** 个熔断 Key。`,
    '建议在确认上游服务恢复后执行。'
  ].join('\n')
})

async function loadMonitors(silent = false) {
  const currentRequestId = ++loadRequestId
  loadingMonitors.value = true

  try {
    const params = {
      lookback_hours: parseInt(lookbackHours.value),
      per_format_limit: 60
    }

    const monitorPromise = props.isAdmin
      ? getEndpointStatusMonitor(params)
      : getPublicEndpointStatusMonitor(params)

    const [monitorResult, summaryResult] = await Promise.allSettled([
      monitorPromise,
      props.isAdmin ? getHealthSummary() : Promise.resolve(null)
    ])

    if (currentRequestId !== loadRequestId) return

    if (monitorResult.status === 'fulfilled') {
      monitors.value = monitorResult.value.formats || []
      generatedAt.value = monitorResult.value.generated_at ?? null
    } else if (!silent) {
      showError(parseApiError(monitorResult.reason, '加载健康监控数据失败'), '错误')
    }

    if (props.isAdmin) {
      if (summaryResult.status === 'fulfilled') {
        healthSummary.value = summaryResult.value
      }
    } else {
      healthSummary.value = null
    }

    if (selectedMonitorFormat.value && !monitors.value.some(item => item.api_format === selectedMonitorFormat.value)) {
      detailDialogOpen.value = false
      selectedMonitorFormat.value = null
    }
  } catch (err: unknown) {
    if (!silent) {
      showError(parseApiError(err, '加载健康监控数据失败'), '错误')
    }
  } finally {
    if (currentRequestId === loadRequestId) {
      loadingMonitors.value = false
    }
  }
}

async function refreshData(options: { silent?: boolean } = {}) {
  await loadMonitors(Boolean(options.silent))
}

async function loadKeysByFormat(force = false) {
  if (!props.isAdmin) return
  if (!force && Object.keys(keysByFormat.value).length > 0) return

  loadingFormatKeys.value = true
  try {
    keysByFormat.value = await getKeysGroupedByFormat()
  } catch (err: unknown) {
    showError(parseApiError(err, '加载 Key 诊断数据失败'), '错误')
  } finally {
    loadingFormatKeys.value = false
  }
}

function resetFilters() {
  statusFilter.value = 'all'
  familyFilter.value = 'all'
  onlyTraffic.value = false
}

function resetDetailFilters() {
  detailProviderFilter.value = 'all'
  detailKeyScopeFilter.value = 'all'
}

function stopAutoRefresh() {
  if (!autoRefreshTimer) return
  clearInterval(autoRefreshTimer)
  autoRefreshTimer = null
}

function startAutoRefresh() {
  stopAutoRefresh()
  if (!autoRefreshEnabled.value) return

  autoRefreshTimer = setInterval(() => {
    if (document.visibilityState === 'hidden') return
    void refreshData({ silent: true })
  }, AUTO_REFRESH_INTERVAL_MS)
}

function handleVisibilityChange() {
  if (document.visibilityState === 'visible' && autoRefreshEnabled.value) {
    void refreshData({ silent: true })
  }
}

async function openMonitorDetail(monitor: MonitorItem) {
  selectedMonitorFormat.value = monitor.api_format
  resetDetailFilters()
  detailDialogOpen.value = true
  if (props.isAdmin) {
    await loadKeysByFormat()
  }
}

async function confirmRecoverAllKeys() {
  if (recoveringAllKeys.value) return

  recoveringAllKeys.value = true
  try {
    const result = await recoverAllKeysHealth()
    recoverAllConfirmOpen.value = false

    if (result.recovered_count > 0) {
      showSuccess(result.message || `已恢复 ${result.recovered_count} 个熔断 Key`)
    } else {
      showWarning(result.message || '当前没有需要恢复的熔断 Key')
    }

    await refreshData({ silent: false })
  } catch (err: unknown) {
    showError(parseApiError(err, '批量恢复熔断 Key 失败'), '错误')
  } finally {
    recoveringAllKeys.value = false
  }
}

async function recoverFormatKey(key: GroupedFormatKey) {
  if (recoveringKeyId.value || !selectedMonitorFormat.value) return

  recoveringKeyId.value = key.id
  try {
    const result = await recoverKeyHealth(key.id, selectedMonitorFormat.value)
    showSuccess(result.message || '当前格式已恢复')
    await Promise.all([
      refreshData({ silent: true }),
      loadKeysByFormat(true)
    ])
  } catch (err: unknown) {
    showError(parseApiError(err, '恢复 Key 健康状态失败'), '错误')
  } finally {
    recoveringKeyId.value = null
  }
}

function shouldShowRecoverHealthAction(key: GroupedFormatKey): boolean {
  if (key.health_score == null) return false
  return key.health_score < 0.999
}

function toggleDetailProviderFilter(providerId: string) {
  detailProviderFilter.value = detailProviderFilter.value === providerId ? 'all' : providerId
}

function goToProviderManagement(providerId: string) {
  void router.push({
    name: 'ProviderManagement',
    query: { providerId }
  })
}

function goToPoolManagement(providerId: string, search?: string) {
  const query: Record<string, string> = { providerId }
  const normalizedSearch = search?.trim()
  if (normalizedSearch) {
    query.search = normalizedSearch
  }

  void router.push({
    name: 'PoolManagement',
    query
  })
}

function getMonitorFamily(apiFormat: string): 'claude' | 'openai' | 'gemini' | 'other' {
  if (apiFormat.startsWith('claude:')) return 'claude'
  if (apiFormat.startsWith('openai:')) return 'openai'
  if (apiFormat.startsWith('gemini:')) return 'gemini'
  return 'other'
}

function getLatestTimelineStatus(monitor: MonitorItem): MonitorStatus | null {
  const timeline = monitor.timeline ?? []
  for (let index = timeline.length - 1; index >= 0; index -= 1) {
    const status = timeline[index]
    if (status === 'healthy' || status === 'warning' || status === 'unhealthy') {
      return status
    }
  }
  return null
}

function getMonitorStatus(monitor: MonitorItem): MonitorStatus {
  const timelineStatus = getLatestTimelineStatus(monitor)
  if (timelineStatus === 'unhealthy') return 'unhealthy'
  if (timelineStatus === 'warning') return 'warning'

  if (monitor.total_attempts <= 0) {
    return 'unknown'
  }

  if (monitor.success_rate < 0.7) return 'unhealthy'
  if (monitor.success_rate < 0.95) return 'warning'
  if (timelineStatus === 'healthy' || monitor.success_rate >= 0.95) return 'healthy'
  return 'unknown'
}

function getMonitorStatusLabel(monitor: MonitorItem): string {
  switch (getMonitorStatus(monitor)) {
    case 'unhealthy':
      return '异常'
    case 'warning':
      return '预警'
    case 'healthy':
      return '健康'
    default:
      return '无流量'
  }
}

function getMonitorStatusVariant(monitor: MonitorItem): 'success' | 'warning' | 'destructive' | 'outline' {
  switch (getMonitorStatus(monitor)) {
    case 'healthy':
      return 'success'
    case 'warning':
      return 'warning'
    case 'unhealthy':
      return 'destructive'
    default:
      return 'outline'
  }
}

function getMonitorCardClass(monitor: MonitorItem): string {
  switch (getMonitorStatus(monitor)) {
    case 'unhealthy':
      return 'border-red-300/70 bg-red-500/5 hover:border-red-400/80'
    case 'warning':
      return 'border-yellow-300/70 bg-yellow-500/5 hover:border-yellow-400/80'
    case 'healthy':
      return 'border-border/60 hover:border-primary/50'
    default:
      return 'border-border/40 bg-muted/20 hover:border-border/70'
  }
}

function getSuccessRateVariant(rate: number): 'success' | 'warning' | 'destructive' | 'outline' {
  if (rate >= 0.95) return 'success'
  if (rate >= 0.8) return 'warning'
  if (rate >= 0) return 'destructive'
  return 'outline'
}

function getEventStatusVariant(status: string): 'success' | 'warning' | 'destructive' | 'outline' {
  switch (status) {
    case 'success':
      return 'success'
    case 'failed':
      return 'destructive'
    case 'skipped':
      return 'warning'
    default:
      return 'outline'
  }
}

function getEventStatusLabel(status: string): string {
  switch (status) {
    case 'success':
      return '成功'
    case 'failed':
      return '失败'
    case 'skipped':
      return '跳过'
    case 'started':
      return '执行中'
    default:
      return '未知'
  }
}

function buildCountItems(values: string[]): CountItem[] {
  const counter = new Map<string, number>()
  for (const value of values) {
    counter.set(value, (counter.get(value) ?? 0) + 1)
  }
  return Array.from(counter.entries())
    .map(([label, count]) => ({ label, count }))
    .sort((left, right) => right.count - left.count)
    .slice(0, 8)
}

function pickPercentile(sortedValues: number[], percentile: number): number {
  if (sortedValues.length === 0) return 0
  const index = Math.min(sortedValues.length - 1, Math.max(0, Math.ceil(sortedValues.length * percentile) - 1))
  return sortedValues[index]
}

function formatPercent(rate: number): string {
  return `${(rate * 100).toFixed(0)}%`
}

function formatScore(score: number | null | undefined): string {
  if (score == null) return '--'
  return `${(score * 100).toFixed(0)}%`
}

function formatLatency(latencyMs: number): string {
  if (latencyMs < 1000) return `${latencyMs}ms`
  return `${(latencyMs / 1000).toFixed(2)}s`
}

function formatRelativeTime(dateString: string): string {
  const diff = Date.now() - new Date(dateString).getTime()
  if (Number.isNaN(diff)) return '未知'
  if (diff < 60_000) return '刚刚'

  const minutes = Math.floor(diff / 60_000)
  if (minutes < 60) return `${minutes} 分钟前`

  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时前`

  const days = Math.floor(hours / 24)
  if (days < 7) return `${days} 天前`

  return formatAbsoluteTime(dateString)
}

function formatAbsoluteTime(dateString: string): string {
  return new Date(dateString).toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

function buildEventKey(event: MonitorEvent): string {
  return `${event.timestamp}-${event.status}-${event.status_code ?? 'na'}-${event.error_type ?? 'none'}-${event.latency_ms ?? 'na'}`
}

watch(lookbackHours, () => {
  void refreshData({ silent: false })
})

watch(() => props.isAdmin, () => {
  keysByFormat.value = {}
  void refreshData({ silent: false })
})

watch(autoRefreshEnabled, () => {
  startAutoRefresh()
})

watch(detailDialogOpen, (open) => {
  if (!open) {
    selectedMonitorFormat.value = null
    resetDetailFilters()
  }
})

watch(selectedFormatKeys, (keys) => {
  if (detailProviderFilter.value === 'all') return
  if (!keys.some(key => key.provider_id === detailProviderFilter.value)) {
    detailProviderFilter.value = 'all'
  }
})

onMounted(() => {
  document.addEventListener('visibilitychange', handleVisibilityChange)
  startAutoRefresh()
  void refreshData({ silent: false })
})

onUnmounted(() => {
  document.removeEventListener('visibilitychange', handleVisibilityChange)
  stopAutoRefresh()
})
</script>
