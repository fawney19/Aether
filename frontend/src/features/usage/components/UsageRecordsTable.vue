<template>
  <TableCard
    title="使用记录"
    class="relative"
  >
    <template #actions>
      <!-- 时间范围筛选 -->
      <TimeRangePicker
        v-model="timeRangeModel"
        :show-granularity="false"
        class="hidden shrink-0 md:flex"
      />

      <!-- 分隔线 -->
      <div class="hidden sm:block h-4 w-px bg-border" />

      <!-- 通用搜索 -->
      <div class="order-1 flex w-full items-center gap-2 md:order-none md:w-auto">
        <div class="relative min-w-0 flex-1 md:w-48 md:flex-none">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
          <Input
            id="usage-records-search"
            v-model="localSearch"
            :placeholder="isAdmin ? '搜索用户/密钥' : '搜索密钥/模型'"
            class="h-8 w-full text-xs border-border/60 pl-8"
          />
        </div>
      </div>

      <Button
        variant="ghost"
        size="icon"
        data-usage-hide-unknown-toggle="mobile"
        class="absolute right-12 top-2.5 h-8 w-8 shrink-0 md:hidden"
        :class="hideUnknownRecords ? 'text-primary' : ''"
        :title="hideUnknownRecords ? '显示 unknown 请求' : '隐藏 unknown 请求'"
        aria-label="隐藏 unknown 模型或提供商的请求"
        :aria-pressed="hideUnknownRecords"
        @click="$emit('update:hideUnknownRecords', !hideUnknownRecords)"
      >
        <EyeOff class="w-3.5 h-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="absolute right-4 top-2.5 h-8 w-8 shrink-0 md:hidden"
        :class="autoRefresh ? 'text-primary' : ''"
        :title="autoRefresh ? '点击关闭自动刷新' : '点击开启自动刷新'"
        @click="$emit('update:autoRefresh', !autoRefresh)"
      >
        <RefreshCcw
          class="w-3.5 h-3.5"
          :class="autoRefresh ? 'animate-spin' : ''"
        />
      </Button>

      <div class="order-3 grid w-full grid-cols-2 gap-2 md:hidden">
        <!-- 时间范围筛选 -->
        <TimeRangePicker
          v-model="timeRangeModel"
          :show-granularity="false"
          class="min-w-0"
          preset-trigger-class="!w-full"
        />

        <!-- 用户筛选（仅管理员可见） -->
        <ServerUserSelector
          v-if="isAdmin"
          class="min-w-0"
          :model-value="filterUser"
          :initial-users="availableUsers"
          dropdown
          @update:model-value="$emit('update:filterUser', $event)"
        />

        <!-- 模型筛选 -->
        <Select
          :model-value="filterModel"
          @update:model-value="$emit('update:filterModel', $event)"
        >
          <SelectTrigger class="h-8 w-full min-w-0 text-xs border-border/60">
            <SelectValue placeholder="模型" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">
              全部模型
            </SelectItem>
            <SelectItem
              v-for="model in availableModels"
              :key="model"
              :value="model"
            >
              {{ model.replace('claude-', '') }}
            </SelectItem>
          </SelectContent>
        </Select>

        <!-- 提供商筛选（仅管理员可见） -->
        <Select
          v-if="isAdmin"
          :model-value="filterProvider"
          @update:model-value="$emit('update:filterProvider', $event)"
        >
          <SelectTrigger class="h-8 w-full min-w-0 text-xs border-border/60">
            <SelectValue placeholder="提供商" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">
              全部提供商
            </SelectItem>
            <SelectItem
              v-for="provider in availableProviders"
              :key="provider"
              :value="provider"
            >
              {{ provider }}
            </SelectItem>
          </SelectContent>
        </Select>

        <!-- API格式筛选 -->
        <Select
          :model-value="filterApiFormat"
          @update:model-value="$emit('update:filterApiFormat', $event)"
        >
          <SelectTrigger class="h-8 w-full min-w-0 text-xs border-border/60">
            <SelectValue placeholder="格式" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">
              全部格式
            </SelectItem>
            <SelectItem
              v-for="format in availableApiFormats"
              :key="format.value"
              :value="format.value"
            >
              {{ format.label }}
            </SelectItem>
          </SelectContent>
        </Select>

        <!-- 状态筛选 -->
        <Select
          :model-value="filterStatus"
          @update:model-value="$emit('update:filterStatus', $event)"
        >
          <SelectTrigger class="h-8 w-full min-w-0 text-xs border-border/60">
            <SelectValue placeholder="状态" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__all__">
              全部状态
            </SelectItem>
            <SelectItem value="stream">
              流式
            </SelectItem>
            <SelectItem value="standard">
              标准
            </SelectItem>
            <SelectItem value="active">
              活跃
            </SelectItem>
            <SelectItem value="failed">
              失败
            </SelectItem>
            <SelectItem value="cancelled">
              已取消
            </SelectItem>
            <SelectItem value="has_retry">
              发生重试
            </SelectItem>
            <SelectItem value="has_fallback">
              发生转移
            </SelectItem>
          </SelectContent>
        </Select>
      </div>

      <!-- 分隔线 -->
      <div class="hidden sm:block h-4 w-px bg-border" />

      <!-- 列显示配置（桌面端） -->
      <MultiSelect
        v-model="visibleColumnIds"
        class="hidden md:block"
        :options="columnSelectOptions"
        placeholder="显示列"
        trigger-class="w-40 h-8 text-xs border-border/60"
        dropdown-min-width="14rem"
        :searchable="false"
      />

      <!-- 分隔线 -->
      <div class="hidden sm:block h-4 w-px bg-border" />

      <!-- 自动刷新按钮 -->
      <Button
        variant="ghost"
        size="icon"
        data-usage-hide-unknown-toggle="desktop"
        class="hidden h-8 w-8 shrink-0 md:inline-flex"
        :class="hideUnknownRecords ? 'text-primary' : ''"
        :title="hideUnknownRecords ? '显示 unknown 请求' : '隐藏 unknown 请求'"
        aria-label="隐藏 unknown 模型或提供商的请求"
        :aria-pressed="hideUnknownRecords"
        @click="$emit('update:hideUnknownRecords', !hideUnknownRecords)"
      >
        <EyeOff class="w-3.5 h-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="hidden h-8 w-8 shrink-0 md:inline-flex"
        :class="autoRefresh ? 'text-primary' : ''"
        :title="autoRefresh ? '点击关闭自动刷新' : '点击开启自动刷新'"
        @click="$emit('update:autoRefresh', !autoRefresh)"
      >
        <RefreshCcw
          class="w-3.5 h-3.5"
          :class="autoRefresh ? 'animate-spin' : ''"
        />
      </Button>
    </template>

    <!-- 移动端卡片视图 -->
    <div class="md:hidden">
      <div
        v-if="records.length === 0"
        class="text-center py-12 text-muted-foreground"
      >
        暂无请求记录
      </div>
      <div
        v-for="record in records"
        v-else
        :key="record.id"
        class="border-b border-border/40 px-3 py-2.5"
        :class="isAdmin ? 'cursor-pointer active:bg-muted/30 transition-colors' : ''"
        @click="isAdmin && emit('showDetail', record.id)"
      >
        <!-- 第一行：模型 + 费用 -->
        <div class="flex items-start justify-between gap-2">
          <div class="min-w-0 flex-1">
            <div class="flex min-w-0 items-center gap-1.5">
              <span class="min-w-0 truncate text-[15px] font-semibold leading-5">{{ record.model }}</span>
              <Badge
                v-if="getReasoningEffort(record)"
                variant="outline"
                class="h-4 rounded-full border-primary/30 bg-primary/5 px-1.5 text-[10px] leading-4 text-primary flex-shrink-0"
                :title="getReasoningEffortTitle(record)"
              >
                {{ getReasoningEffort(record) }}
              </Badge>
              <Badge
                v-if="getFastBadge(record)"
                variant="outline"
                class="h-4 rounded-full px-1.5 text-[10px] leading-4 text-foreground flex-shrink-0"
                :title="getFastBadgeTitle(record)"
              >
                fast
              </Badge>
              <!-- 状态 Badge -->
              <Badge
                v-if="isUsageRecordFailed(record)"
                variant="destructive"
                class="whitespace-nowrap text-[10px] px-1.5 h-4 leading-4 inline-flex items-center flex-shrink-0"
              >
                失败
              </Badge>
              <Badge
                v-else-if="getDisplayStatus(record) === 'pending'"
                variant="outline"
                class="whitespace-nowrap animate-pulse border-muted-foreground/30 text-muted-foreground text-[10px] px-1.5 h-4 leading-4 inline-flex items-center flex-shrink-0"
              >
                等待
              </Badge>
              <Badge
                v-else-if="getDisplayStatus(record) === 'streaming'"
                variant="outline"
                class="whitespace-nowrap animate-pulse border-primary/50 text-primary text-[10px] px-1.5 h-4 leading-4 inline-flex items-center flex-shrink-0"
              >
                传输
              </Badge>
              <Badge
                v-else-if="record.status === 'cancelled'"
                variant="outline"
                class="whitespace-nowrap border-amber-500/50 text-amber-600 dark:text-amber-400 text-[10px] px-1.5 h-4 leading-4 inline-flex items-center flex-shrink-0"
              >
                取消
              </Badge>
              <Badge
                v-else-if="getStreamModeSegments(record).hasConversion"
                :variant="streamBadgeVariant(getStreamModeSegments(record).client === '流式')"
                :class="(streamBadgeVariant(getStreamModeSegments(record).client === '流式') === 'secondary')
                  ? 'whitespace-nowrap text-[10px] px-1.5 h-4 leading-4 inline-flex items-center gap-0.5 flex-shrink-0'
                  : 'whitespace-nowrap border-border/60 text-muted-foreground text-[10px] px-1.5 h-4 leading-4 inline-flex items-center gap-0.5 flex-shrink-0'"
              >
                <span>{{ getStreamModeSegments(record).client }}</span>
                <span class="opacity-60">→</span>
                <span>{{ getStreamModeSegments(record).upstream }}</span>
              </Badge>
              <Badge
                v-else
                :variant="streamBadgeVariant(getUpstreamStream(record))"
                :class="(streamBadgeVariant(getUpstreamStream(record)) === 'secondary')
                  ? 'whitespace-nowrap text-[10px] px-1.5 h-4 leading-4 inline-flex items-center flex-shrink-0'
                  : 'whitespace-nowrap border-border/60 text-muted-foreground text-[10px] px-1.5 h-4 leading-4 inline-flex items-center flex-shrink-0'"
              >
                {{ getStreamModeLabel(record) }}
              </Badge>
            </div>
            <span
              v-if="getActualModel(record)"
              class="text-[11px] text-muted-foreground truncate block"
            >-> {{ getActualModel(record) }}</span>
          </div>
          <div class="flex flex-col items-end flex-shrink-0">
            <span class="text-sm text-primary font-semibold leading-5">{{ formatCurrency(record.cost || 0) }}</span>
            <span
              v-if="showActualCost && record.actual_cost !== undefined && record.rate_multiplier && record.rate_multiplier !== 1.0"
              class="text-[10px] text-muted-foreground"
            >{{ formatCurrency(record.actual_cost) }}</span>
          </div>
        </div>

        <!-- 第二行：时间 + API格式 -->
        <div class="mt-1.5 flex min-w-0 items-center gap-1.5 text-[10px] leading-3.5 text-muted-foreground">
          <span class="shrink-0 tabular-nums text-foreground whitespace-nowrap">
            {{ formatRecordTime(record.created_at) }}
          </span>
          <span class="shrink-0 tabular-nums whitespace-nowrap">
            {{ formatRecordShortDate(record.created_at) }}
          </span>
          <template v-if="record.api_format">
            <span class="text-muted-foreground/40">·</span>
            <span class="min-w-0 truncate">{{ formatApiFormat(record.api_format) }}</span>
          </template>
        </div>

        <!-- 第三行：性能指标 -->
        <div class="mt-1.5 flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-1 text-[10px] leading-3.5 text-muted-foreground">
          <span
            class="min-w-0 truncate whitespace-nowrap tabular-nums text-foreground"
            :title="getRecordPerformanceTitle(record)"
          >
            <span class="text-muted-foreground">耗时&amp;速度</span>
            <template v-if="getDisplayStatus(record) === 'pending' || getDisplayStatus(record) === 'streaming'">
              <span class="ml-1">{{ formatRecordDurationSeconds(record.first_byte_time_ms) }}</span>
              <span class="text-muted-foreground"> / </span>
              <ElapsedTimeText
                class="text-primary"
                :created-at="record.created_at"
                :response-time-updated-at="record.response_time_updated_at ?? null"
                :status="getDisplayStatus(record)"
                :response-time-ms="record.response_time_ms ?? null"
              />
              <span class="text-muted-foreground"> / </span>
              <span>{{ formatOutputRate(getRecordDisplayOutputRate(record)) }}</span>
            </template>
            <span
              v-else-if="record.response_time_ms != null || record.first_byte_time_ms != null"
              class="ml-1"
            >{{ formatRecordLatencyPair(record) }} / {{ formatOutputRate(getRecordDisplayOutputRate(record)) }}</span>
            <span
              v-else
              class="ml-1"
            >- / {{ formatOutputRate(getRecordDisplayOutputRate(record)) }}</span>
          </span>
          <span class="text-muted-foreground/40">·</span>
          <span
            class="min-w-0 truncate whitespace-nowrap tabular-nums text-foreground"
            :title="hasRecordCacheTokens(record) ? getRecordCacheTokensTitle(record) : undefined"
          >
            <span class="text-muted-foreground">Tokens</span>
            <span class="ml-1">{{ formatTokens(getRecordEffectiveInputTokens(record)) }} / {{ formatTokens(record.output_tokens || 0) }}</span>
            <template v-if="hasRecordCacheTokens(record)">
              <span class="text-muted-foreground"> | </span>
              <span>{{ formatOptionalTokens(getRecordCacheReadTokens(record)) }} / {{ formatOptionalTokens(getRecordCacheCreationTokens(record)) }}</span>
            </template>
          </span>
        </div>
      </div>
    </div>

    <!-- 桌面端表格视图 -->
    <Table
      class="hidden md:table table-fixed w-full"
      :class="[desktopTableMinWidthClass]"
    >
      <colgroup v-if="isAdmin">
        <col
          v-if="isColumnVisible('time')"
          class="w-[8%]"
        >
        <col
          v-if="isColumnVisible('user')"
          class="w-[12%]"
        >
        <col
          v-if="isColumnVisible('model')"
          class="w-[14%]"
        >
        <col
          v-if="isColumnVisible('provider')"
          class="w-[16%]"
        >
        <col
          v-if="isColumnVisible('api_format')"
          class="w-[15%]"
        >
        <col
          v-if="isColumnVisible('status')"
          class="w-[10%]"
        >
        <col
          v-if="isColumnVisible('tokens')"
          class="w-[10%]"
        >
        <col
          v-if="isColumnVisible('cost')"
          class="w-[6%]"
        >
        <col
          v-if="isColumnVisible('performance')"
          class="w-[9%]"
        >
        <col
          v-if="isColumnVisible('client_family')"
          class="w-[12%]"
        >
        <col
          v-if="isColumnVisible('client_ip')"
          class="w-[10%]"
        >
        <col
          v-if="isColumnVisible('user_agent')"
          class="w-[13%]"
        >
      </colgroup>
      <colgroup v-else>
        <col
          v-if="isColumnVisible('time')"
          class="w-[9%]"
        >
        <col
          v-if="isColumnVisible('key')"
          class="w-[17%]"
        >
        <col
          v-if="isColumnVisible('model')"
          class="w-[22%]"
        >
        <col
          v-if="isColumnVisible('api_format')"
          class="w-[14%]"
        >
        <col
          v-if="isColumnVisible('status')"
          class="w-[10%]"
        >
        <col
          v-if="isColumnVisible('tokens')"
          class="w-[11%]"
        >
        <col
          v-if="isColumnVisible('cost')"
          class="w-[7%]"
        >
        <col
          v-if="isColumnVisible('performance')"
          class="w-[10%]"
        >
        <col
          v-if="isColumnVisible('client_family')"
          class="w-[12%]"
        >
        <col
          v-if="isColumnVisible('client_ip')"
          class="w-[10%]"
        >
        <col
          v-if="isColumnVisible('user_agent')"
          class="w-[13%]"
        >
      </colgroup>
      <TableHeader>
        <TableRow class="border-b border-border/60 hover:bg-transparent">
          <TableHead
            v-if="isColumnVisible('time')"
            class="h-12 font-semibold w-[8%]"
          >
            时间
          </TableHead>
          <SortableTableHead
            v-if="isAdmin && isColumnVisible('user')"
            class="h-12 font-semibold w-[12%]"
            column-key="user"
            :sortable="false"
            :filter-active="filterUser !== '__all__'"
            filter-title="筛选用户"
            filter-content-class="w-64 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
          >
            用户
            <template #filter="{ close }">
              <ServerUserSelector
                :model-value="filterUser"
                :initial-users="availableUsers"
                @update:model-value="$emit('update:filterUser', $event)"
                @select="close"
              />
            </template>
          </SortableTableHead>
          <TableHead
            v-if="!isAdmin && isColumnVisible('key')"
            class="h-12 font-semibold w-[17%]"
          >
            密钥
          </TableHead>
          <SortableTableHead
            v-if="isColumnVisible('model')"
            class="h-12 font-semibold"
            :class="[isAdmin ? 'w-[14%]' : 'w-[22%]']"
            column-key="model"
            :sortable="false"
            :filter-active="filterModel !== '__all__'"
            filter-title="筛选模型"
            filter-content-class="w-64 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
          >
            模型
            <template #filter="{ close }">
              <TableFilterMenu
                :model-value="filterModel"
                :options="modelFilterOptions"
                @update:model-value="$emit('update:filterModel', $event)"
                @select="close"
              />
            </template>
          </SortableTableHead>
          <SortableTableHead
            v-if="isAdmin && isColumnVisible('provider')"
            class="h-12 font-semibold w-[16%]"
            column-key="provider"
            :sortable="false"
            :filter-active="filterProvider !== '__all__'"
            filter-title="筛选提供商"
            filter-content-class="w-48 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
          >
            提供商
            <template #filter="{ close }">
              <TableFilterMenu
                :model-value="filterProvider"
                :options="providerFilterOptions"
                @update:model-value="$emit('update:filterProvider', $event)"
                @select="close"
              />
            </template>
          </SortableTableHead>
          <SortableTableHead
            v-if="isColumnVisible('api_format')"
            class="h-12 font-semibold"
            :class="[isAdmin ? 'w-[15%]' : 'w-[14%]']"
            column-key="api_format"
            :sortable="false"
            :filter-active="filterApiFormat !== '__all__'"
            filter-title="筛选 API 格式"
            filter-content-class="w-72 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
          >
            API格式
            <template #filter="{ close }">
              <TableFilterMenu
                :model-value="filterApiFormat"
                :options="apiFormatFilterOptions"
                @update:model-value="$emit('update:filterApiFormat', $event)"
                @select="close"
              />
            </template>
          </SortableTableHead>
          <SortableTableHead
            v-if="isColumnVisible('status')"
            class="h-12 font-semibold w-[10%] text-center"
            column-key="status"
            :sortable="false"
            align="center"
            :filter-active="filterStatus !== '__all__'"
            filter-title="筛选类型"
            filter-content-class="w-44 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
          >
            类型
            <template #filter="{ close }">
              <TableFilterMenu
                :model-value="filterStatus"
                :options="statusFilterOptions"
                @update:model-value="$emit('update:filterStatus', $event)"
                @select="close"
              />
            </template>
          </SortableTableHead>
          <TableHead
            v-if="isColumnVisible('tokens')"
            class="h-12 font-semibold w-[10%] text-center"
          >
            Tokens
          </TableHead>
          <TableHead
            v-if="isColumnVisible('cost')"
            class="h-12 font-semibold w-[6%] text-right"
          >
            费用
          </TableHead>
          <TableHead
            v-if="isColumnVisible('performance')"
            class="h-12 font-semibold w-[9%] text-right"
          >
            <div class="flex flex-col items-end text-xs gap-0.5">
              <span class="whitespace-nowrap">首字/总耗时</span>
              <span class="text-muted-foreground font-normal">输出速度</span>
            </div>
          </TableHead>
          <SortableTableHead
            v-if="isColumnVisible('client_family')"
            class="h-12 font-semibold w-[12%]"
            column-key="client_family"
            :sortable="false"
            :filter-active="filterClientFamily !== '__all__'"
            filter-title="筛选客户端"
            filter-content-class="w-44 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
          >
            客户端
            <template #filter="{ close }">
              <TableFilterMenu
                :model-value="filterClientFamily"
                :options="clientFamilyFilterOptions"
                @update:model-value="$emit('update:filterClientFamily', $event)"
                @select="close"
              />
            </template>
          </SortableTableHead>
          <TableHead
            v-if="isColumnVisible('client_ip')"
            class="h-12 font-semibold w-[10%]"
          >
            IP 地址
          </TableHead>
          <TableHead
            v-if="isColumnVisible('user_agent')"
            class="h-12 font-semibold w-[13%]"
          >
            User-Agent
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-if="records.length === 0">
          <TableCell
            :colspan="visibleColumnCount"
            class="text-center py-12 text-muted-foreground"
          >
            暂无请求记录
          </TableCell>
        </TableRow>
        <TableRow
          v-for="record in records"
          v-else
          :key="record.id"
          :class="isAdmin ? 'cursor-pointer border-b border-border/40 hover:bg-muted/30 transition-colors h-[72px]' : 'border-b border-border/40 hover:bg-muted/30 transition-colors h-[72px]'"
          @mousedown="handleRowMouseDown($event, record.id)"
          @click="handleRowClick($event, record.id)"
        >
          <TableCell
            v-if="isColumnVisible('time')"
            class="py-4 w-[8%] align-top"
          >
            <div class="flex flex-col gap-0.5 leading-tight">
              <span class="text-xs text-foreground tabular-nums whitespace-nowrap">
                {{ formatRecordTime(record.created_at) }}
              </span>
              <span class="text-[11px] text-muted-foreground tabular-nums whitespace-nowrap">
                {{ formatRecordDate(record.created_at) }}
              </span>
            </div>
          </TableCell>
          <TableCell
            v-if="isAdmin && isColumnVisible('user')"
            class="py-4 w-[12%] truncate"
            :title="record.username || record.user_email || (record.user_id ? `User ${record.user_id}` : '已删除用户')"
          >
            <div class="flex flex-col text-xs gap-0.5">
              <span class="truncate">
                {{ record.username || record.user_email || (record.user_id ? `User ${record.user_id}` : '已删除用户') }}
              </span>
              <span
                v-if="record.api_key?.name"
                class="text-muted-foreground truncate"
                :title="record.api_key.name"
              >
                {{ record.api_key.name }}
              </span>
            </div>
          </TableCell>
          <!-- 用户页面的密钥列 -->
          <TableCell
            v-if="!isAdmin && isColumnVisible('key')"
            class="py-4 w-[17%]"
            :title="record.api_key?.name || '-'"
          >
            <div class="flex flex-col text-xs gap-0.5">
              <span class="truncate">{{ record.api_key?.name || '-' }}</span>
              <span
                v-if="record.api_key?.display"
                class="text-muted-foreground truncate"
              >
                {{ record.api_key.display }}
              </span>
            </div>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('model')"
            class="font-medium py-4"
            :class="[isAdmin ? 'w-[14%]' : 'w-[22%]']"
            :title="getModelTooltip(record)"
          >
            <div
              v-if="getActualModel(record)"
              class="flex flex-col text-xs gap-0.5"
            >
              <div class="flex min-w-0 items-center gap-1">
                <span class="truncate">{{ record.model }}</span>
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  viewBox="0 0 20 20"
                  fill="currentColor"
                  class="w-3 h-3 text-muted-foreground flex-shrink-0"
                >
                  <path
                    fill-rule="evenodd"
                    d="M3 10a.75.75 0 01.75-.75h10.638L10.23 5.29a.75.75 0 111.04-1.08l5.5 5.25a.75.75 0 010 1.08l-5.5 5.25a.75.75 0 11-1.04-1.08l4.158-3.96H3.75A.75.75 0 013 10z"
                    clip-rule="evenodd"
                  />
                </svg>
                <Badge
                  v-if="getReasoningEffort(record)"
                  variant="outline"
                  class="h-4 rounded-full border-primary/30 bg-primary/5 px-1.5 text-[10px] leading-4 text-primary flex-shrink-0"
                  :title="getReasoningEffortTitle(record)"
                >
                  {{ getReasoningEffort(record) }}
                </Badge>
                <Badge
                  v-if="getFastBadge(record)"
                  variant="outline"
                  class="h-4 rounded-full px-1.5 text-[10px] leading-4 text-foreground flex-shrink-0"
                  :title="getFastBadgeTitle(record)"
                >
                  fast
                </Badge>
              </div>
              <span class="text-muted-foreground truncate">{{ getActualModel(record) }}</span>
            </div>
            <span
              v-else
              class="flex min-w-0 items-center gap-1"
            >
              <span class="truncate">{{ record.model }}</span>
              <Badge
                v-if="getReasoningEffort(record)"
                variant="outline"
                class="h-4 rounded-full border-primary/30 bg-primary/5 px-1.5 text-[10px] leading-4 text-primary flex-shrink-0"
                :title="getReasoningEffortTitle(record)"
              >
                {{ getReasoningEffort(record) }}
              </Badge>
              <Badge
                v-if="getFastBadge(record)"
                variant="outline"
                class="h-4 rounded-full px-1.5 text-[10px] leading-4 text-foreground flex-shrink-0"
                :title="getFastBadgeTitle(record)"
              >
                fast
              </Badge>
            </span>
          </TableCell>
          <TableCell
            v-if="isAdmin && isColumnVisible('provider')"
            class="py-4 w-[16%]"
          >
            <div class="flex min-w-0 items-center gap-1">
              <div class="flex min-w-0 flex-col text-xs gap-0.5">
                <span class="truncate">{{ record.provider }}</span>
                <span
                  v-if="record.provider_key_name"
                  class="text-muted-foreground truncate"
                  :title="record.provider_key_name"
                >
                  {{ record.provider_key_name }}
                  <span
                    v-if="record.rate_multiplier && record.rate_multiplier !== 1.0"
                    class="text-foreground/60"
                  >({{ record.rate_multiplier }}x)</span>
                </span>
              </div>
              <Shuffle
                v-if="record.has_fallback"
                data-usage-attempt-marker="fallback"
                class="w-3.5 h-3.5 text-amber-600 dark:text-amber-400 flex-shrink-0"
                title="此请求发生了 Provider 故障转移"
                aria-label="发生 Provider 故障转移"
              />
              <RefreshCcw
                v-if="record.has_retry"
                data-usage-attempt-marker="retry"
                class="w-3.5 h-3.5 text-blue-600 dark:text-blue-400 flex-shrink-0"
                title="此请求发生了重试"
                aria-label="发生重试"
              />
            </div>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('api_format')"
            class="py-4"
            :class="[isAdmin ? 'w-[15%]' : 'w-[14%]']"
            :title="getApiFormatTooltip(record)"
          >
            <!-- 有格式转换或同族格式差异：两行显示 -->
            <div
              v-if="shouldShowFormatConversion(record)"
              class="flex flex-col text-xs gap-0.5"
            >
              <div class="flex items-center gap-1 whitespace-nowrap">
                <span>{{ formatApiFormat(record.api_format!) }}</span>
                <svg
                  xmlns="http://www.w3.org/2000/svg"
                  viewBox="0 0 20 20"
                  fill="currentColor"
                  class="w-3 h-3 text-muted-foreground flex-shrink-0"
                >
                  <path
                    fill-rule="evenodd"
                    d="M3 10a.75.75 0 01.75-.75h10.638L10.23 5.29a.75.75 0 111.04-1.08l5.5 5.25a.75.75 0 010 1.08l-5.5 5.25a.75.75 0 11-1.04-1.08l4.158-3.96H3.75A.75.75 0 013 10z"
                    clip-rule="evenodd"
                  />
                </svg>
              </div>
              <span class="text-muted-foreground whitespace-nowrap">{{ formatApiFormat(record.endpoint_api_format!) }}</span>
            </div>
            <!-- 无格式转换：单行显示 -->
            <span
              v-else-if="record.api_format"
              class="text-xs whitespace-nowrap"
            >{{ formatApiFormat(record.api_format) }}</span>
            <span
              v-else
              class="text-muted-foreground text-xs"
            >-</span>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('status')"
            class="text-center py-4 w-[10%]"
          >
            <!-- 优先显示请求状态 -->
            <Badge
              v-if="isUsageRecordFailed(record)"
              variant="destructive"
              class="whitespace-nowrap"
            >
              失败
            </Badge>
            <Badge
              v-else-if="getDisplayStatus(record) === 'pending'"
              variant="outline"
              class="whitespace-nowrap animate-pulse border-muted-foreground/30 text-muted-foreground"
            >
              等待中
            </Badge>
            <Badge
              v-else-if="getDisplayStatus(record) === 'streaming'"
              variant="outline"
              class="whitespace-nowrap animate-pulse border-primary/50 text-primary"
            >
              传输中
            </Badge>
            <Badge
              v-else-if="record.status === 'cancelled'"
              variant="outline"
              class="whitespace-nowrap border-amber-500/50 text-amber-600 dark:text-amber-400"
            >
              已取消
            </Badge>
            <Badge
              v-else-if="getStreamModeSegments(record).hasConversion"
              :variant="streamBadgeVariant(getStreamModeSegments(record).client === '流式')"
              :class="(streamBadgeVariant(getStreamModeSegments(record).client === '流式') === 'secondary')
                ? 'whitespace-nowrap inline-flex items-center gap-1'
                : 'whitespace-nowrap border-border/60 text-muted-foreground inline-flex items-center gap-1'"
            >
              <span>{{ getStreamModeSegments(record).client }}</span>
              <span class="opacity-60">→</span>
              <span>{{ getStreamModeSegments(record).upstream }}</span>
            </Badge>
            <Badge
              v-else
              :variant="streamBadgeVariant(getUpstreamStream(record))"
              :class="(streamBadgeVariant(getUpstreamStream(record)) === 'secondary')
                ? 'whitespace-nowrap'
                : 'whitespace-nowrap border-border/60 text-muted-foreground'"
            >
              {{ getStreamModeLabel(record) }}
            </Badge>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('tokens')"
            class="py-4 w-[10%]"
          >
            <div class="grid w-full min-w-0 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] gap-x-1 text-xs leading-tight tabular-nums">
              <span class="justify-self-end whitespace-nowrap text-right">
                {{ formatTokens(getRecordEffectiveInputTokens(record)) }}
              </span>
              <span class="justify-self-center text-muted-foreground">
                /
              </span>
              <span class="justify-self-start whitespace-nowrap text-left">
                {{ formatTokens(record.output_tokens || 0) }}
              </span>
            </div>
            <div class="mt-0.5 grid w-full min-w-0 grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] gap-x-1 text-xs leading-tight tabular-nums text-muted-foreground">
              <span
                class="justify-self-end whitespace-nowrap text-right"
                :class="[
                  hasPositiveTokens(getRecordCacheReadTokens(record)) ? 'text-foreground/70' : ''
                ]"
              >
                {{ formatOptionalTokens(getRecordCacheReadTokens(record)) }}
              </span>
              <span class="justify-self-center">
                /
              </span>
              <span
                class="justify-self-start whitespace-nowrap text-left"
                :class="[
                  hasPositiveTokens(getRecordCacheCreationTokens(record)) ? 'text-foreground/70' : ''
                ]"
              >
                {{ formatOptionalTokens(getRecordCacheCreationTokens(record)) }}
              </span>
            </div>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('cost')"
            class="text-right py-4 w-[6%]"
          >
            <div class="flex flex-col items-end text-xs gap-0.5">
              <span class="text-primary font-medium">{{ formatCurrency(record.cost || 0) }}</span>
              <span
                v-if="showActualCost && record.actual_cost !== undefined && record.rate_multiplier && record.rate_multiplier !== 1.0"
                class="text-muted-foreground"
              >
                {{ formatCurrency(record.actual_cost) }}
              </span>
            </div>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('performance')"
            class="text-right py-4 w-[9%]"
          >
            <!-- pending/streaming 状态：首字与动态总耗时保留在同一行 -->
            <div
              v-if="getDisplayStatus(record) === 'pending' || getDisplayStatus(record) === 'streaming'"
              class="flex flex-col items-end text-xs gap-0.5"
            >
              <span class="tabular-nums whitespace-nowrap">
                <span>{{ formatRecordDurationSeconds(record.first_byte_time_ms) }}</span>
                <span class="text-muted-foreground"> / </span>
                <ElapsedTimeText
                  class="text-primary"
                  :created-at="record.created_at"
                  :response-time-updated-at="record.response_time_updated_at ?? null"
                  :status="getDisplayStatus(record)"
                  :response-time-ms="record.response_time_ms ?? null"
                />
              </span>
            </div>
            <!-- 已完成状态：首字 + 总耗时 -->
            <div
              v-else-if="record.response_time_ms != null || record.first_byte_time_ms != null"
              class="flex flex-col items-end text-xs gap-0.5"
              :title="getRecordPerformanceTitle(record)"
            >
              <span class="tabular-nums whitespace-nowrap">{{ formatRecordLatencyPair(record) }}</span>
              <span class="text-muted-foreground tabular-nums whitespace-nowrap">
                {{ formatOutputRate(getRecordDisplayOutputRate(record)) }}
              </span>
            </div>
            <span
              v-else
              class="text-muted-foreground"
            >-</span>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('client_family')"
            class="py-4 w-[12%] text-xs"
            :title="formatClientFamily(record.client_family)"
          >
            <Badge
              variant="outline"
              class="w-fit max-w-full border-border/60 text-muted-foreground"
            >
              <span class="truncate">{{ formatClientFamily(record.client_family) }}</span>
            </Badge>
          </TableCell>
          <TableCell
            v-if="isColumnVisible('client_ip')"
            class="py-4 w-[10%] text-xs truncate"
            :title="record.client_ip || '-'"
          >
            {{ record.client_ip || '-' }}
          </TableCell>
          <TableCell
            v-if="isColumnVisible('user_agent')"
            class="py-4 w-[13%] text-xs truncate"
            :title="record.user_agent || '-'"
          >
            {{ formatUserAgent(record.user_agent) }}
          </TableCell>
        </TableRow>
      </TableBody>
    </Table>

    <!-- 分页控件 -->
    <template #pagination>
      <Pagination
        v-if="totalRecords > 0"
        :current="currentPage"
        :total="totalRecords"
        :page-size="pageSize"
        :page-size-options="pageSizeOptions"
        cache-key="usage-records-page-size"
        @update:current="$emit('update:currentPage', $event)"
        @update:page-size="$emit('update:pageSize', $event)"
      />
    </template>
  </TableCard>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { useDebounceFn, useLocalStorage } from '@vueuse/core'
