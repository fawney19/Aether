<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <!-- 页面头部：统计卡片与系统公告 -->
    <div class="grid min-w-0 grid-cols-1 gap-4 min-[1440px]:grid-cols-[minmax(0,1fr)_240px] 2xl:grid-cols-[minmax(0,1fr)_260px]">
      <div class="min-w-0 flex flex-col">
        <Badge
          :variant="authStore.isAdmin ? 'default' : 'secondary'"
          class="mb-4 self-start uppercase tracking-[0.45em]"
        >
          {{ dashboardModeLabel }}
        </Badge>

        <div
          v-if="dashboardError && !isAdmin"
          role="alert"
          class="mb-4 flex items-center justify-between gap-3 text-sm text-destructive"
        >
          <span>{{ dashboardError }}</span>
          <Button
            variant="outline"
            size="sm"
            :disabled="loading"
            @click="loadDashboardData"
          >
            {{ getI18nLocale() === 'en-US' ? 'Retry' : '重试' }}
          </Button>
        </div>

        <!-- 主要统计卡片 -->
        <div
          class="grid gap-3 sm:gap-4"
          :class="isAdmin
            ? 'grid-cols-1 min-[480px]:grid-cols-2 xl:grid-cols-5'
            : 'grid-cols-2 xl:grid-cols-4'"
        >
          <!-- 加载中骨架屏 -->
          <template v-if="loading">
            <Card
              v-for="i in statSkeletonCount"
              :key="'skeleton-' + i"
              class="p-5"
            >
              <Skeleton class="h-4 w-20 mb-4" />
              <Skeleton class="h-8 w-32 mb-2" />
              <Skeleton class="h-4 w-16" />
            </Card>
          </template>
          <!-- 有数据时显示统计卡片 -->
          <template v-else-if="stats.length > 0">
            <Card
              v-for="(stat, index) in stats"
              :key="stat.name"
              class="relative overflow-hidden p-3"
              :class="[statCardBorders[index % statCardBorders.length], isAdmin ? 'sm:p-4' : 'sm:p-5'].join(' ')"
            >
              <div
                class="pointer-events-none absolute -right-4 -top-6 h-28 w-28 rounded-full blur-3xl opacity-40"
                :class="statCardGlows[index % statCardGlows.length]"
              />
              <!-- 图标固定在右上角 -->
              <div
                v-if="!isAdmin"
                class="absolute top-3 right-3 sm:top-5 sm:right-5 rounded-xl sm:rounded-2xl border border-border bg-card/50 p-2 sm:p-3 shadow-inner backdrop-blur-sm"
                :class="getStatIconColor(index)"
              >
                <component
                  :is="stat.icon"
                  class="h-4 w-4 sm:h-5 sm:w-5"
                />
              </div>
              <!-- 内容区域 -->
              <div :class="isAdmin ? 'grid min-w-0 grid-rows-[20px_36px_auto] gap-y-1' : ''">
                <p
                  class="text-xs font-semibold tracking-normal text-muted-foreground"
                  :class="isAdmin ? 'whitespace-nowrap leading-5' : 'min-h-10 pr-10 sm:pr-14 leading-snug break-words'"
                  :title="isAdmin ? statisticsScope : undefined"
                >
                  {{ stat.name }}
                </p>
                <div
                  v-if="isAdmin"
                  class="flex h-9 min-w-0 items-center"
                >
                  <MetricValue
                    :value="stat.value"
                    :max-font-size="30"
                    :title="stat.valueHint || stat.value"
                    class="w-full font-semibold leading-none tabular-nums text-foreground"
                  />
                </div>
                <p
                  v-else
                  :title="stat.valueHint"
                  class="mt-2 sm:mt-4 text-xl sm:text-3xl font-semibold tabular-nums text-foreground"
                >
                  {{ stat.value }}
                </p>
                <p
                  v-if="stat.subValue"
                  :title="stat.totalHint || (isAdmin ? statisticsScope : undefined)"
                  class="break-words text-[10px] sm:text-sm text-muted-foreground"
                  :class="isAdmin ? 'leading-5' : 'mt-0.5 sm:mt-1'"
                >
                  <span>{{ stat.subValue }}</span>
                  <span
                    v-if="stat.userChange"
                    class="ml-2 inline-flex gap-2 whitespace-nowrap tabular-nums"
                  >
                    <span
                      class="text-red-600 dark:text-red-400"
                      :title="t('今日新增', 'Added today')"
                    >+{{ metricCount(stat.userChange.added) }}</span>
                    <span
                      class="text-green-600 dark:text-green-400"
                      :title="t('今日减少', 'Removed today')"
                    >-{{ metricCount(stat.userChange.removed) }}</span>
                  </span>
                </p>
                <div
                  v-if="!isAdmin && (stat.change || stat.extraBadge)"
                  class="mt-1.5 sm:mt-2 flex items-center gap-1 sm:gap-1.5 flex-wrap"
                >
                  <Badge
                    v-if="stat.change"
                    variant="secondary"
                    class="text-[9px] sm:text-xs"
                  >
                    {{ stat.change }}
                  </Badge>
                  <Badge
                    v-if="stat.extraBadge"
                    variant="secondary"
                    class="text-[9px] sm:text-xs"
                  >
                    {{ stat.extraBadge }}
                  </Badge>
                </div>
              </div>
            </Card>
          </template>
          <!-- 无数据时显示占位卡片 -->
          <template v-else>
            <Card
              v-for="(placeholder, index) in emptyStatPlaceholders"
              :key="'empty-' + index"
              class="relative overflow-hidden p-3"
              :class="[statCardBorders[index % statCardBorders.length], isAdmin ? 'sm:p-4' : 'sm:p-5'].join(' ')"
            >
              <div
                class="pointer-events-none absolute -right-4 -top-6 h-28 w-28 rounded-full blur-3xl opacity-20"
                :class="statCardGlows[index % statCardGlows.length]"
              />
              <div
                v-if="!isAdmin"
                class="absolute top-3 right-3 sm:top-5 sm:right-5 rounded-xl sm:rounded-2xl border border-border bg-card/50 p-2 sm:p-3 shadow-inner backdrop-blur-sm"
                :class="getStatIconColor(index)"
              >
                <component
                  :is="placeholder.icon"
                  class="h-4 w-4 sm:h-5 sm:w-5"
                />
              </div>
              <div :class="isAdmin ? 'grid min-w-0 grid-rows-[20px_36px_auto] gap-y-1' : ''">
                <p
                  class="text-xs font-semibold tracking-normal text-muted-foreground"
                  :class="isAdmin ? 'whitespace-nowrap leading-5' : 'min-h-10 pr-10 sm:pr-14 leading-snug break-words'"
                  :title="isAdmin ? statisticsScope : undefined"
                >
                  {{ placeholder.name }}
                </p>
                <p
                  class="font-semibold text-muted-foreground/50"
                  :class="isAdmin ? 'flex h-9 items-center text-3xl leading-none' : 'mt-2 sm:mt-4 text-xl sm:text-3xl'"
                >
                  {{ isAdmin ? '—' : '--' }}
                </p>
                <p
                  class="text-[10px] sm:text-sm text-muted-foreground/50"
                  :class="isAdmin ? 'leading-5' : 'mt-0.5 sm:mt-1'"
                >
                  暂无数据
                </p>
              </div>
            </Card>
          </template>
        </div>

        <!-- 管理员：全站今日性能 -->
        <div
          v-if="isAdmin"
          class="mt-6"
          :aria-busy="loading"
        >
          <div class="mb-3 flex items-center justify-between gap-2">
            <h3 class="text-sm font-medium text-foreground">
              {{ t('请求概况', 'Request overview') }}
            </h3>
            <Badge
              variant="outline"
              class="shrink-0 text-[10px]"
            >
              {{ t('实时', 'Live') }}
            </Badge>
          </div>
          <div class="grid grid-cols-2 gap-2 sm:grid-cols-3 sm:gap-3 xl:grid-cols-6">
            <Card
              v-for="(metric, index) in performanceCards"
              :key="metric.key"
              :data-request-metric="metric.key"
              :title="metric.tooltip"
              :tabindex="metric.tooltip ? 0 : undefined"
              class="relative min-w-0 p-3 sm:p-4"
              :class="statCardBorders[index % statCardBorders.length]"
            >
              <p class="text-xs font-semibold break-words text-muted-foreground">
                {{ metric.label }}
              </p>
              <Skeleton
                v-if="loading"
                class="mt-2 h-7 w-16"
              />
              <MetricValue
                v-else
                :value="metric.value"
                class="mt-2 font-semibold tabular-nums"
              />
            </Card>
          </div>
        </div>

        <!-- 普通用户：月度统计 -->
        <div
          v-else-if="
            !isAdmin &&
              (hasCacheData || (userMonthlyCost !== null && userMonthlyCost > 0))
          "
          class="mt-6"
        >
          <div class="mb-3 flex items-center justify-between">
            <h3 class="text-sm font-medium text-foreground">
              本月统计
            </h3>
            <Badge
              variant="outline"
              class="uppercase tracking-[0.3em] text-[10px]"
            >
              Monthly
            </Badge>
          </div>
          <div class="grid grid-cols-2 gap-2 sm:gap-3 xl:grid-cols-4">
            <Card
              v-if="cacheStats"
              class="relative p-3 sm:p-4 border-book-cloth/30"
            >
              <Database
                class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground"
              />
              <div class="pr-6">
                <p
                  class="text-xs font-semibold tracking-normal break-words text-muted-foreground"
                >
                  缓存命中率
                </p>
                <p
                  class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground"
                >
                  {{ cacheStats.cache_hit_rate || 0 }}%
                </p>
              </div>
            </Card>
            <Card
              v-if="cacheStats"
              class="relative p-3 sm:p-4 border-kraft/30"
            >
              <Hash
                class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground"
              />
              <div class="pr-6">
                <p
                  class="text-xs font-semibold tracking-normal break-words text-muted-foreground"
                >
                  缓存读取
                </p>
                <p
                  class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground"
                >
                  {{ formatTokens(cacheStats.cache_read_tokens) }}
                </p>
              </div>
            </Card>
            <Card
              v-if="cacheStats"
              class="relative p-3 sm:p-4 border-book-cloth/25"
            >
              <Database
                class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground"
              />
              <div class="pr-6">
                <p
                  class="text-xs font-semibold tracking-normal break-words text-muted-foreground"
                >
                  缓存创建
                </p>
                <p
                  class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground"
                >
                  {{ formatTokens(cacheStats.cache_creation_tokens) }}
                </p>
              </div>
            </Card>
            <Card
              v-if="userMonthlyCost !== null"
              class="relative p-3 sm:p-4 border-manilla/40"
            >
              <DollarSign
                class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground"
              />
              <div class="pr-6">
                <p
                  class="text-xs font-semibold tracking-normal break-words text-muted-foreground"
                >
                  本月费用
                </p>
                <p
                  class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground"
                >
                  {{ formatCurrency(userMonthlyCost) }}
                </p>
              </div>
            </Card>
          </div>
        </div>
      </div>
      <div class="min-w-0 min-[1440px]:relative">
        <DashboardAnnouncements class="min-[1440px]:absolute min-[1440px]:inset-0" />
      </div>
    </div>

    <DashboardActivity
      v-if="isAdmin"
      :data="activityHeatmap"
      :consecutive-active-days="dashboardSnapshot?.consecutive_active_days ?? null"
      :active-days="dashboardSnapshot?.active_days ?? null"
      :scope-hint="activityScope"
      :loading="loading"
      :error="false"
      :timeline-data="isDemo ? demoTimeline : undefined"
    />

    <!-- 趋势图表筛选 -->
    <div class="flex flex-wrap items-center justify-between gap-3">
      <h3
        class="text-xs font-semibold uppercase tracking-wider text-muted-foreground"
      >
        统计周期
      </h3>
      <TimeRangePicker
        v-model="dailyTimeRange"
        :allow-hourly="!isAdmin"
        :show-granularity="!isAdmin"
      />
    </div>

    <div
      v-if="dailyError"
      role="alert"
      class="flex items-center justify-between gap-3 rounded-lg border border-border p-4 text-sm text-muted-foreground"
    >
      <span>{{ dailyError }}</span>
      <Button
        variant="outline"
        size="sm"
        @click="loadDailyStats"
      >
        {{ t('重试', 'Retry') }}
      </Button>
    </div>

    <!-- 趋势图表区域 -->
    <div
      v-if="!dailyError"
      class="grid grid-cols-1 gap-6 lg:grid-cols-2"
    >
      <!-- 每日使用趋势（折线图）- 普通用户可见 -->
      <Card
        v-if="!isAdmin"
        class="p-5"
      >
        <h4
          class="mb-3 text-xs font-semibold text-foreground uppercase tracking-wider"
        >
          每日使用趋势
        </h4>
        <div
          v-if="loadingDaily"
          class="flex items-center justify-center h-[280px]"
        >
          <Skeleton class="h-full w-full" />
        </div>
        <div
          v-else
          style="height: 280px"
        >
          <LineChart
            v-if="
              dailyUsageTrendChartData.labels &&
                dailyUsageTrendChartData.labels.length > 0
            "
            :data="dailyUsageTrendChartData"
            :options="dailyUsageTrendChartOptions"
          />
          <div
            v-else
            class="flex h-full items-center justify-center text-xs text-muted-foreground"
          >
            暂无数据
          </div>
        </div>
      </Card>

      <!-- 每日模型费用（堆叠柱状图）- 仅管理员可见 -->
      <Card
        v-if="isAdmin"
        class="p-5"
      >
        <h4
          class="mb-3 text-xs font-semibold text-foreground uppercase tracking-wider"
        >
          每日模型费用
        </h4>
        <div
          v-if="loadingDaily"
          class="flex items-center justify-center h-[280px]"
        >
          <Skeleton class="h-full w-full" />
        </div>
        <div
          v-else
          style="height: 280px"
        >
          <BarChart
            v-if="
              dailyModelCostChartData.labels &&
                dailyModelCostChartData.labels.length > 0 && hasDailyModelCost
            "
            :data="dailyModelCostChartData"
            :options="dailyModelCostChartOptions"
          />
          <div
            v-else
            class="flex h-full items-center justify-center text-xs text-muted-foreground"
          >
            {{ dailyCostEmptyLabel }}
          </div>
        </div>
      </Card>

      <!-- 提供商费用分布（环形图）- 仅管理员可见 -->
      <Card
        v-if="isAdmin"
        class="p-5"
      >
        <h4
          class="mb-3 text-xs font-semibold text-foreground uppercase tracking-wider"
        >
          提供商费用分布
        </h4>
        <div
          v-if="loadingDaily"
          class="flex items-center justify-center h-[280px]"
        >
          <Skeleton class="h-full w-full" />
        </div>
        <div
          v-else
          style="height: 280px"
        >
          <DoughnutChart
            v-if="
              providerCostChartData.labels &&
                providerCostChartData.labels.length > 0
            "
            :data="providerCostChartData"
            :options="providerCostChartOptions"
          />
          <div
            v-else
            class="flex h-full items-center justify-center text-xs text-muted-foreground"
          >
            {{ dailyCostEmptyLabel }}
          </div>
        </div>
      </Card>

      <!-- 每日模型费用（堆叠柱状图）- 普通用户可见 -->
      <Card
        v-if="!isAdmin"
        class="p-5"
      >
        <h4
          class="mb-3 text-xs font-semibold text-foreground uppercase tracking-wider"
        >
          每日模型费用
        </h4>
        <div
          v-if="loadingDaily"
          class="flex items-center justify-center h-[280px]"
        >
          <Skeleton class="h-full w-full" />
        </div>
        <div
          v-else
          style="height: 280px"
        >
          <BarChart
            v-if="
              dailyModelCostChartData.labels &&
                dailyModelCostChartData.labels.length > 0
            "
            :data="dailyModelCostChartData"
            :options="dailyModelCostChartOptions"
          />
          <div
            v-else
            class="flex h-full items-center justify-center text-xs text-muted-foreground"
          >
            暂无数据
          </div>
        </div>
      </Card>
    </div>

    <p
      v-if="dailyCostsPartial && !loadingDaily"
      class="mt-3 text-xs text-amber-700 dark:text-amber-400"
    >
      {{ t('统计为已知数据小计，分布占比按已知金额计算', 'Statistics are known subtotals; distribution shares use known amounts') }}
    </p>

    <!-- 每日统计 -->
    <Card
      v-if="!dailyError"
      class="overflow-hidden mt-6"
    >
      <!-- 移动端：卡片列表 -->
      <div class="sm:hidden">
        <div class="px-4 py-3 border-b border-border/60">
          <h3 class="text-sm font-semibold">
            每日统计
          </h3>
        </div>
        <div
          v-if="loadingDaily"
          class="flex items-center justify-center py-8"
        >
          <Skeleton class="h-5 w-5 rounded-full" />
          <span class="ml-2 text-muted-foreground text-xs">加载中...</span>
        </div>
        <div
          v-else-if="displayDailyStats.length === 0"
          class="py-8 text-center text-muted-foreground text-xs"
        >
          暂无数据
        </div>
        <div
          v-else
          class="divide-y divide-border/60"
        >
          <div
            v-for="stat in displayDailyStats.slice().reverse()"
            :key="stat.date"
            class="p-4 space-y-2"
          >
            <div class="flex items-center justify-between">
              <span class="font-medium text-sm">{{
                formatDailyDate(stat.date)
              }}</span>
              <Badge
                variant="success"
                class="text-[10px]"
              >
                {{ formatDailyCost(stat.cost) }}
                <span v-if="stat.billableAmount">{{ amountStatus(stat.billableAmount, t) }}</span>
              </Badge>
            </div>
            <div class="grid grid-cols-2 gap-2 text-xs">
              <div class="flex justify-between">
                <span class="text-muted-foreground">请求</span>
                <span>{{ stat.requests.toLocaleString() }}</span>
              </div>
              <div class="flex justify-between">
                <span class="text-muted-foreground">Tokens</span>
                <span>{{ compactTokens(stat.tokens) }}</span>
              </div>
              <div class="flex justify-between">
                <span class="text-muted-foreground">响应</span>
                <span>{{ formatResponseTime(stat.avg_response_time) }}</span>
              </div>
              <div class="flex justify-between">
                <span class="text-muted-foreground">模型</span>
                <span>{{ stat.unique_models }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 桌面端：表格 -->
      <Table class="hidden sm:table">
        <TableHeader>
          <TableRow>
            <TableHead class="text-left">
              日期
            </TableHead>
            <TableHead class="text-center">
              请求次数
            </TableHead>
            <TableHead class="text-center">
              Tokens
            </TableHead>
            <TableHead class="text-center">
              费用
            </TableHead>
            <TableHead class="text-center">
              平均响应
            </TableHead>
            <TableHead class="text-center">
              使用模型
            </TableHead>
            <TableHead
              v-if="isAdmin"
              class="text-center"
            >
              使用提供商
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-if="loadingDaily">
            <TableCell
              :colspan="isAdmin ? 7 : 6"
              class="text-center py-8"
            >
              <div class="flex items-center justify-center gap-2">
                <Skeleton class="h-5 w-5 rounded-full" />
                <span class="text-muted-foreground text-xs">加载中...</span>
              </div>
            </TableCell>
          </TableRow>
          <TableRow v-else-if="displayDailyStats.length === 0">
            <TableCell
              :colspan="isAdmin ? 7 : 6"
              class="text-center py-8 text-muted-foreground text-xs"
            >
              暂无数据
            </TableCell>
          </TableRow>
          <template v-else>
            <TableRow
              v-for="stat in displayDailyStats.slice().reverse()"
              :key="stat.date"
              :data-daily-date="stat.date"
            >
              <TableCell class="font-medium text-xs">
                {{ formatDailyDate(stat.date) }}
              </TableCell>
              <TableCell class="text-center text-xs">
                {{ stat.requests.toLocaleString() }}
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="secondary"
                  class="text-[10px]"
                >
                  {{ compactTokens(stat.tokens) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="success"
                  class="text-[10px]"
                >
                  {{ formatDailyCost(stat.cost) }}
                  <span v-if="stat.billableAmount">{{ amountStatus(stat.billableAmount, t) }}</span>
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="outline"
                  class="text-[10px]"
                >
                  {{ formatResponseTime(stat.avg_response_time) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center text-xs">
                {{ stat.unique_models }}
              </TableCell>
              <TableCell
                v-if="isAdmin"
                class="text-center text-xs"
              >
                {{ stat.unique_providers ?? '—' }}
              </TableCell>
            </TableRow>
          </template>
        </TableBody>
      </Table>

      <!-- 汇总信息 -->
      <div
        v-if="displayDailyStats.length > 0 && !loadingDaily"
        class="border-t border-border bg-muted/30 backdrop-blur-sm px-4 py-3 text-xs"
        data-daily-total
      >
        <div class="grid grid-cols-2 gap-4 sm:grid-cols-4">
          <div class="text-center">
            <div class="text-muted-foreground text-[10px]">
              总请求
            </div>
            <div class="font-semibold text-foreground">
              {{ totalStats.requests.toLocaleString() }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-muted-foreground text-[10px]">
              总Tokens
            </div>
            <div class="font-semibold text-book-cloth dark:text-kraft">
              {{ compactTokens(totalStats.tokens) }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-muted-foreground text-[10px]">
              总费用
            </div>
            <div class="font-semibold text-amber-600 dark:text-amber-400">
              {{ formatDailyCost(totalStats.cost) }}
              <span v-if="adminDailyCharts">{{ amountStatus(adminDailyCharts.summary.billable_amount, t) }}</span>
            </div>
          </div>
          <div class="text-center">
            <div class="text-muted-foreground text-[10px]">
              平均响应
            </div>
            <div class="font-semibold text-book-cloth dark:text-kraft">
              {{ formatResponseTime(totalStats.avgResponseTime) }}
            </div>
          </div>
        </div>
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { getI18nLocale } from '@/i18n'
import { formatRelativeTime } from '@/utils/format'
import {
  ref,
  onMounted,
  computed,
  onBeforeUnmount,
  watch,
  markRaw,
  inject,
} from "vue";
import type { Component } from "vue";
import { routeLocationKey } from "vue-router";
import type { IntervalTimelineResponse } from "@/api/cache";
import { useAuthStore } from "@/stores/auth";
import {
  dashboardApi,
  type DashboardStat,
  type DashboardStatsResponse,
  type DailyStat,
  type ProviderSummary,
} from "@/api/dashboard";
import { overviewApi, type OverviewAmount, type OverviewDashboardCharts, type OverviewDashboardSummary, type OverviewRange } from "@/api/overview";
import { amountStatus, amountValue } from "@/features/overview/dashboard/amount";
import { chartDate, dashboardChartRange, modelDatasets, providerSlices } from "@/features/overview/dashboard/charts";
import { zonedInput } from "@/features/overview/query";
import { count, percent, timestamp } from "@/features/overview/format";
import DashboardActivity from "@/features/overview/dashboard/DashboardActivity.vue";
import DashboardAnnouncements from "@/features/overview/dashboard/DashboardAnnouncements.vue";
import MetricValue from "@/features/overview/components/MetricValue.vue";
import type { ActivityHeatmap } from "@/types/activity";
import { useOverviewI18n } from "@/features/overview/i18n";
import { getDateRangeFromPeriod } from "@/features/usage/composables";
import type { DateRangeParams } from "@/features/usage/types";
import {
  Card,
  Badge,
  Button,
  Skeleton,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "@/components/ui";
import { TimeRangePicker } from "@/components/common";
import BarChart from "@/components/charts/BarChart.vue";
import DoughnutChart from "@/components/charts/DoughnutChart.vue";
import LineChart from "@/components/charts/LineChart.vue";
import {
  Users,
  Activity,
  TrendingUp,
  DollarSign,
  Key,
  Hash,
  Zap,
  Database,
} from "lucide-vue-next";
import { formatTokens, formatCurrency } from "@/utils/format";
import { parseDateLike } from "@/utils/date";
import type {
  ChartData,
  ChartOptions,
  ChartDataset,
  TooltipItem,
} from "chart.js";

const authStore = useAuthStore();
const { t } = useOverviewI18n();

type DashboardStatCard = Omit<DashboardStat, "icon"> & {
  icon: Component;
  userChange?: { added: number; removed: number };
  valueHint?: string;
  totalHint?: string;
};


const isAdmin = computed(() => authStore.canAccessAdmin);
const route = inject(routeLocationKey, undefined);
const isDemo = computed(() => import.meta.env.DEV && isAdmin.value && route?.query.demo === '1');
const demoTimeline = ref<IntervalTimelineResponse | null>(null);
const dashboardModeLabel = computed(() => {
  if (isDemo.value) return t('演示数据', 'DEMO DATA');
  if (authStore.isAdmin) return "ADMIN MODE";
  if (authStore.isAuditAdmin) return "AUDIT MODE";
  return "PERSONAL MODE";
});

const statCardBorders = [
  "border-book-cloth/30 dark:border-book-cloth/25",
  "border-kraft/30 dark:border-kraft/25",
  "border-manilla/40 dark:border-manilla/30",
  "border-book-cloth/25 dark:border-kraft/25",
];

const statCardGlows = [
  "bg-book-cloth/30",
  "bg-kraft/30",
  "bg-manilla/35",
  "bg-kraft/30",
];

const getStatIconColor = (_index: number): string => {
  return "text-muted-foreground";
};

// 统计数据
const personalStats = ref<DashboardStatCard[]>([]);
const stats = computed(() => isAdmin.value
  ? dashboardSnapshot.value ? adminStatCards(dashboardSnapshot.value) : []
  : personalStats.value);
const todayStats = ref<{
  requests: number;
  tokens: number;
  cost: number;
  actual_cost?: number;
  cache_creation_tokens?: number;
  cache_read_tokens?: number;
}>({ requests: 0, tokens: 0, cost: 0 });

const dashboardSnapshot = ref<OverviewDashboardSummary | null>(null);
const statsSince = computed(() => timestamp(dashboardSnapshot.value?.stats_since, dashboardSnapshot.value?.timezone));
const statisticsScope = computed(() => `${t('统计自', 'Statistics since')} ${statsSince.value} · ${t('更新于', 'Updated at')} ${timestamp(dashboardSnapshot.value?.generated_at, dashboardSnapshot.value?.timezone)}`);
const activityScope = computed(() => [
  statisticsScope.value,
  t('展示近365天', 'Last 365 days shown'),
  dashboardSnapshot.value?.activity_timezone ?? dashboardSnapshot.value?.timezone,
].filter(Boolean).join(' · '));
const activityHeatmap = computed<ActivityHeatmap | null>(() => {
  const snapshot = dashboardSnapshot.value;
  if (!snapshot) return null;
  const dateParts = new Intl.DateTimeFormat('en', {
    timeZone: snapshot.activity_timezone ?? snapshot.timezone,
    year: 'numeric', month: '2-digit', day: '2-digit',
  }).formatToParts(new Date(snapshot.generated_at));
  const datePart = (type: Intl.DateTimeFormatPartTypes) => Number(dateParts.find(part => part.type === type)?.value);
  const today = Date.UTC(datePart('year'), datePart('month') - 1, datePart('day'));
  const requestsByDate = new Map(snapshot.activity_days.map(day => [day.date, day.requests]));
  const days = Array.from({ length: 365 }, (_, index) => {
    const date = new Date(today - (364 - index) * 86_400_000).toISOString().slice(0, 10);
    return { date, requests: requestsByDate.get(date) ?? 0 };
  });
  return {
    start_date: new Date(today - 364 * 86_400_000).toISOString().slice(0, 10),
    end_date: new Date(today).toISOString().slice(0, 10),
    total_days: 365,
    max_requests: days.reduce((max, day) => Math.max(max, day.requests), 0),
    days,
  };
});
const metricCount = (value: number | null | undefined) => value == null || !Number.isFinite(value) ? '—' : count(value);
const compactTokens = (value: number | null | undefined) => value == null ? '—' : formatTokens(value);
function cacheHitRate(metrics: { cache_read_tokens: number | null; cache_input_tokens: number | null } | undefined): string {
  const read = metrics?.cache_read_tokens;
  const input = metrics?.cache_input_tokens;
  return read != null && input != null && Number.isFinite(read) && Number.isFinite(input) && input > 0
    ? percent(read / input) : '—';
}
const performanceCards = computed(() => {
  const today = dashboardSnapshot.value?.today;
  const concurrency = dashboardSnapshot.value?.concurrency;
  const duration = (value: number | null | undefined) => {
    if (value == null || !Number.isFinite(value)) return '—';
    return value < 1000 ? `${count(value)}ms` : `${count(value / 1000)}s`;
  };
  const concurrencyAvailable = concurrency && concurrency.coverage !== 'unavailable';
  const concurrencyNote = concurrency?.coverage === 'complete'
    ? t('当前节点 · 今日观测', 'Current node · Observed today')
    : concurrency?.coverage === 'partial'
      ? t('当前节点 · 部分时段', 'Current node · Partial observation')
      : t('当前节点 · 暂无观测', 'Current node · No observation');
  const concurrencyTooltip = `${concurrencyNote} · ${t('有效观测时间', 'Observed interval')}: ${timestamp(concurrency?.observed_from, dashboardSnapshot.value?.timezone)} — ${timestamp(concurrency?.observed_through, dashboardSnapshot.value?.timezone)}`;
  return [
    { key: 'first-byte', label: t('平均首字', 'Avg first byte'), value: duration(today?.avg_first_byte_ms) },
    { key: 'response', label: t('平均响应', 'Avg response'), value: duration(today?.avg_response_ms) },
    { key: 'concurrency-avg', label: t('平均并发', 'Average concurrency'), value: metricCount(concurrencyAvailable ? concurrency.avg : null), tooltip: concurrencyTooltip },
    { key: 'concurrency-peak', label: t('峰值并发', 'Peak concurrency'), value: metricCount(concurrencyAvailable ? concurrency.peak : null), tooltip: concurrencyTooltip },
    { key: 'stream', label: t('流式请求', 'Streaming requests'), value: metricCount(today?.stream_requests) },
    { key: 'standard', label: t('标准请求', 'Standard requests'), value: metricCount(today?.standard_requests) },
  ];
});

const cacheStats = ref<{
  cache_creation_tokens: number;
  cache_read_tokens: number;
  cache_creation_cost?: number;
  cache_read_cost?: number;
  cache_hit_rate?: number;
  total_cache_tokens: number;
} | null>(null);

const userMonthlyCost = ref<number | null>(null);

const hasCacheData = computed(
  () => cacheStats.value && cacheStats.value.total_cache_tokens > 0,
);

const tokenBreakdown = ref<{
  input: number;
  output: number;
  cache_creation: number;
  cache_read: number;
} | null>(null);

const dailyStats = ref<DailyStat[]>([]);
const providerSummary = ref<ProviderSummary[]>([]);
const adminDailyCharts = ref<OverviewDashboardCharts | null>(null);
const adminDailyRange = ref<OverviewRange | null>(null);
type DisplayDailyStat = Omit<DailyStat, 'tokens' | 'cost' | 'avg_response_time' | 'model_breakdown' | 'unique_providers'> & {
  tokens: number | null;
  cost: number | null;
  avg_response_time: number | null;
  unique_providers?: number | null;
  billableAmount?: OverviewAmount;
};
const displayDailyStats = computed<DisplayDailyStat[]>(() => {
  if (!isAdmin.value || (import.meta.env.DEV && isDemo.value)) return dailyStats.value;
  const charts = adminDailyCharts.value;
  if (!charts) return [];
  return charts.series.map(day => ({
    date: day.bucket_start,
    requests: day.request_count,
    tokens: day.total_tokens,
    cost: amountValue(day.billable_amount),
    billableAmount: day.billable_amount,
    avg_response_time: day.latency_ms.avg === null ? null : day.latency_ms.avg / 1000,
    unique_models: new Set(charts.models.filter(model => Date.parse(model.bucket_start) === Date.parse(day.bucket_start) && model.id !== null).map(model => model.id)).size,
    unique_providers: day.unique_providers,
  }));
});
const dailyTimeRange = ref<DateRangeParams>(
  { ...getDateRangeFromPeriod("last7days"), granularity: 'day' },
);
// 统计周期
const loadingDaily = ref(false);
const dailyError = ref('');
const loading = ref(false);
const dashboardError = ref("");
let dashboardRequestId = 0;
let dashboardController: AbortController | null = null;
let dashboardTimezone: string | null = null;
let dailyStatsRequestId = 0;
let dailyStatsController: AbortController | null = null;
let dailyStatsDebounceTimer: ReturnType<typeof setTimeout> | null = null;


const iconMap: Record<string, Component> = {
  Users,
  Activity,
  TrendingUp,
  DollarSign,
  Key,
  Hash,
  Zap,
  Database,
};

// 空状态占位卡片
const emptyStatPlaceholders = computed(() => {
  if (isAdmin.value) {
    return [
      { name: "今日请求", icon: Activity },
      { name: "今日 Token", icon: Hash },
      { name: "今日缓存", icon: Database },
      { name: "今日消费", icon: DollarSign },
      { name: "今日活跃用户", icon: Users },
    ];
  }
  return [
    { name: "今日请求", icon: Activity },
    { name: "今日 Tokens", icon: Hash },
    { name: "API Keys", icon: Key },
    { name: "今日费用", icon: DollarSign },
  ];
});

const statSkeletonCount = computed(() => emptyStatPlaceholders.value.length);

const totalStats = computed(() => {
  if (isAdmin.value && adminDailyCharts.value) {
    const summary = adminDailyCharts.value.summary;
    return {
      requests: summary.request_count,
      tokens: summary.total_tokens,
      cost: amountValue(summary.billable_amount),
      avgResponseTime: summary.latency_ms.avg === null ? null : summary.latency_ms.avg / 1000,
    };
  }
  if (dailyStats.value.length === 0) {
    return { requests: 0, tokens: 0, cost: 0, avgResponseTime: 0 };
  }
  const totals = dailyStats.value.reduce(
    (acc, stat) => {
      acc.requests += stat.requests;
      acc.tokens += stat.tokens;
      acc.cost += stat.cost;
      acc.totalResponseTime += stat.avg_response_time * stat.requests;
      return acc;
    },
    { requests: 0, tokens: 0, cost: 0, totalResponseTime: 0 },
  );
  return {
    requests: totals.requests,
    tokens: totals.tokens,
    cost: totals.cost,
    avgResponseTime:
      totals.requests > 0 ? totals.totalResponseTime / totals.requests : 0,
  };
});

// 每日模型费用（堆叠柱状图）
const MODEL_COLORS = [
  "rgba(59, 130, 246, 0.8)", // blue
  "rgba(239, 68, 68, 0.8)", // red
  "rgba(16, 185, 129, 0.8)", // green
  "rgba(245, 158, 11, 0.8)", // amber
  "rgba(139, 92, 246, 0.8)", // purple
  "rgba(6, 182, 212, 0.8)", // cyan
  "rgba(132, 204, 22, 0.8)", // lime
  "rgba(249, 115, 22, 0.8)", // orange
];

const dailyModelCostChartData = computed<ChartData<"bar">>(() => {
  if (isAdmin.value && adminDailyCharts.value && adminDailyRange.value) {
    const timezone = adminDailyRange.value.timezone;
    return {
      labels: adminDailyCharts.value.series.map(day => chartDate(day.bucket_start, timezone)),
      datasets: modelDatasets(adminDailyCharts.value, t('未知模型', 'Unknown model'), t('其他模型', 'Other models')),
    };
  }
  if (dailyStats.value.length === 0) {
    return { labels: [], datasets: [] };
  }

  // 收集所有出现过的模型
  const allModels = new Set<string>();
  dailyStats.value.forEach((day) => {
    day.model_breakdown?.forEach((mb) => allModels.add(mb.model));
  });
  const modelList = Array.from(allModels);

  // 按总费用降序排列模型
  const modelTotalCost = new Map<string, number>();
  dailyStats.value.forEach((day) => {
    day.model_breakdown?.forEach((mb) => {
      modelTotalCost.set(
        mb.model,
        (modelTotalCost.get(mb.model) || 0) + mb.cost,
      );
    });
  });
  modelList.sort(
    (a, b) => (modelTotalCost.get(b) || 0) - (modelTotalCost.get(a) || 0),
  );

  // 为每个模型创建一个 dataset
  const datasets: ChartDataset<"bar", number[]>[] = modelList.map(
    (model, index) => ({
      label: model.replace("claude-", "").replace("gpt-", ""),
      data: dailyStats.value.map((day) => {
        const found = day.model_breakdown?.find((mb) => mb.model === model);
        return found ? found.cost : 0;
      }),
      backgroundColor: MODEL_COLORS[index % MODEL_COLORS.length],
      borderRadius: 2,
      stack: "stack0",
      barPercentage: 0.6,
      categoryPercentage: 0.7,
    }),
  );

  return {
    labels: dailyStats.value.map((stat) => formatDateForChart(stat.date)),
    datasets,
  };
});

const hasDailyModelCost = computed(() => dailyModelCostChartData.value.datasets.some(dataset => dataset.data.some(value => typeof value === 'number' && value !== 0)));
const dailyCostsPartial = computed(() => {
  const data = adminDailyCharts.value;
  if (!data) return false;
  const amounts = [data.summary, ...data.series, ...data.models, ...data.providers].map(row => row.billable_amount);
  return amounts.some(amount => amountValue(amount) !== null)
    && amounts.some(amount => amountValue(amount) === null || amount.status === 'known_subtotal' || amount.status === 'estimated_subtotal');
});
const dailyCostEmptyLabel = computed(() => {
  const data = adminDailyCharts.value;
  if (!data) return t('暂无数据', 'No data');
  const amount = amountValue(data.summary.billable_amount);
  if (amount === null || dailyCostsPartial.value) return t('费用尚未确认', 'Cost not yet known');
  if (amount > 0) return t('暂无费用明细', 'No cost breakdown available');
  return data.summary.request_count === 0 ? t('暂无数据', 'No data') : t('此周期暂无计费费用', 'No billable cost in this period');
});

const dailyModelCostChartOptions = computed<ChartOptions<"bar">>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: {
    mode: "index",
    intersect: false,
  },
  scales: {
    x: {
      stacked: true,
      ticks: { font: { size: 10 } },
    },
    y: {
      stacked: true,
      title: {
        display: true,
        text: getI18nLocale() === 'en-US' ? 'Cost ($)' : '费用 ($)',
        color: "rgb(107, 114, 128)",
        font: { size: 10 },
      },
      ticks: { font: { size: 10 } },
    },
  },
  plugins: {
    legend: {
      display: true,
      position: "bottom",
      labels: { font: { size: 10 }, boxWidth: 12, padding: 8 },
    },
    tooltip: {
      callbacks: {
        label: (context: TooltipItem<"bar">) => {
          const value = typeof context.raw === "number" ? context.raw : 0;
          if (value === 0) return "";
          return `${context.dataset.label}: $${value.toFixed(4)}`;
        },
        footer: (items: TooltipItem<"bar">[]) => {
          const total = items.reduce((sum, item) => {
            const val = typeof item.raw === "number" ? item.raw : 0;
            return sum + val;
          }, 0);
          const label = getI18nLocale() === 'en-US' ? 'Total' : '总计';
          return `${label}: $${total.toFixed(4)}`;
        },
      },
    },
  },
}));

// 提供商费用分布（环形图）
const PROVIDER_COLORS = [
  "rgba(59, 130, 246, 0.8)", // blue
  "rgba(239, 68, 68, 0.8)", // red
  "rgba(16, 185, 129, 0.8)", // green
  "rgba(245, 158, 11, 0.8)", // amber
  "rgba(139, 92, 246, 0.8)", // purple
  "rgba(6, 182, 212, 0.8)", // cyan
  "rgba(132, 204, 22, 0.8)", // lime
  "rgba(249, 115, 22, 0.8)", // orange
];

const providerCostChartData = computed<ChartData<"doughnut">>(() => {
  if (isAdmin.value && adminDailyCharts.value) {
    const slices = providerSlices(adminDailyCharts.value.providers, t('未知提供商', 'Unknown provider'), t('其他提供商', 'Other providers'));
    return {
      labels: slices.map(slice => slice.label),
      datasets: [{ data: slices.map(slice => slice.value), backgroundColor: slices.map((_, i) => PROVIDER_COLORS[i % PROVIDER_COLORS.length]), borderWidth: 2, borderColor: 'rgba(255, 255, 255, 0.1)' }],
    };
  }
  if (providerSummary.value.length === 0) {
    return { labels: [], datasets: [] };
  }

  return {
    labels: providerSummary.value.map((p) => p.provider),
    datasets: [
      {
        data: providerSummary.value.map((p) => p.cost),
        backgroundColor: providerSummary.value.map(
          (_, i) => PROVIDER_COLORS[i % PROVIDER_COLORS.length],
        ),
        borderWidth: 2,
        borderColor: "rgba(255, 255, 255, 0.1)",
      },
    ],
  };
});

const providerCostChartOptions = computed<ChartOptions<"doughnut">>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  cutout: "60%",
  plugins: {
    legend: {
      position: "right",
      labels: {
        font: { size: 10 },
        boxWidth: 12,
        padding: 8,
      },
    },
    tooltip: {
      callbacks: {
        label: (context) => {
          const value = context.raw as number;
          const total = (context.dataset.data as number[]).reduce(
            (a, b) => a + b,
            0,
          );
          const percentage =
            total > 0 ? ((value / total) * 100).toFixed(1) : "0";
          return `${context.label}: $${value.toFixed(4)} (${percentage}%)`;
        },
      },
    },
  },
}));

// 每日使用趋势（折线图）- 普通用户
const dailyUsageTrendChartData = computed<ChartData<"line">>(() => {
  // 管理员不需要此图表，直接返回空数据
  if (isAdmin.value || dailyStats.value.length === 0) {
    return { labels: [], datasets: [] };
  }

  return {
    labels: dailyStats.value.map((stat) => formatDateForChart(stat.date)),
    datasets: [
      {
        label: getI18nLocale() === 'en-US' ? 'Requests' : '请求数',
        data: dailyStats.value.map((stat) => stat.requests),
        borderColor: "rgba(59, 130, 246, 0.8)",
        backgroundColor: "rgba(59, 130, 246, 0.1)",
        fill: true,
        tension: 0.3,
        yAxisID: "y",
      },
      {
        label: "Tokens (K)",
        data: dailyStats.value.map((stat) => stat.tokens / 1000),
        borderColor: "rgba(16, 185, 129, 0.8)",
        backgroundColor: "rgba(16, 185, 129, 0.1)",
        fill: true,
        tension: 0.3,
        yAxisID: "y1",
      },
    ],
  };
});

const dailyUsageTrendChartOptions = computed<ChartOptions<"line">>(() => {
  // 管理员不需要此图表
  if (isAdmin.value) {
    return {} as ChartOptions<"line">;
  }
  return {
    responsive: true,
    maintainAspectRatio: false,
    interaction: {
      mode: "index",
      intersect: false,
    },
    scales: {
      x: {
        ticks: { font: { size: 10 } },
      },
      y: {
        type: "linear",
        display: true,
        position: "left",
        title: {
          display: true,
          text: getI18nLocale() === 'en-US' ? 'Requests' : '请求数',
          color: "rgb(107, 114, 128)",
          font: { size: 10 },
        },
        ticks: { font: { size: 10 } },
      },
      y1: {
        type: "linear",
        display: true,
        position: "right",
        title: {
          display: true,
          text: "Tokens (K)",
          color: "rgb(107, 114, 128)",
          font: { size: 10 },
        },
        ticks: { font: { size: 10 } },
        grid: { drawOnChartArea: false },
      },
    },
    plugins: {
      legend: {
        display: true,
        position: "bottom",
        labels: { font: { size: 10 }, boxWidth: 12, padding: 8 },
      },
      tooltip: {
        callbacks: {
          label: (context) => {
            const value = context.raw as number;
            if (context.dataset.label === "Tokens (K)") {
              return `${context.dataset.label}: ${value.toFixed(1)}K`;
            }
            return `${context.dataset.label}: ${value}`;
          },
        },
      },
    },
  };
});

onMounted(async () => {
  await Promise.all([
    loadDashboardData(),
    loadDailyStats(),
  ]);
});

onBeforeUnmount(() => {
  if (dailyStatsDebounceTimer) {
    clearTimeout(dailyStatsDebounceTimer);
    dailyStatsDebounceTimer = null;
  }
  dailyStatsRequestId += 1;
  dailyStatsController?.abort();
  dailyStatsController = null;
  dashboardRequestId += 1;
  dashboardController?.abort();
  dashboardController = null;
});

async function loadDashboardData() {
  if (isAdmin.value) {
    return loadAdminDashboard();
  }
  const requestId = ++dashboardRequestId;
  loading.value = true;
  dashboardError.value = "";
  try {
    const statsData = await dashboardApi.getStats({
      timezone: dailyTimeRange.value.timezone,
      tz_offset_minutes: dailyTimeRange.value.tz_offset_minutes,
    });
    if (requestId !== dashboardRequestId) return;
    personalStats.value = statsData.stats.map((stat) => ({
      ...stat,
      icon: markRaw(iconMap[stat.icon] || Activity),
    }));
    applyDashboardDetails(statsData);
  } catch {
    if (requestId !== dashboardRequestId) return;
    dashboardError.value = getI18nLocale() === 'en-US'
      ? 'Dashboard statistics could not be loaded.' : '仪表盘统计加载失败，请重试。';
  } finally {
    if (requestId === dashboardRequestId) loading.value = false;
  }
}

function selectedDashboardTimezone(): string {
  return dailyTimeRange.value.timezone || Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
}

async function loadAdminDashboard() {
  const timezone = selectedDashboardTimezone();
  if (dashboardController && dashboardTimezone === timezone) return;
  dashboardController?.abort();
  if (dashboardTimezone !== timezone) dashboardSnapshot.value = null;
  dashboardTimezone = timezone;
  const requestId = ++dashboardRequestId;
  const controller = new AbortController();
  dashboardController = controller;
  loading.value = true;
  dashboardError.value = '';
  try {
    let snapshot: OverviewDashboardSummary;
    if (import.meta.env.DEV && isDemo.value) {
      const demo = await import('@/features/overview/dashboard/demo');
      snapshot = demo.createDashboardDemo(timezone);
      if (requestId !== dashboardRequestId || controller.signal.aborted) return;
      demoTimeline.value = demo.createDashboardTimelineDemo();
    } else {
      snapshot = await overviewApi.dashboardSummary(timezone, controller.signal);
    }
    if (requestId !== dashboardRequestId || controller.signal.aborted) return;
    dashboardSnapshot.value = snapshot;
  } catch {
    if (requestId !== dashboardRequestId || controller.signal.aborted) return;
    dashboardError.value = t('仪表盘统计加载失败，请重试。', 'Dashboard statistics could not be loaded. Please retry.');
  } finally {
    if (requestId === dashboardRequestId) {
      loading.value = false;
      dashboardController = null;
    }
  }
}

function applyDashboardDetails(statsData: DashboardStatsResponse) {
  if (statsData.today) todayStats.value = statsData.today;
  if (statsData.cache_stats) cacheStats.value = statsData.cache_stats;
  if (statsData.token_breakdown) tokenBreakdown.value = statsData.token_breakdown;
  if (statsData.monthly_cost !== undefined) {
    userMonthlyCost.value = statsData.monthly_cost;
  }
}

function adminStatCards(snapshot: OverviewDashboardSummary): DashboardStatCard[] {
  const { today, total, users } = snapshot;
  const cost = (value: typeof today.billable_amount) => {
    const amount = amountValue(value);
    return amount === null ? '—' : formatCurrency(amount);
  };
  const costHint = (value: typeof today.billable_amount) => {
    if (value.status === 'known_subtotal' || value.status === 'estimated_subtotal') {
      return t('部分请求价格未知，当前为已知金额小计。', 'Some request prices are unknown; this is the subtotal of known amounts.');
    }
    if (value.status === 'estimated') return t('估算金额', 'Estimated amount');
    return amountValue(value) === null ? t('金额未知', 'Amount unknown') : undefined;
  };
  return [
    { name: t('今日请求', "Today's requests"), value: metricCount(today.request_count), subValue: `${t('总请求', 'Total requests')} ${metricCount(total.request_count)}`, icon: markRaw(Activity) },
    { name: t('今日 Token', "Today's tokens"), value: `${compactTokens(today.input_tokens)} / ${compactTokens(today.output_tokens)}`, subValue: `${t('总 Token', 'Total tokens')} ${compactTokens(total.total_tokens)}`, icon: markRaw(Hash) },
    { name: t('今日缓存', "Today's cache"), value: cacheHitRate(today), valueHint: t('缓存读取 Token / 输入上下文 Token', 'Cache read tokens / input context tokens'), subValue: `${t('总缓存', 'Total cache')} ${cacheHitRate(total)}`, totalHint: `${statisticsScope.value} · ${t('累计缓存读取 Token / 累计输入上下文 Token', 'Total cache read tokens / total input context tokens')}`, icon: markRaw(Database) },
    { name: t('今日消费', "Today's spending"), value: cost(today.billable_amount), valueHint: costHint(today.billable_amount), subValue: `${t('总消费', 'Total spending')} ${cost(total.billable_amount)}`, totalHint: [statisticsScope.value, costHint(total.billable_amount)].filter(Boolean).join(' · '), icon: markRaw(DollarSign) },
    { name: t('今日活跃用户', 'Active users today'), value: metricCount(today.active_users), subValue: `${t('总用户', 'Total users')} ${metricCount(users.total)}`, userChange: { added: users.created_today, removed: users.deleted_today }, icon: markRaw(Users) },
  ];
}

async function loadDailyStats() {
  dailyStatsController?.abort();
  const controller = new AbortController();
  dailyStatsController = controller;
  const requestId = ++dailyStatsRequestId;
  loadingDaily.value = true;
  dailyError.value = '';
  try {
    if (isAdmin.value && !(import.meta.env.DEV && isDemo.value)) {
      const range = dashboardChartRange(dailyTimeRange.value);
      const response = await overviewApi.dashboardCharts(range, controller.signal);
      if (requestId !== dailyStatsRequestId || controller.signal.aborted) return;
      adminDailyCharts.value = response.data;
      adminDailyRange.value = range;
      dailyStats.value = [];
      providerSummary.value = [];
    } else {
      const response = import.meta.env.DEV && isDemo.value
        ? (await import('@/features/overview/dashboard/demo')).createDashboardDailyDemo(dailyTimeRange.value)
        : await dashboardApi.getDailyStats(dailyTimeRange.value);
      if (requestId !== dailyStatsRequestId || controller.signal.aborted) return;
      adminDailyCharts.value = null;
      adminDailyRange.value = null;
      dailyStats.value = response.daily_stats;
      providerSummary.value = response.provider_summary || [];
    }
  } catch {
    if (requestId !== dailyStatsRequestId || controller.signal.aborted) return;
    if (isAdmin.value) dailyError.value = t('统计加载失败，请重试', 'Statistics could not be loaded. Please retry.');
    dailyStats.value = [];
    providerSummary.value = [];
    adminDailyCharts.value = null;
    adminDailyRange.value = null;
  } finally {
    if (requestId === dailyStatsRequestId) {
      loadingDaily.value = false;
      dailyStatsController = null;
    }
  }
}

function scheduleDailyStatsLoad() {
  dailyStatsController?.abort();
  dailyStatsRequestId += 1;
  loadingDaily.value = true;
  if (dailyStatsDebounceTimer) {
    clearTimeout(dailyStatsDebounceTimer);
  }
  dailyStatsDebounceTimer = setTimeout(() => {
    dailyStatsDebounceTimer = null;
    void loadDailyStats();
  }, 120);
}

watch(() => JSON.stringify([
  dailyTimeRange.value.from, dailyTimeRange.value.to,
  dailyTimeRange.value.start_date, dailyTimeRange.value.end_date,
  dailyTimeRange.value.preset, dailyTimeRange.value.granularity,
  dailyTimeRange.value.timezone, dailyTimeRange.value.tz_offset_minutes,
]), scheduleDailyStatsLoad);
watch(isDemo, () => {
  dashboardController?.abort();
  dashboardController = null;
  dashboardSnapshot.value = null;
  demoTimeline.value = null;
  dailyStatsRequestId += 1;
  dailyStatsController?.abort();
  dailyStats.value = [];
  providerSummary.value = [];
  adminDailyCharts.value = null;
  adminDailyRange.value = null;
  dailyError.value = '';
  void loadDashboardData();
  void loadDailyStats();
});
watch(() => dailyTimeRange.value.timezone, () => {
  if (isAdmin.value) void loadDashboardData();
});

function formatDate(dateString: string): string {
  const date = parseDateLike(dateString);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return formatRelativeTime(0, 'day');
  if (date.toDateString() === yesterday.toDateString()) return formatRelativeTime(-1, 'day');
  return date.toLocaleDateString(getI18nLocale(), {
    month: "2-digit",
    day: "2-digit",
    weekday: "short",
  });
}

function formatDateForChart(dateString: string): string {
  const date = parseDateLike(dateString);
  const today = new Date();
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return formatRelativeTime(0, 'day');
  if (date.toDateString() === yesterday.toDateString()) return formatRelativeTime(-1, 'day');
  return date.toLocaleDateString(getI18nLocale(), { month: "numeric", day: "numeric" });
}

function formatDailyCost(cost: number | null): string {
  return cost === null ? '—' : `$${cost.toFixed(4)}`;
}

function formatDailyDate(value: string): string {
  if (isAdmin.value && adminDailyRange.value) {
    const timezone = adminDailyRange.value.timezone;
    const date = zonedInput(value, timezone).slice(0, 10);
    const today = zonedInput(new Date(), timezone).slice(0, 10);
    const yesterday = new Date(Date.parse(`${today}T00:00:00Z`) - 86_400_000).toISOString().slice(0, 10);
    if (date === today) return formatRelativeTime(0, 'day');
    if (date === yesterday) return formatRelativeTime(-1, 'day');
    return parseDateLike(date).toLocaleDateString(getI18nLocale(), { month: '2-digit', day: '2-digit', weekday: 'short' });
  }
  return formatDate(value);
}

function formatResponseTime(seconds: number | null): string {
  if (seconds === null) return '—';
  if (seconds === 0) return "-";
  if (seconds < 1) return `${(seconds * 1000).toFixed(0)}ms`;
  return `${seconds.toFixed(2)}s`;
}

</script>
