<template>
  <TableCard title="使用记录">
    <template #actions>
      <!-- 时间范围筛选 -->
      <TimeRangePicker
        v-model="timeRangeModel"
        :show-granularity="false"
      />

      <!-- 分隔线 -->
      <div class="hidden sm:block h-4 w-px bg-border" />

      <!-- 通用搜索 -->
      <div class="relative">
        <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
        <Input
          id="usage-records-search"
          v-model="localSearch"
          :placeholder="isAdmin ? '搜索用户/密钥' : '搜索密钥/模型'"
          class="w-[7.5rem] sm:w-48 h-8 text-xs border-border/60 pl-8"
        />
      </div>

      <!-- 用户筛选（仅管理员可见） -->
      <Select
        v-if="isAdmin && availableUsers.length > 0"
        :model-value="filterUser"
        @update:model-value="$emit('update:filterUser', $event)"
      >
        <SelectTrigger class="flex-1 min-w-0 sm:flex-none sm:w-36 h-8 text-xs border-border/60">
          <SelectValue placeholder="用户" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__all__">
            全部用户
          </SelectItem>
          <SelectItem
            v-for="user in availableUsers"
            :key="user.id"
            :value="user.id"
          >
            {{ user.username || user.email }}
          </SelectItem>
        </SelectContent>
      </Select>

      <!-- 模型筛选 -->
      <Select
        :model-value="filterModel"
        @update:model-value="$emit('update:filterModel', $event)"
      >
        <SelectTrigger class="flex-1 min-w-0 sm:flex-none sm:w-40 h-8 text-xs border-border/60">
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
        <SelectTrigger class="flex-1 min-w-0 sm:flex-none sm:w-32 h-8 text-xs border-border/60">
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
        <SelectTrigger class="flex-1 min-w-0 sm:flex-none sm:w-32 h-8 text-xs border-border/60">
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
        <SelectTrigger class="flex-1 min-w-0 sm:flex-none sm:w-28 h-8 text-xs border-border/60">
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

      <!-- 分隔线 -->
      <div class="hidden sm:block h-4 w-px bg-border" />

      <!-- 自动刷新按钮 -->
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
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
        class="border-b border-border/40 py-2.5 px-2"
        :class="isAdmin ? 'cursor-pointer active:bg-muted/30 transition-colors' : ''"
        @click="isAdmin && emit('showDetail', record.id)"
      >
        <!-- 第一行：模型 + 费用 -->
        <div class="flex items-center justify-between gap-2">
          <div class="min-w-0 flex-1">
            <span class="text-sm font-medium truncate block">{{ record.model }}</span>
            <span
              v-if="getActualModel(record)"
              class="text-[11px] text-muted-foreground truncate block"
            >-> {{ getActualModel(record) }}</span>
          </div>
          <div class="flex flex-col items-end flex-shrink-0">
            <span class="text-xs text-primary font-medium">{{ formatCurrency(record.cost || 0) }}</span>
            <span
              v-if="showActualCost && record.actual_cost !== undefined && record.rate_multiplier && record.rate_multiplier !== 1.0"
              class="text-[10px] text-muted-foreground"
            >{{ formatCurrency(record.actual_cost) }}</span>
          </div>
        </div>

        <!-- 第二行：状态 | 时间 | API格式 | 耗时 | Tokens -->
        <div class="flex items-center justify-between text-[11px] text-muted-foreground mt-1 leading-4">
          <div class="flex items-center gap-1.5">
            <!-- 状态 Badge -->
            <Badge
              v-if="isUsageRecordFailed(record)"
              variant="destructive"
              class="whitespace-nowrap text-[10px] px-1.5 h-4 leading-4 inline-flex items-center"
            >
              失败
            </Badge>
            <Badge
              v-else-if="getDisplayStatus(record) === 'pending'"
              variant="outline"
              class="whitespace-nowrap animate-pulse border-muted-foreground/30 text-muted-foreground text-[10px] px-1.5 h-4 leading-4 inline-flex items-center"
            >
              等待
            </Badge>
            <Badge
              v-else-if="getDisplayStatus(record) === 'streaming'"
              variant="outline"
              class="whitespace-nowrap animate-pulse border-primary/50 text-primary text-[10px] px-1.5 h-4 leading-4 inline-flex items-center"
            >
              传输
            </Badge>
            <Badge
              v-else-if="record.status === 'cancelled'"
              variant="outline"
              class="whitespace-nowrap border-amber-500/50 text-amber-600 dark:text-amber-400 text-[10px] px-1.5 h-4 leading-4 inline-flex items-center"
            >
              取消
            </Badge>
            <Badge
              v-else-if="getStreamModeSegments(record).hasConversion"
              :variant="getStreamModeSegments(record).client === '流式' ? 'secondary' : 'outline'"
              :class="getStreamModeSegments(record).client === '流式'
                ? 'whitespace-nowrap text-[10px] px-1.5 h-4 leading-4 inline-flex items-center gap-0.5'
                : 'whitespace-nowrap border-border/60 text-muted-foreground text-[10px] px-1.5 h-4 leading-4 inline-flex items-center gap-0.5'"
            >
              <span>{{ getStreamModeSegments(record).client }}</span>
              <span class="opacity-60">→</span>
              <span>{{ getStreamModeSegments(record).upstream }}</span>
            </Badge>
            <Badge
              v-else
              :variant="getUpstreamStream(record) ? 'secondary' : 'outline'"
              :class="getUpstreamStream(record)
                ? 'whitespace-nowrap text-[10px] px-1.5 h-4 leading-4 inline-flex items-center'
                : 'whitespace-nowrap border-border/60 text-muted-foreground text-[10px] px-1.5 h-4 leading-4 inline-flex items-center'"
            >
              {{ getStreamModeLabel(record) }}
            </Badge>
            <span class="text-muted-foreground/50">|</span>
            <span>{{ formatDateTime(record.created_at) }}</span>
            <template v-if="record.api_format">
              <span class="text-muted-foreground/50">|</span>
              <span>{{ formatApiFormat(record.api_format) }}</span>
            </template>
          </div>
          <div class="flex items-center gap-1.5">
            <!-- 耗时 -->
            <span
              v-if="getDisplayStatus(record) === 'pending' || getDisplayStatus(record) === 'streaming'"
              class="text-primary tabular-nums"
            ><ElapsedTimeText
              :created-at="record.created_at"
              :status="getDisplayStatus(record)"
              :response-time-ms="record.response_time_ms ?? null"
            /></span>
            <span
              v-else-if="record.response_time_ms != null || record.first_byte_time_ms != null"
              class="tabular-nums"
            >{{ record.first_byte_time_ms != null ? (record.first_byte_time_ms / 1000).toFixed(1) + '/' : '' }}{{ record.response_time_ms != null ? (record.response_time_ms / 1000).toFixed(1) : '-' }}{{ record.response_time_ms != null ? 's' : '' }}</span>
            <span
              v-else
              class="tabular-nums"
            >-</span>
            <span class="text-muted-foreground/50">|</span>
            <!-- Tokens -->
            <span>{{ formatTokens(getRecordEffectiveInputTokens(record)) }}/{{ formatTokens(record.output_tokens || 0) }}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- 桌面端表格视图 -->
    <Table class="hidden md:table">
      <TableHeader>
        <TableRow class="border-b border-border/60 hover:bg-transparent">
          <TableHead class="h-12 font-semibold w-[70px]">
            时间
          </TableHead>
          <TableHead
            v-if="isAdmin"
            class="h-12 font-semibold w-[100px]"
          >
            用户
          </TableHead>
          <TableHead
            v-if="!isAdmin"
            class="h-12 font-semibold w-[100px]"
          >
            密钥
          </TableHead>
          <TableHead class="h-12 font-semibold w-[140px]">
            模型
          </TableHead>
          <TableHead
            v-if="isAdmin"
            class="h-12 font-semibold w-[100px]"
          >
            提供商
          </TableHead>
          <TableHead class="h-12 font-semibold w-[120px]">
            API格式
          </TableHead>
          <TableHead class="h-12 font-semibold w-[110px] text-center">
            类型
          </TableHead>
          <TableHead class="h-12 font-semibold w-[140px] text-right">
            Tokens
          </TableHead>
          <TableHead class="h-12 font-semibold w-[100px] text-right">
            费用
          </TableHead>
          <TableHead class="h-12 font-semibold w-[70px] text-right">
            <div class="flex flex-col items-end text-xs gap-0.5">
              <span>首字</span>
              <span class="text-muted-foreground font-normal">总耗时</span>
            </div>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        <TableRow v-if="records.length === 0">
          <TableCell
            :colspan="isAdmin ? 9 : 8"
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
          <TableCell class="text-xs py-4 w-[70px]">
            {{ formatDateTime(record.created_at) }}
          </TableCell>
          <TableCell
            v-if="isAdmin"
            class="py-4 w-[100px] truncate"
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
            v-if="!isAdmin"
            class="py-4 w-[100px]"
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
            class="font-medium py-4 w-[140px]"
            :title="getModelTooltip(record)"
          >
            <div
              v-if="getActualModel(record)"
              class="flex flex-col text-xs gap-0.5"
            >
              <div class="flex items-center gap-1 truncate">
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
              </div>
              <span class="text-muted-foreground truncate">{{ getActualModel(record) }}</span>
            </div>
            <span
              v-else
              class="truncate block"
            >{{ record.model }}</span>
          </TableCell>
          <TableCell
            v-if="isAdmin"
            class="py-4 w-[60px]"
          >
            <div class="flex items-center gap-1">
              <div class="flex flex-col text-xs gap-0.5">
                <span>{{ record.provider }}</span>
                <span
                  v-if="record.provider_key_name || record.api_key_name"
                  class="text-muted-foreground truncate"
                  :title="record.provider_key_name || record.api_key_name"
                >
                  {{ record.provider_key_name || record.api_key_name }}
                  <span
                    v-if="record.rate_multiplier && record.rate_multiplier !== 1.0"
                    class="text-foreground/60"
                  >({{ record.rate_multiplier }}x)</span>
                </span>
              </div>
              <!-- 故障转移图标（优先显示） -->
              <svg
                v-if="record.has_fallback"
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="w-3.5 h-3.5 text-amber-600 dark:text-amber-400 flex-shrink-0"
                title="此请求发生了 Provider 故障转移"
              >
                <path d="m16 3 4 4-4 4" />
                <path d="M20 7H4" />
                <path d="m8 21-4-4 4-4" />
                <path d="M4 17h16" />
              </svg>
              <!-- 重试图标（仅在无故障转移时显示） -->
              <svg
                v-else-if="record.has_retry"
                xmlns="http://www.w3.org/2000/svg"
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                stroke-width="2"
                stroke-linecap="round"
                stroke-linejoin="round"
                class="w-3.5 h-3.5 text-blue-600 dark:text-blue-400 flex-shrink-0"
                title="此请求发生了亲和缓存重试"
              >
                <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
                <path d="M21 21v-5h-5" />
                <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
                <path d="M3 3v5h5" />
              </svg>
            </div>
          </TableCell>
          <TableCell
            class="py-4 w-[120px]"
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
          <TableCell class="text-center py-4 w-[70px]">
            <!-- 优先显示请求状态 -->
            <Badge
              v-if="getDisplayStatus(record) === 'pending'"
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
              v-else-if="isUsageRecordFailed(record)"
              variant="destructive"
              class="whitespace-nowrap"
            >
              失败
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
              :variant="getStreamModeSegments(record).client === '流式' ? 'secondary' : 'outline'"
              :class="getStreamModeSegments(record).client === '流式'
                ? 'whitespace-nowrap inline-flex items-center gap-1'
                : 'whitespace-nowrap border-border/60 text-muted-foreground inline-flex items-center gap-1'"
            >
              <span>{{ getStreamModeSegments(record).client }}</span>
              <span class="opacity-60">→</span>
              <span>{{ getStreamModeSegments(record).upstream }}</span>
            </Badge>
            <Badge
              v-else
              :variant="getUpstreamStream(record) ? 'secondary' : 'outline'"
              :class="getUpstreamStream(record)
                ? 'whitespace-nowrap'
                : 'whitespace-nowrap border-border/60 text-muted-foreground'"
            >
              {{ getStreamModeLabel(record) }}
            </Badge>
          </TableCell>
          <TableCell class="text-right py-4 w-[140px]">
            <div class="flex flex-col items-end text-xs gap-0.5">
              <div class="flex items-center gap-1">
                <span>{{ formatTokens(getRecordEffectiveInputTokens(record)) }}</span>
                <span class="text-muted-foreground">/</span>
                <span>{{ formatTokens(record.output_tokens || 0) }}</span>
              </div>
              <div class="flex items-center gap-1 text-muted-foreground">
                <span :class="hasPositiveTokens(getRecordCacheReadTokens(record)) ? 'text-foreground/70' : ''">{{ formatOptionalTokens(getRecordCacheReadTokens(record)) }}</span>
                <span>/</span>
                <span :class="hasPositiveTokens(getRecordCacheCreationTokens(record)) ? 'text-foreground/70' : ''">{{ formatOptionalTokens(getRecordCacheCreationTokens(record)) }}</span>
              </div>
            </div>
          </TableCell>
          <TableCell class="text-right py-4 w-[100px]">
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
          <TableCell class="text-right py-4 w-[70px]">
            <!-- pending 状态：只显示增长的总时间 -->
            <div
              v-if="getDisplayStatus(record) === 'pending'"
              class="flex flex-col items-end text-xs gap-0.5"
            >
              <span class="text-muted-foreground">-</span>
              <span class="text-primary tabular-nums"><ElapsedTimeText
                :created-at="record.created_at"
                :status="getDisplayStatus(record)"
                :response-time-ms="record.response_time_ms ?? null"
              /></span>
            </div>
            <!-- streaming 状态：首字固定 + 总时间增长 -->
            <div
              v-else-if="getDisplayStatus(record) === 'streaming'"
              class="flex flex-col items-end text-xs gap-0.5"
            >
              <span
                v-if="record.first_byte_time_ms != null"
                class="tabular-nums"
              >{{ (record.first_byte_time_ms / 1000).toFixed(2) }}s</span>
              <span
                v-else
                class="text-muted-foreground"
              >-</span>
              <span class="text-primary tabular-nums"><ElapsedTimeText
                :created-at="record.created_at"
                :status="getDisplayStatus(record)"
                :response-time-ms="record.response_time_ms ?? null"
              /></span>
            </div>
            <!-- 已完成状态：首字 + 总耗时 -->
            <div
              v-else-if="record.response_time_ms != null || record.first_byte_time_ms != null"
              class="flex flex-col items-end text-xs gap-0.5"
            >
              <span
                v-if="record.first_byte_time_ms != null"
                class="tabular-nums"
              >{{ (record.first_byte_time_ms / 1000).toFixed(2) }}s</span>
              <span
                v-else
                class="text-muted-foreground"
              >-</span>
              <span
                v-if="record.response_time_ms != null"
                class="text-muted-foreground tabular-nums"
              >{{ (record.response_time_ms / 1000).toFixed(2) }}s</span>
              <span
                v-else
                class="text-muted-foreground"
              >-</span>
            </div>
            <span
              v-else
              class="text-muted-foreground"
            >-</span>
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
import { useDebounceFn } from '@vueuse/core'
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
} from '@/components/ui'
import { RefreshCcw, Search } from 'lucide-vue-next'
import { formatTokens, formatCurrency } from '@/utils/format'
import { formatDateTime } from '../composables'
import { getCacheCreationTokens, getCacheReadTokens, getEffectiveInputTokens } from '../token-normalization'
import {
  formatUsageStreamLabel,
  isUsageRecordFailed,
  isUsageUpstreamStream,
  resolveDisplayRequestStatus,
  resolveUsageStreamLabelSegments
} from '../utils/status'
import { useRowClick } from '@/composables/useRowClick'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import type { DateRangeParams, UsageRecord } from '../types'
import { TimeRangePicker } from '@/components/common'
import ElapsedTimeText from './ElapsedTimeText.vue'