import {
  TableCard,
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
  SortableTableHead,
  TableFilterMenu,
} from '@/components/ui'
import { EyeOff, RefreshCcw, Search, Shuffle } from 'lucide-vue-next'
import { formatTokens, formatCurrency } from '@/utils/format'
import { getCacheCreationTokens, getCacheReadTokens, getEffectiveInputTokens } from '../token-normalization'
import {
  formatOutputRate,
  formatOutputRateValue,
  getDisplayOutputRate,
  getGenerationTimeMs,
} from '../performance'
import {
  formatUsageStreamLabel,
  isUsageRecordFailed,
  isUsageUpstreamStream,
  resolveDisplayRequestStatus,
  resolveUsageStreamLabelSegments
} from '../utils/status'
import { useRowClick } from '@/composables/useRowClick'
import { useDarkMode } from '@/composables/useDarkMode'
import { API_FORMAT_ORDER, formatApiFormat } from '@/api/endpoints/types/api-format'
import { formatClientFamily } from '@/features/usage/utils/clientFamily'
import type { DateRangeParams, UsageRecord } from '../types'
import { MultiSelect, TimeRangePicker } from '@/components/common'
import type { MultiSelectOption } from '@/components/common/MultiSelect.vue'
import ElapsedTimeText from './ElapsedTimeText.vue'
import ServerUserSelector from './ServerUserSelector.vue'

export interface UserOption {
  id: string
  username: string
  email: string
}

interface FilterOption {
  value: string
  label: string
  disabled?: boolean
}

type UsageRecordColumnId =
  | 'time'
  | 'user'
  | 'key'
  | 'model'
  | 'provider'
  | 'api_format'
  | 'status'
  | 'tokens'
  | 'cost'
  | 'performance'
  | 'client_family'
  | 'client_ip'
  | 'user_agent'

interface UsageRecordColumnOption {
  id: UsageRecordColumnId
  label: string
  adminOnly?: boolean
  userOnly?: boolean
}

const props = defineProps<{
  records: UsageRecord[]
  isAdmin: boolean
  showActualCost: boolean
  loading: boolean
  // 时间范围
  timeRange: DateRangeParams
  // 筛选
  filterSearch: string
  filterUser: string
  filterModel: string
  filterProvider: string
  filterApiFormat: string
  filterStatus: string
  filterClientFamily: string
  availableUsers: UserOption[]
  availableModels: string[]
  availableProviders: string[]
  availableClientFamilies: string[]
  // 分页
  currentPage: number
  pageSize: number
  totalRecords: number
  pageSizeOptions: number[]
  // 自动刷新
  autoRefresh: boolean
  hideUnknownRecords: boolean
}>()