export interface UserOption {
  id: string
  username: string
  email: string
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
  availableUsers: UserOption[]
  availableModels: string[]
  availableProviders: string[]
  // 分页
  currentPage: number
  pageSize: number
  totalRecords: number
  pageSizeOptions: number[]
  // 自动刷新
  autoRefresh: boolean
}>()

const emit = defineEmits<{
  'update:timeRange': [value: DateRangeParams]
  'update:filterSearch': [value: string]
  'update:filterUser': [value: string]
  'update:filterModel': [value: string]
  'update:filterProvider': [value: string]
  'update:filterApiFormat': [value: string]
  'update:filterStatus': [value: string]
  'update:currentPage': [value: number]
  'update:pageSize': [value: number]
  'update:autoRefresh': [value: boolean]
  'refresh': []
  'showDetail': [id: string]
  'prefetchDetail': [id: string]
}>()

// 静态常量（放在 defineProps/defineEmits 之后）
const AVAILABLE_API_FORMATS = [
  { value: 'openai:chat', label: 'OpenAI Chat' },
  { value: 'openai:cli', label: 'OpenAI CLI' },
  { value: 'openai:compact', label: 'OpenAI Compact' },
  { value: 'openai:video', label: 'OpenAI Video' },
  { value: 'claude:chat', label: 'Claude Chat' },
  { value: 'claude:cli', label: 'Claude CLI' },
  { value: 'gemini:chat', label: 'Gemini Chat' },
  { value: 'gemini:cli', label: 'Gemini CLI' },
  { value: 'gemini:video', label: 'Gemini Video' },
] as const

// 使用模块级常量
const availableApiFormats = AVAILABLE_API_FORMATS

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

// useDebounceFn 自动处理清理，无需 onUnmounted

// 判断是否应该显示格式转换信息
// 包括：1. 跨格式转换（has_format_conversion=true）2. 同族格式差异（如 CLAUDE_CLI → CLAUDE）
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

// 获取模型列的 tooltip
function getModelTooltip(record: UsageRecord): string {
  const actualModel = getActualModel(record)
  if (actualModel) {
    return `${record.model} -> ${actualModel}`
  }
  return record.model
}
</script>