const emit = defineEmits<{
  'update:timeRange': [value: DateRangeParams]
  'update:filterSearch': [value: string]
  'update:filterUser': [value: string]
  'update:filterModel': [value: string]
  'update:filterProvider': [value: string]
  'update:filterApiFormat': [value: string]
  'update:filterStatus': [value: string]
  'update:filterClientFamily': [value: string]
  'update:currentPage': [value: number]
  'update:pageSize': [value: number]
  'update:autoRefresh': [value: boolean]
  'update:hideUnknownRecords': [value: boolean]
  'refresh': []
  'showDetail': [id: string]
  'prefetchDetail': [id: string]
}>()

const USAGE_RECORD_COLUMN_OPTIONS: UsageRecordColumnOption[] = [
  { id: 'time', label: '时间' },
  { id: 'user', label: '用户', adminOnly: true },
  { id: 'key', label: '密钥', userOnly: true },
  { id: 'model', label: '模型' },
  { id: 'provider', label: '提供商', adminOnly: true },
  { id: 'api_format', label: 'API格式' },
  { id: 'status', label: '类型/状态' },
  { id: 'tokens', label: 'Tokens' },
  { id: 'cost', label: '费用' },
  { id: 'performance', label: '耗时/速度' },
  { id: 'client_family', label: '客户端类型' },
  { id: 'client_ip', label: 'IP 地址' },
  { id: 'user_agent', label: 'User-Agent' },
]

const DEFAULT_ADMIN_COLUMNS: UsageRecordColumnId[] = [
  'time',
  'user',
  'model',
  'provider',
  'api_format',
  'status',
  'tokens',
  'cost',
  'performance',
]

const DEFAULT_USER_COLUMNS: UsageRecordColumnId[] = [
  'time',
  'key',
  'model',
  'api_format',
  'status',
  'tokens',
  'cost',
  'performance',
]

// 使用统一 API 格式枚举，避免使用记录筛选项和系统格式列表漂移。
const availableApiFormats = API_FORMAT_ORDER.map((value) => ({
  value,
  label: formatApiFormat(value),
}))

const adminVisibleColumnIds = useLocalStorage<UsageRecordColumnId[]>(
  'usage-records-visible-columns-admin',
  DEFAULT_ADMIN_COLUMNS,
)
const userVisibleColumnIds = useLocalStorage<UsageRecordColumnId[]>(
  'usage-records-visible-columns-user',
  DEFAULT_USER_COLUMNS,
)

const roleColumnOptions = computed(() => USAGE_RECORD_COLUMN_OPTIONS.filter((column) => {
  if (column.adminOnly && !props.isAdmin) return false
  if (column.userOnly && props.isAdmin) return false
  return true
}))

const roleColumnIds = computed(() => new Set(roleColumnOptions.value.map(column => column.id)))

function sanitizeColumnIds(
  ids: readonly string[],
  fallback: readonly UsageRecordColumnId[],
): UsageRecordColumnId[] {
  const seen = new Set<UsageRecordColumnId>()
  const sanitized = ids.filter((id): id is UsageRecordColumnId => {
    if (!roleColumnIds.value.has(id as UsageRecordColumnId)) return false
    if (seen.has(id as UsageRecordColumnId)) return false
    seen.add(id as UsageRecordColumnId)
    return true
  })
  return sanitized.length > 0 ? sanitized : [...fallback]
}

const visibleColumnIds = computed<UsageRecordColumnId[]>({
  get: () => sanitizeColumnIds(
    props.isAdmin ? adminVisibleColumnIds.value : userVisibleColumnIds.value,
    props.isAdmin ? DEFAULT_ADMIN_COLUMNS : DEFAULT_USER_COLUMNS,
  ),
  set: (value) => {
    const sanitized = sanitizeColumnIds(value, props.isAdmin ? DEFAULT_ADMIN_COLUMNS : DEFAULT_USER_COLUMNS)
    if (props.isAdmin) {
      adminVisibleColumnIds.value = sanitized
    } else {
      userVisibleColumnIds.value = sanitized
    }
  },
})

const visibleColumnSet = computed(() => new Set<UsageRecordColumnId>(visibleColumnIds.value))
const visibleColumnCount = computed(() => visibleColumnIds.value.length)
const desktopTableMinWidthClass = computed(() => {
  const metadataColumnCount = visibleColumnIds.value.filter(column => (
    column === 'client_family' ||
    column === 'client_ip' ||
    column === 'user_agent'
  )).length
  if (metadataColumnCount >= 3) return 'min-w-[1520px]'
  if (metadataColumnCount > 0) return 'min-w-[1320px]'
  return props.isAdmin ? 'min-w-[1120px]' : 'min-w-[960px]'
})

const columnSelectOptions = computed<MultiSelectOption[]>(() => roleColumnOptions.value.map(column => ({
  value: column.id,
  label: column.label,
})))

function isColumnVisible(column: UsageRecordColumnId): boolean {
  return visibleColumnSet.value.has(column)
}

const modelFilterOptions = computed<FilterOption[]>(() => [
  { value: '__all__', label: '全部模型' },
  ...props.availableModels.map((model) => ({
    value: model,
    label: model.replace('claude-', ''),
  })),
])

const providerFilterOptions = computed<FilterOption[]>(() => [
  { value: '__all__', label: '全部提供商' },
  ...props.availableProviders.map((provider) => ({
    value: provider,
    label: provider,
  })),
])

const clientFamilyFilterOptions = computed<FilterOption[]>(() => {
  const families = new Set<string>(props.availableClientFamilies)
  props.records.forEach((record) => {
    const family = record.client_family?.trim()
    if (family) families.add(family)
  })
  return [
    { value: '__all__', label: '全部客户端' },
    ...Array.from(families).sort().map((family) => ({
      value: family,
      label: formatClientFamily(family),
    })),
  ]
})

const apiFormatFilterOptions = computed<FilterOption[]>(() => [
  { value: '__all__', label: '全部格式' },
  ...availableApiFormats.map((format) => ({
    value: format.value,
    label: format.label,
  })),
])

const statusFilterOptions: FilterOption[] = [
  { value: '__all__', label: '全部状态' },
  { value: 'stream', label: '流式' },
  { value: 'standard', label: '标准' },
  { value: 'active', label: '活跃' },
  { value: 'failed', label: '失败' },
  { value: 'cancelled', label: '已取消' },
  { value: 'has_retry', label: '发生重试' },
  { value: 'has_fallback', label: '发生转移' },
]

const timeRangeModel = computed({
  get: () => props.timeRange,
  set: (value: DateRangeParams) => emit('update:timeRange', value)
})

// 通用搜索（输入防抖）
const localSearch = ref(props.filterSearch)
const emitSearchDebounced = useDebounceFn((value: string) => {
  emit('update:filterSearch', value)
}, 300)

function getDisplayStatus(record: UsageRecord) {
  return resolveDisplayRequestStatus(record)
}

function getStreamModeLabel(record: UsageRecord): string {
  return formatUsageStreamLabel(record)
}

function getStreamModeSegments(record: UsageRecord) {
  return resolveUsageStreamLabelSegments(record)
}

function getUpstreamStream(record: UsageRecord): boolean {
  return isUsageUpstreamStream(record)
}

function parseRecordDateTime(dateStr: string): Date {
  const utcDateStr = dateStr.includes('Z') || dateStr.includes('+') ? dateStr : `${dateStr}Z`
  return new Date(utcDateStr)
}

function formatRecordDate(dateStr: string): string {
  const date = parseRecordDateTime(dateStr)
  const year = String(date.getFullYear())
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${year}-${month}-${day}`
}

function formatRecordShortDate(dateStr: string): string {
  const date = parseRecordDateTime(dateStr)
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  return `${month}-${day}`
}

function formatRecordTime(dateStr: string): string {
  const date = parseRecordDateTime(dateStr)
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  const seconds = String(date.getSeconds()).padStart(2, '0')
  return `${hours}:${minutes}:${seconds}`
}

watch(() => props.filterSearch, (value) => {
  if (value !== localSearch.value) {
    localSearch.value = value
  }
})

watch(localSearch, (value) => {
  emitSearchDebounced(value)
})

// 使用复用的行点击逻辑
const { handleMouseDown, shouldTriggerRowClick } = useRowClick()
const { isDark } = useDarkMode()

// 暗色模式下交换"流式"与"标准"徽章的填充/描边样式
function streamBadgeVariant(isStream: boolean): 'secondary' | 'outline' {
  if (isDark.value) {
    return isStream ? 'outline' : 'secondary'
  }
  return isStream ? 'secondary' : 'outline'
}

function handleRowMouseDown(event: MouseEvent, id: string) {
  handleMouseDown(event)
  if (!props.isAdmin) return
  if (event.button !== 0) return
  emit('prefetchDetail', id)
}

// 处理行点击，排除文本选择操作
function handleRowClick(event: MouseEvent, id: string) {
  if (!props.isAdmin) return
  if (!shouldTriggerRowClick(event)) return
  emit('showDetail', id)
}

function getRecordEffectiveInputTokens(record: UsageRecord): number {
  return getEffectiveInputTokens(record)
}

function getRecordCacheReadTokens(record: UsageRecord): number {
  return getCacheReadTokens(record)
}

function getRecordCacheCreationTokens(record: UsageRecord): number {
  return getCacheCreationTokens(record)
}

function hasPositiveTokens(value: number | null | undefined): boolean {
  return typeof value === 'number' && Number.isFinite(value) && value > 0
}

function formatOptionalTokens(value: number | null | undefined): string {
  return hasPositiveTokens(value) ? formatTokens(value) : '-'
}

function hasRecordCacheTokens(record: UsageRecord): boolean {
  return hasPositiveTokens(getRecordCacheReadTokens(record)) || hasPositiveTokens(getRecordCacheCreationTokens(record))
}

function getRecordCacheTokensTitle(record: UsageRecord): string {
  return [
    `缓存读取: ${formatOptionalTokens(getRecordCacheReadTokens(record))}`,
    `缓存写入: ${formatOptionalTokens(getRecordCacheCreationTokens(record))}`,
  ].join('\n')
}

function formatRecordLatencyPair(record: UsageRecord): string {
  const firstByte = formatRecordDurationSeconds(record.first_byte_time_ms)
  const total = formatRecordDurationSeconds(record.response_time_ms)
  return `${firstByte} / ${total}`
}

function formatRecordDurationSeconds(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return '-'
  return `${(ms / 1000).toFixed(2)}s`
}

function getRecordDisplayOutputRate(record: UsageRecord): number | null {
  return getDisplayOutputRate({
    output_tokens: record.output_tokens,
    response_time_ms: record.response_time_ms,
    first_byte_time_ms: record.first_byte_time_ms,
    is_stream: record.is_stream,
    upstream_is_stream: record.upstream_is_stream,
  })
}

function getRecordPerformanceTitle(record: UsageRecord): string {
  const outputRate = getRecordDisplayOutputRate(record)
  return [
    `首字: ${formatRecordDurationSeconds(record.first_byte_time_ms)}`,
    `总耗时: ${formatRecordDurationSeconds(record.response_time_ms)}`,
    `生成耗时: ${formatRecordDurationSeconds(getGenerationTimeMs(record))}`,
    `输出速度: ${formatOutputRateTokensPerSecond(outputRate)}`,
  ].join('\n')
}

function formatOutputRateTokensPerSecond(outputRate: number | null | undefined): string {
  const value = formatOutputRateValue(outputRate)
  if (value === '-') return value
  return `${value} tokens/s`
}

function formatUserAgent(value: string | null | undefined): string {
  const userAgent = value?.trim()
  if (!userAgent) return '-'
  return userAgent.length > 48 ? `${userAgent.slice(0, 45)}...` : userAgent
}

// useDebounceFn 自动处理清理，无需 onUnmounted

// 判断是否应该显示格式转换信息
// 包括：1. 跨格式转换（has_format_conversion=true）2. 同族格式差异
function shouldShowFormatConversion(record: UsageRecord): boolean {
  if (!record.api_format || !record.endpoint_api_format) {
    return false
  }
  // 跨格式转换
  if (record.has_format_conversion) {
    return true
  }
  // 同族格式差异（精确字符串比较，不区分大小写）
  return record.api_format.trim().toLowerCase() !== record.endpoint_api_format.trim().toLowerCase()
}

// 获取 API 格式的 tooltip（包含转换信息）
function getApiFormatTooltip(record: UsageRecord): string {
  if (!record.api_format) {
    return ''
  }
  const displayFormat = formatApiFormat(record.api_format)

  // 如果发生了格式转换或同族格式差异，显示详细信息
  if (shouldShowFormatConversion(record)) {
    const endpointApiFormat = record.endpoint_api_format ?? record.api_format
    const endpointDisplayFormat = formatApiFormat(endpointApiFormat)
    const conversionType = record.has_format_conversion ? '格式转换' : '格式兼容（无需转换）'
    return `用户请求格式: ${displayFormat}\n端点原生格式: ${endpointDisplayFormat}\n${conversionType}`
  }

  return record.api_format
}

// 获取实际使用的模型（优先 target_model，其次列表接口下发的 model_version）
// 只有当实际模型与请求模型不同时才返回，用于显示映射箭头
function getActualModel(record: UsageRecord): string | null {
  // 优先显示模型映射
  if (record.target_model && record.target_model !== record.model) {
    return record.target_model
  }
  // 其次显示 Provider 返回的实际版本（如 Gemini 的 modelVersion）
  if (record.model_version && record.model_version !== record.model) {
    return record.model_version
  }
  return null
}

function getReasoningEffort(record: UsageRecord): string | null {
  const effort = record.reasoning_effort?.trim()
  return effort || null
}

function getReasoningEffortTitle(record: UsageRecord): string {
  const effort = getReasoningEffort(record)
  return effort ? `Reasoning: ${effort}` : ''
}

function getServiceTier(record: UsageRecord): string | null {
  const serviceTier = record.service_tier?.trim().toLowerCase()
  return serviceTier || null
}

function getFastBadge(record: UsageRecord): boolean {
  return getServiceTier(record) === 'priority'
}

function getFastBadgeTitle(record: UsageRecord): string {
  const serviceTier = getServiceTier(record)
  return serviceTier ? `Service tier: ${serviceTier}` : ''
}

// 获取模型列的 tooltip
function getModelTooltip(record: UsageRecord): string {
  const actualModel = getActualModel(record)
  const reasoningEffort = getReasoningEffort(record)
  const fastSuffix = getFastBadge(record) ? '\nService tier: priority' : ''
  const suffix = `${reasoningEffort ? `\nReasoning: ${reasoningEffort}` : ''}${fastSuffix}`
  if (actualModel) {
    return `${record.model} -> ${actualModel}${suffix}`
  }
  return `${record.model}${suffix}`
}
</script>
