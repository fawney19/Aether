<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <!-- 页面头部：统计卡片 + 公告 -->
    <div class="flex flex-col sm:flex-row gap-6 sm:items-start">
      <!-- 左侧统计区域 -->
      <div
        ref="statsPanelRef"
        class="flex-1 min-w-0 flex flex-col"
      >
        <Badge
          :variant="authStore.user?.role === 'admin' ? 'default' : 'secondary'"
          class="uppercase tracking-[0.45em] mb-4 self-start"
        >
          {{ authStore.user?.role === 'admin' ? 'ADMIN MODE' : 'PERSONAL MODE' }}
        </Badge>

        <!-- 主要统计卡片 -->
        <div class="grid grid-cols-2 gap-3 sm:gap-4 xl:grid-cols-4">
          <!-- 加载中骨架屏 -->
          <template v-if="loading">
            <Card
              v-for="i in 4"
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
              class="relative overflow-hidden p-3 sm:p-5"
              :class="statCardBorders[index % statCardBorders.length]"
            >
              <div
                class="pointer-events-none absolute -right-4 -top-6 h-28 w-28 rounded-full blur-3xl opacity-40"
                :class="statCardGlows[index % statCardGlows.length]"
              />
              <!-- 图标固定在右上角 -->
              <div
                class="absolute top-3 right-3 sm:top-5 sm:right-5 rounded-xl sm:rounded-2xl border border-border bg-card/50 p-2 sm:p-3 shadow-inner backdrop-blur-sm"
                :class="getStatIconColor(index)"
              >
                <component
                  :is="stat.icon"
                  class="h-4 w-4 sm:h-5 sm:w-5"
                />
              </div>
              <!-- 内容区域 -->
              <div>
                <p class="text-[9px] sm:text-[11px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.4em] text-muted-foreground pr-10 sm:pr-14">
                  {{ stat.name }}
                </p>
                <p class="mt-2 sm:mt-4 text-xl sm:text-3xl font-semibold text-foreground">
                  {{ stat.value }}
                </p>
                <p
                  v-if="stat.subValue"
                  class="mt-0.5 sm:mt-1 text-[10px] sm:text-sm text-muted-foreground"
                >
                  {{ stat.subValue }}
                </p>
                <div
                  v-if="stat.change || stat.extraBadge"
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
              class="relative overflow-hidden p-3 sm:p-5"
              :class="statCardBorders[index % statCardBorders.length]"
            >
              <div
                class="pointer-events-none absolute -right-4 -top-6 h-28 w-28 rounded-full blur-3xl opacity-20"
                :class="statCardGlows[index % statCardGlows.length]"
              />
              <div
                class="absolute top-3 right-3 sm:top-5 sm:right-5 rounded-xl sm:rounded-2xl border border-border bg-card/50 p-2 sm:p-3 shadow-inner backdrop-blur-sm"
                :class="getStatIconColor(index)"
              >
                <component
                  :is="placeholder.icon"
                  class="h-4 w-4 sm:h-5 sm:w-5"
                />
              </div>
              <div>
                <p class="text-[9px] sm:text-[11px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.4em] text-muted-foreground pr-10 sm:pr-14">
                  {{ placeholder.name }}
                </p>
                <p class="mt-2 sm:mt-4 text-xl sm:text-3xl font-semibold text-muted-foreground/50">
                  --
                </p>
                <p class="mt-0.5 sm:mt-1 text-[10px] sm:text-sm text-muted-foreground/50">
                  暂无数据
                </p>
              </div>
            </Card>
          </template>
        </div>

        <!-- 管理员：系统健康摘要 -->
        <div
          v-if="isAdmin && systemHealth"
          class="mt-6"
        >
          <div class="mb-3 flex items-center justify-between">
            <h3 class="text-sm font-medium text-foreground">
              本月系统健康
            </h3>
            <Badge
              variant="outline"
              class="uppercase tracking-[0.3em] text-[10px]"
            >
              Monthly
            </Badge>
          </div>
          <div class="grid grid-cols-2 gap-2 sm:gap-3 xl:grid-cols-4">
            <Card class="relative p-3 sm:p-4 border-book-cloth/30">
              <Clock class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  平均响应
                </p>
                <p class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground">
                  {{ systemHealth.avg_response_time }}s
                </p>
              </div>
            </Card>
            <Card class="relative p-3 sm:p-4 border-kraft/30">
              <AlertTriangle class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  错误率
                </p>
                <p
                  class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold"
                  :class="systemHealth.error_rate > 5 ? 'text-destructive' : 'text-foreground'"
                >
                  {{ systemHealth.error_rate }}%
                </p>
              </div>
            </Card>
            <Card class="relative p-3 sm:p-4 border-book-cloth/25">
              <Shuffle class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  转移次数
                </p>
                <p class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground">
                  {{ systemHealth.fallback_count }}
                </p>
              </div>
            </Card>
            <Card
              v-if="costStats"
              class="relative p-3 sm:p-4 border-manilla/40"
            >
              <DollarSign class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  本月费用
                </p>
                <p class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground">
                  {{ formatCurrency(costStats.total_cost) }}
                </p>
                <Badge
                  v-if="costStats.cost_savings > 0"
                  variant="success"
                  class="mt-1 text-[9px] sm:text-[10px]"
                >
                  节省 {{ formatCurrency(costStats.cost_savings) }}
                </Badge>
              </div>
            </Card>
          </div>
        </div>

        <!-- 普通用户：月度统计 -->
        <div
          v-else-if="!isAdmin && (hasCacheData || (userMonthlyCost !== null && userMonthlyCost > 0))"
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
          <div
            class="grid gap-2 sm:gap-3"
            :class="[
              hasCacheData ? 'grid-cols-2 xl:grid-cols-4' : 'grid-cols-1 max-w-xs'
            ]"
          >
            <Card
              v-if="cacheStats"
              class="relative p-3 sm:p-4 border-book-cloth/30"
            >
              <Database class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  缓存命中率
                </p>
                <p class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground">
                  {{ cacheStats.cache_hit_rate || 0 }}%
                </p>
              </div>
            </Card>
            <Card
              v-if="cacheStats"
              class="relative p-3 sm:p-4 border-kraft/30"
            >
              <Hash class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  缓存读取
                </p>
                <p class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground">
                  {{ formatTokens(cacheStats.cache_read_tokens) }}
                </p>
              </div>
            </Card>
            <Card
              v-if="cacheStats"
              class="relative p-3 sm:p-4 border-book-cloth/25"
            >
              <Database class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  缓存创建
                </p>
                <p class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground">
                  {{ formatTokens(cacheStats.cache_creation_tokens) }}
                </p>
              </div>
            </Card>
            <Card
              v-if="userMonthlyCost !== null"
              class="relative p-3 sm:p-4 border-manilla/40"
            >
              <DollarSign class="absolute top-3 right-3 h-3.5 w-3.5 sm:h-4 sm:w-4 text-muted-foreground" />
              <div class="pr-6">
                <p class="text-[9px] sm:text-[10px] font-semibold uppercase tracking-[0.2em] sm:tracking-[0.3em] text-muted-foreground">
                  本月费用
                </p>
                <p class="mt-1.5 sm:mt-2 text-lg sm:text-xl font-semibold text-foreground">
                  {{ formatCurrency(userMonthlyCost) }}
                </p>
              </div>
            </Card>
          </div>
        </div>
      </div>

      <!-- 右侧系统公告 -->
      <div
        id="announcements-section"
        class="w-full sm:w-[260px] md:w-[300px] lg:w-[320px] flex-shrink-0 flex flex-col min-h-0"
        :style="announcementsContainerStyle"
      >
        <div class="mb-3 flex items-center justify-between flex-shrink-0">
          <h3 class="text-sm font-medium text-foreground">
            系统公告
          </h3>
          <Badge
            variant="outline"
            class="uppercase tracking-[0.3em] text-[10px]"
          >
            Live
          </Badge>
        </div>

        <Card class="overflow-hidden p-4 flex flex-col flex-1 min-h-0 h-full max-h-[280px] sm:max-h-none">
          <div
            v-if="loadingAnnouncements"
            class="flex-1 flex items-center justify-center"
          >
            <Loader2 class="h-5 w-5 animate-spin text-muted-foreground" />
          </div>

          <div
            v-else-if="announcements.length === 0"
            class="flex-1 flex flex-col items-center justify-center"
          >
            <Bell class="h-8 w-8 text-muted-foreground/40" />
            <p class="mt-2 text-xs text-muted-foreground">
              暂无公告
            </p>
          </div>

          <div
            v-else
            class="-mx-4 px-4 flex-1 overflow-y-auto scrollbar-thin min-h-0 pb-2"
          >
            <div
              ref="announcementsTimelineRef"
              class="relative pl-5"
            >
              <div
                v-if="announcements.length > 1"
                class="absolute left-[7px] w-[2px] bg-slate-200 dark:bg-muted"
                :style="timelineLineStyle"
              />

              <button
                v-for="announcement in announcements"
                :key="announcement.id"
                data-announcement-item
                type="button"
                class="relative w-full text-left mb-3 last:mb-0"
                @click="viewAnnouncementDetail(announcement)"
              >
                <div class="flex gap-2">
                  <div class="absolute left-[-18px] top-1 z-10">
                    <span
                      data-announcement-marker
                      class="flex h-3 w-3 items-center justify-center rounded-full border-2 border-white dark:border-slate-900"
                      :class="[
                        announcement.is_pinned
                          ? 'bg-amber-500 dark:bg-amber-400'
                          : announcement.is_read
                            ? 'bg-slate-300 dark:bg-slate-600'
                            : getAnnouncementDotColor(announcement.type)
                      ]"
                    >
                      <span
                        v-if="!announcement.is_read && !announcement.is_pinned"
                        class="h-1.5 w-1.5 rounded-full bg-white"
                      />
                    </span>
                  </div>

                  <div
                    class="flex-1 rounded-lg p-2 transition"
                    :class="[
                      announcement.is_pinned
                        ? 'hover:bg-amber-50/50 dark:hover:bg-amber-900/10'
                        : 'hover:bg-slate-50/50 dark:hover:bg-slate-800/30'
                    ]"
                  >
                    <div class="flex items-center gap-2 mb-1">
                      <h4 class="text-xs font-medium text-foreground line-clamp-1 flex-1">
                        {{ announcement.title }}
                      </h4>
                      <span
                        v-if="announcement.is_pinned"
                        class="flex-shrink-0 rounded-full bg-amber-100 dark:bg-amber-900/30 px-1.5 py-0.5 text-[9px] font-medium text-amber-700 dark:text-amber-400"
                      >
                        置顶
                      </span>
                    </div>
                    <div class="text-[11px] text-muted-foreground leading-relaxed line-clamp-2 mb-1">
                      {{ getPlainText(announcement.content) }}
                    </div>
                    <div class="text-[10px] text-muted-foreground/70">
                      {{ formatAnnouncementDate(announcement.created_at) }}
                    </div>
                  </div>
                </div>
              </button>
            </div>
          </div>
        </Card>
      </div>
    </div>

    <!-- 趋势图表区域 -->
    <div class="grid grid-cols-1 gap-6 lg:grid-cols-2">
      <!-- 请求次数和费用趋势 -->
      <Card class="p-5">
        <h4 class="mb-3 text-xs font-semibold text-foreground uppercase tracking-wider">
          请求次数 / 费用趋势
        </h4>
        <div
          v-if="loadingDaily"
          class="flex items-center justify-center h-[280px]"
        >
          <Skeleton class="h-full w-full" />
        </div>
        <div
          v-else
          style="height: 280px;"
        >
          <LineChart
            v-if="chartData.requests"
            :data="chartData.requests"
            :options="chartOptions.requests"
          />
          <div
            v-else
            class="flex h-full items-center justify-center text-xs text-muted-foreground"
          >
            暂无数据
          </div>
        </div>
      </Card>

      <!-- 每日模型成本（堆叠柱状图） -->
      <Card class="p-5">
        <h4 class="mb-3 text-xs font-semibold text-foreground uppercase tracking-wider">
          每日模型成本
        </h4>
        <div
          v-if="loadingDaily"
          class="flex items-center justify-center h-[280px]"
        >
          <Skeleton class="h-full w-full" />
        </div>
        <div
          v-else
          style="height: 280px;"
        >
          <BarChart
            v-if="dailyModelCostChartData.labels && dailyModelCostChartData.labels.length > 0"
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

    <!-- 每日统计 -->
    <Card class="overflow-hidden mt-6">
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
          v-else-if="dailyStats.length === 0"
          class="py-8 text-center text-muted-foreground text-xs"
        >
          暂无数据
        </div>
        <div
          v-else
          class="divide-y divide-border/60"
        >
          <div
            v-for="stat in dailyStats.slice().reverse()"
            :key="stat.date"
            class="p-4 space-y-2"
          >
            <div class="flex items-center justify-between">
              <span class="font-medium text-sm">{{ formatDate(stat.date) }}</span>
              <Badge
                variant="success"
                class="text-[10px]"
              >
                ${{ stat.cost.toFixed(4) }}
              </Badge>
            </div>
            <div class="grid grid-cols-2 gap-2 text-xs">
              <div class="flex justify-between">
                <span class="text-muted-foreground">请求</span>
                <span>{{ stat.requests.toLocaleString() }}</span>
              </div>
              <div class="flex justify-between">
                <span class="text-muted-foreground">Tokens</span>
                <span>{{ formatTokens(stat.tokens) }}</span>
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
            <TableHead class="text-center">
              使用提供商
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <TableRow v-if="loadingDaily">
            <TableCell
              colspan="7"
              class="text-center py-8"
            >
              <div class="flex items-center justify-center gap-2">
                <Skeleton class="h-5 w-5 rounded-full" />
                <span class="text-muted-foreground text-xs">加载中...</span>
              </div>
            </TableCell>
          </TableRow>
          <TableRow v-else-if="dailyStats.length === 0">
            <TableCell
              colspan="7"
              class="text-center py-8 text-muted-foreground text-xs"
            >
              暂无数据
            </TableCell>
          </TableRow>
          <template v-else>
            <TableRow
              v-for="stat in dailyStats.slice().reverse()"
              :key="stat.date"
            >
              <TableCell class="font-medium text-xs">
                {{ formatDate(stat.date) }}
              </TableCell>
              <TableCell class="text-center text-xs">
                {{ stat.requests.toLocaleString() }}
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="secondary"
                  class="text-[10px]"
                >
                  {{ formatTokens(stat.tokens) }}
                </Badge>
              </TableCell>
              <TableCell class="text-center">
                <Badge
                  variant="success"
                  class="text-[10px]"
                >
                  ${{ stat.cost.toFixed(4) }}
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
              <TableCell class="text-center text-xs">
                {{ stat.unique_providers }}
              </TableCell>
            </TableRow>
          </template>
        </TableBody>
      </Table>

      <!-- 汇总信息 -->
      <div
        v-if="dailyStats.length > 0"
        class="border-t border-border bg-muted/30 backdrop-blur-sm px-4 py-3 text-xs"
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
              {{ formatTokens(totalStats.tokens) }}
            </div>
          </div>
          <div class="text-center">
            <div class="text-muted-foreground text-[10px]">
              总费用
            </div>
            <div class="font-semibold text-amber-600 dark:text-amber-400">
              ${{ totalStats.cost.toFixed(4) }}
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

  <!-- 公告详情对话框 -->
  <Dialog
    v-model="detailDialogOpen"
    size="lg"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <component
            :is="getAnnouncementIcon(selectedAnnouncement.type)"
            v-if="selectedAnnouncement"
            class="h-5 w-5 flex-shrink-0"
            :class="getAnnouncementIconColor(selectedAnnouncement.type)"
          />
          <div class="flex-1 min-w-0">
            <h3 class="text-lg font-semibold text-foreground leading-tight truncate">
              {{ selectedAnnouncement?.title || '公告详情' }}
            </h3>
            <p class="text-xs text-muted-foreground">
              系统公告
            </p>
          </div>
        </div>
      </div>
    </template>

    <div
      v-if="selectedAnnouncement"
      class="space-y-4"
    >
      <div class="text-xs text-muted-foreground">
        {{ formatFullDate(selectedAnnouncement.created_at) }}
      </div>

      <div
        class="prose prose-sm dark:prose-invert max-w-none"
        v-html="renderMarkdown(selectedAnnouncement.content)"
      />
    </div>

    <template #footer>
      <Button
        variant="outline"
        class="h-10 px-5"
        @click="detailDialogOpen = false"
      >
        关闭
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, onMounted, computed, onBeforeUnmount, nextTick, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { dashboardApi, type DashboardStat, type DailyStat } from '@/api/dashboard'
import { announcementApi, type Announcement } from '@/api/announcements'
import {
  Card,
  Badge,
  Button,
  Skeleton,
  Dialog,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from '@/components/ui'
import LineChart from '@/components/charts/LineChart.vue'
import BarChart from '@/components/charts/BarChart.vue'
import {
  Users,
  Activity,
  TrendingUp,
  DollarSign,
  Key,
  Hash,
  Bell,
  AlertCircle,
  AlertTriangle,
  Info,
  Wrench,
  Loader2,
  Clock,
  Database,
  Shuffle
} from 'lucide-vue-next'
import { formatTokens, formatCurrency } from '@/utils/format'
import { marked } from 'marked'
import { sanitizeMarkdown } from '@/utils/sanitize'
import type { ChartData, ChartOptions, ChartDataset, TooltipItem } from 'chart.js'

const authStore = useAuthStore()

const statsPanelRef = ref<HTMLElement | null>(null)
const announcementsHeight = ref<number | null>(null)
const announcementsTimelineRef = ref<HTMLElement | null>(null)
const timelineLineStyle = ref<{ top: string; bottom: string }>({ top: '0px', bottom: '0px' })
const isLargeScreen = ref(false)

const announcementsContainerStyle = computed(() => {
  // 移动端不设置固定高度，让内容自然流动
  if (!isLargeScreen.value || !announcementsHeight.value) return {}
  // 桌面端设置固定高度，与左侧统计面板保持一致
  return { height: `${announcementsHeight.value}px` }
})

function checkScreenSize() {
  if (typeof window !== 'undefined') {
    isLargeScreen.value = window.innerWidth >= 640 // sm breakpoint
  }
}

let statsPanelObserver: ResizeObserver | null = null
let announcementsTimelineObserver: ResizeObserver | null = null

function updateAnnouncementsHeight() {
  if (typeof window === 'undefined') return
  const panel = statsPanelRef.value
  if (!panel) return
  const { height } = panel.getBoundingClientRect()
  if (height <= 0) return
  announcementsHeight.value = Math.round(height)
  nextTick(() => updateTimelineLine())
}

function updateTimelineLine() {
  if (typeof window === 'undefined') return
  const container = announcementsTimelineRef.value
  if (!container) return
  const items = container.querySelectorAll<HTMLElement>('[data-announcement-item]')
  if (items.length < 2) {
    timelineLineStyle.value = { top: '0px', bottom: '0px' }
    return
  }
  const firstMarker = items[0].querySelector<HTMLElement>('[data-announcement-marker]')
  const lastMarker = items[items.length - 1].querySelector<HTMLElement>('[data-announcement-marker]')
  if (!firstMarker || !lastMarker) return
  const containerRect = container.getBoundingClientRect()
  const firstRect = firstMarker.getBoundingClientRect()
  const lastRect = lastMarker.getBoundingClientRect()
  const topOffset = Math.max(0, firstRect.top + firstRect.height / 2 - containerRect.top)
  const bottomOffset = Math.max(0, containerRect.bottom - (lastRect.top + lastRect.height / 2))
  timelineLineStyle.value = { top: `${topOffset}px`, bottom: `${bottomOffset}px` }
}

function handleWindowResize() {
  checkScreenSize()
  updateAnnouncementsHeight()
  updateTimelineLine()
}

function setupResizeObserver() {
  if (typeof window === 'undefined') return
  const panel = statsPanelRef.value
  if (!panel || !('ResizeObserver' in window)) return
  statsPanelObserver = new ResizeObserver(() => updateAnnouncementsHeight())
  statsPanelObserver.observe(panel)
  updateAnnouncementsHeight()
}

function setupTimelineResizeObserver() {
  if (typeof window === 'undefined' || !('ResizeObserver' in window)) return
  const container = announcementsTimelineRef.value
  announcementsTimelineObserver?.disconnect()
  announcementsTimelineObserver = null
  if (!container) return
  announcementsTimelineObserver = new ResizeObserver(() => updateTimelineLine())
  announcementsTimelineObserver.observe(container)
}

const isAdmin = computed(() => authStore.user?.role === 'admin')

const statCardBorders = [
  'border-book-cloth/30 dark:border-book-cloth/25',
  'border-kraft/30 dark:border-kraft/25',
  'border-manilla/40 dark:border-manilla/30',
  'border-book-cloth/25 dark:border-kraft/25'
]

const statCardGlows = [
  'bg-book-cloth/30',
  'bg-kraft/30',
  'bg-manilla/35',
  'bg-kraft/30'
]

const getStatIconColor = (_index: number): string => {
  return 'text-muted-foreground'
}

// 统计数据
const stats = ref<DashboardStat[]>([])
const todayStats = ref<{
  requests: number
  tokens: number
  cost: number
  actual_cost?: number
  cache_creation_tokens?: number
  cache_read_tokens?: number
}>({ requests: 0, tokens: 0, cost: 0 })

const systemHealth = ref<{
  avg_response_time: number
  error_rate: number
  error_requests: number
  fallback_count: number
  total_requests: number
} | null>(null)

const costStats = ref<{
  total_cost: number
  total_actual_cost: number
  cost_savings: number
} | null>(null)

const cacheStats = ref<{
  cache_creation_tokens: number
  cache_read_tokens: number
  cache_creation_cost?: number
  cache_read_cost?: number
  cache_hit_rate?: number
  total_cache_tokens: number
} | null>(null)

const userMonthlyCost = ref<number | null>(null)

const hasCacheData = computed(() =>
  cacheStats.value && cacheStats.value.total_cache_tokens > 0
)

const tokenBreakdown = ref<{
  input: number
  output: number
  cache_creation: number
  cache_read: number
} | null>(null)

const activeUsers = ref(0)
const dailyStats = ref<DailyStat[]>([])
const selectedDays = ref(7)
const loadingDaily = ref(false)
const loading = ref(false)


// 公告
const announcements = ref<Announcement[]>([])
const loadingAnnouncements = ref(false)
const selectedAnnouncement = ref<Announcement | null>(null)
const detailDialogOpen = ref(false)

const iconMap: Record<string, any> = {
  Users, Activity, TrendingUp, DollarSign, Key, Hash, Database
}

// 空状态占位卡片
const emptyStatPlaceholders = computed(() => {
  if (isAdmin.value) {
    return [
      { name: '今日请求', icon: Activity },
      { name: '今日 Tokens', icon: Hash },
      { name: '活跃用户', icon: Users },
      { name: '今日费用', icon: DollarSign }
    ]
  }
  return [
    { name: '今日请求', icon: Activity },
    { name: '今日 Tokens', icon: Hash },
    { name: 'API Keys', icon: Key },
    { name: '今日费用', icon: DollarSign }
  ]
})

const totalStats = computed(() => {
  if (dailyStats.value.length === 0) {
    return { requests: 0, tokens: 0, cost: 0, avgResponseTime: 0 }
  }
  const totals = dailyStats.value.reduce((acc, stat) => {
    acc.requests += stat.requests
    acc.tokens += stat.tokens
    acc.cost += stat.cost
    acc.totalResponseTime += stat.avg_response_time * stat.requests
    return acc
  }, { requests: 0, tokens: 0, cost: 0, totalResponseTime: 0 })
  return {
    requests: totals.requests,
    tokens: totals.tokens,
    cost: totals.cost,
    avgResponseTime: totals.requests > 0 ? totals.totalResponseTime / totals.requests : 0
  }
})

// 图表数据
const chartData = computed(() => {
  if (dailyStats.value.length === 0) {
    return { requests: null }
  }

  const labels = dailyStats.value.map(stat => formatDateForChart(stat.date))
  const requests = dailyStats.value.map(stat => stat.requests)
  const costs = dailyStats.value.map(stat => stat.cost)

  return {
    requests: {
      labels,
      datasets: [
        {
          label: '请求次数',
          data: requests,
          borderColor: 'rgb(59, 130, 246)',
          backgroundColor: 'rgba(59, 130, 246, 0.1)',
          tension: 0.4,
          yAxisID: 'y'
        },
        {
          label: '费用 ($)',
          data: costs,
          borderColor: 'rgb(34, 197, 94)',
          backgroundColor: 'rgba(34, 197, 94, 0.1)',
          tension: 0.4,
          yAxisID: 'y1'
        }
      ]
    } as ChartData<'line'>
  }
})

// 每日模型成本（堆叠柱状图）
const MODEL_COLORS = [
  'rgba(59, 130, 246, 0.8)',   // blue
  'rgba(239, 68, 68, 0.8)',    // red
  'rgba(16, 185, 129, 0.8)',   // green
  'rgba(245, 158, 11, 0.8)',   // amber
  'rgba(139, 92, 246, 0.8)',   // purple
  'rgba(6, 182, 212, 0.8)',    // cyan
  'rgba(132, 204, 22, 0.8)',   // lime
  'rgba(249, 115, 22, 0.8)'    // orange
]

const dailyModelCostChartData = computed<ChartData<'bar'>>(() => {
  if (dailyStats.value.length === 0) {
    return { labels: [], datasets: [] }
  }

  // 收集所有出现过的模型
  const allModels = new Set<string>()
  dailyStats.value.forEach(day => {
    day.model_breakdown?.forEach(mb => allModels.add(mb.model))
  })
  const modelList = Array.from(allModels)

  // 按总费用降序排列模型
  const modelTotalCost = new Map<string, number>()
  dailyStats.value.forEach(day => {
    day.model_breakdown?.forEach(mb => {
      modelTotalCost.set(mb.model, (modelTotalCost.get(mb.model) || 0) + mb.cost)
    })
  })
  modelList.sort((a, b) => (modelTotalCost.get(b) || 0) - (modelTotalCost.get(a) || 0))

  // 为每个模型创建一个 dataset
  const datasets: ChartDataset<'bar', number[]>[] = modelList.map((model, index) => ({
    label: model.replace('claude-', '').replace('gpt-', ''),
    data: dailyStats.value.map(day => {
      const found = day.model_breakdown?.find(mb => mb.model === model)
      return found ? found.cost : 0
    }),
    backgroundColor: MODEL_COLORS[index % MODEL_COLORS.length],
    borderRadius: 2,
    stack: 'stack0',
    barPercentage: 0.6,
    categoryPercentage: 0.7
  }))

  return {
    labels: dailyStats.value.map(stat => formatDateForChart(stat.date)),
    datasets
  }
})

const dailyModelCostChartOptions = computed<ChartOptions<'bar'>>(() => ({
  responsive: true,
  maintainAspectRatio: false,
  interaction: {
    mode: 'index',
    intersect: false
  },
  scales: {
    x: {
      stacked: true,
      ticks: { font: { size: 10 } }
    },
    y: {
      stacked: true,
      title: { display: true, text: '费用 ($)', color: 'rgb(107, 114, 128)', font: { size: 10 } },
      ticks: { font: { size: 10 } }
    }
  },
  plugins: {
    legend: {
      display: true,
      position: 'bottom',
      labels: { font: { size: 10 }, boxWidth: 12, padding: 8 }
    },
    tooltip: {
      callbacks: {
        label: (context: TooltipItem<'bar'>) => {
          const value = typeof context.raw === 'number' ? context.raw : 0
          if (value === 0) return ''
          return `${context.dataset.label}: $${value.toFixed(4)}`
        },
        footer: (items: TooltipItem<'bar'>[]) => {
          const total = items.reduce((sum, item) => {
            const val = typeof item.raw === 'number' ? item.raw : 0
            return sum + val
          }, 0)
          return `Total: $${total.toFixed(4)}`
        }
      }
    }
  }
}))

const chartOptions = computed(() => ({
  requests: {
    scales: {
      y: {
        type: 'linear',
        display: true,
        position: 'left',
        title: { display: true, text: '请求次数', color: 'rgb(107, 114, 128)', font: { size: 10 } }
      },
      y1: {
        type: 'linear',
        display: true,
        position: 'right',
        title: { display: true, text: '费用 ($)', color: 'rgb(107, 114, 128)', font: { size: 10 } },
        grid: { drawOnChartArea: false }
      }
    },
    plugins: {
      legend: { labels: { font: { size: 11 } } },
      tooltip: {
        callbacks: {
          label: (context: any) => {
            const label = context.dataset.label || ''
            const value = context.parsed.y
            if (label.includes('费用')) return `${label}: $${value.toFixed(4)}`
            return `${label}: ${value.toLocaleString()}`
          }
        }
      }
    }
  } as ChartOptions<'line'>
}))

onMounted(async () => {
  checkScreenSize()
  setupResizeObserver()
  if (typeof window !== 'undefined') {
    window.addEventListener('resize', handleWindowResize)
  }
  await Promise.all([
    loadDashboardData(),
    loadDailyStats(),
    loadAnnouncements()
  ])
  await nextTick()
  setupTimelineResizeObserver()
  updateAnnouncementsHeight()
  updateTimelineLine()
})

onBeforeUnmount(() => {
  if (typeof window !== 'undefined') {
    window.removeEventListener('resize', handleWindowResize)
  }
  if (statsPanelObserver && statsPanelRef.value) {
    statsPanelObserver.unobserve(statsPanelRef.value)
  }
  statsPanelObserver?.disconnect()
  statsPanelObserver = null
  announcementsTimelineObserver?.disconnect()
  announcementsTimelineObserver = null
})

async function loadDashboardData() {
  loading.value = true
  try {
    const statsData = await dashboardApi.getStats()
    stats.value = statsData.stats.map(stat => ({
      ...stat,
      icon: iconMap[stat.icon] || Activity
    }))
    if (statsData.today) todayStats.value = statsData.today
    if (isAdmin.value) {
      if (statsData.system_health) systemHealth.value = statsData.system_health
      if (statsData.cost_stats) costStats.value = statsData.cost_stats
      if (statsData.cache_stats) cacheStats.value = statsData.cache_stats
      if (statsData.token_breakdown) tokenBreakdown.value = statsData.token_breakdown
      if (statsData.users) activeUsers.value = statsData.users.active
    } else {
      if (statsData.cache_stats) cacheStats.value = statsData.cache_stats
      if (statsData.token_breakdown) tokenBreakdown.value = statsData.token_breakdown
      if (statsData.monthly_cost !== undefined) userMonthlyCost.value = statsData.monthly_cost
    }
  } finally {
    loading.value = false
  }
}

async function loadDailyStats() {
  loadingDaily.value = true
  try {
    const response = await dashboardApi.getDailyStats(selectedDays.value)
    dailyStats.value = response.daily_stats
  } catch {
    dailyStats.value = []
  } finally {
    loadingDaily.value = false
  }
}

function formatDate(dateString: string): string {
  const date = new Date(dateString)
  const today = new Date()
  const yesterday = new Date(today)
  yesterday.setDate(yesterday.getDate() - 1)
  if (date.toDateString() === today.toDateString()) return '今天'
  if (date.toDateString() === yesterday.toDateString()) return '昨天'
  return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit', weekday: 'short' })
}

function formatDateForChart(dateString: string): string {
  const date = new Date(dateString)
  const today = new Date()
  const yesterday = new Date(today)
  yesterday.setDate(yesterday.getDate() - 1)
  if (date.toDateString() === today.toDateString()) return '今天'
  if (date.toDateString() === yesterday.toDateString()) return '昨天'
  return date.toLocaleDateString('zh-CN', { month: 'numeric', day: 'numeric' })
}

function formatResponseTime(seconds: number): string {
  if (seconds === 0) return '-'
  if (seconds < 1) return `${(seconds * 1000).toFixed(0)}ms`
  return `${seconds.toFixed(2)}s`
}

// 公告相关
async function loadAnnouncements() {
  loadingAnnouncements.value = true
  try {
    const response = await announcementApi.getAnnouncements({ active_only: true, limit: 100 })
    announcements.value = response.items
  } catch {
    announcements.value = []
  } finally {
    loadingAnnouncements.value = false
    await nextTick()
    setupTimelineResizeObserver()
    updateTimelineLine()
  }
}

watch(() => announcements.value.length, async () => {
  await nextTick()
  setupTimelineResizeObserver()
  updateTimelineLine()
})

async function viewAnnouncementDetail(announcement: Announcement) {
  if (!announcement.is_read && !isAdmin.value) {
    try {
      await announcementApi.markAsRead(announcement.id)
      announcement.is_read = true
    } catch { /* 静默忽略标记已读错误 */ }
  }
  selectedAnnouncement.value = announcement
  detailDialogOpen.value = true
}

function getPlainText(content: string): string {
  const cleaned = content
    .replace(/```[\s\S]*?```/g, ' ')
    .replace(/`[^`]*`/g, ' ')
    .replace(/!\[[^\]]*]\([^)]*\)/g, ' ')
    .replace(/\[[^\]]*]\(([^)]*)\)/g, '$1')
    .replace(/[#>*_~]/g, '')
    .replace(/\n+/g, ' ')
    .replace(/\s+/g, ' ')
    .trim()
  if (cleaned.length <= 100) return cleaned
  return `${cleaned.slice(0, 100).trim()}...`
}

function getAnnouncementIcon(type: string) {
  switch (type) {
    case 'important': return AlertCircle
    case 'warning': return AlertTriangle
    case 'maintenance': return Wrench
    default: return Info
  }
}

function getAnnouncementIconColor(type: string) {
  switch (type) {
    case 'important': return 'text-rose-600 dark:text-rose-400'
    case 'warning': return 'text-amber-600 dark:text-amber-400'
    case 'maintenance': return 'text-orange-600 dark:text-orange-400'
    default: return 'text-primary dark:text-primary'
  }
}

function formatAnnouncementDate(dateString: string): string {
  const date = new Date(dateString)
  const now = new Date()
  const diff = now.getTime() - date.getTime()
  const minutes = Math.floor(diff / (1000 * 60))
  const hours = Math.floor(diff / (1000 * 60 * 60))
  const days = Math.floor(diff / (1000 * 60 * 60 * 24))
  if (minutes < 1) return '刚刚'
  if (minutes < 60) return `${minutes}分钟前`
  if (hours < 24) return `${hours}小时前`
  if (days < 7) return `${days}天前`
  return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
}

function getAnnouncementDotColor(type: string): string {
  switch (type) {
    case 'important': return 'bg-rose-500 dark:bg-rose-400'
    case 'warning': return 'bg-amber-500 dark:bg-amber-400'
    case 'maintenance': return 'bg-orange-500 dark:bg-orange-400'
    default: return 'bg-emerald-500 dark:bg-emerald-400'
  }
}

function formatFullDate(dateString: string): string {
  const date = new Date(dateString)
  return date.toLocaleDateString('zh-CN', { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
}

function renderMarkdown(content: string): string {
  const rawHtml = marked(content) as string
  return sanitizeMarkdown(rawHtml)
}
</script>

<style scoped>
.line-clamp-1,
.line-clamp-2 {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.line-clamp-1 { -webkit-line-clamp: 1; }
.line-clamp-2 { -webkit-line-clamp: 2; }

.scrollbar-thin::-webkit-scrollbar { width: 5px; }
.scrollbar-thin::-webkit-scrollbar-track { background: transparent; }
.scrollbar-thin::-webkit-scrollbar-thumb { background: rgb(203 213 225); border-radius: 2px; }
.dark .scrollbar-thin::-webkit-scrollbar-thumb { background: rgb(71 85 105); }
.scrollbar-thin::-webkit-scrollbar-thumb:hover { background: rgb(148 163 184); }
.dark .scrollbar-thin::-webkit-scrollbar-thumb:hover { background: rgb(100 116 139); }

:deep(.prose) { color: var(--color-text); }
:deep(.prose p) { margin-top: 0.75em; margin-bottom: 0.75em; line-height: 1.65; }
:deep(.prose ul), :deep(.prose ol) { margin-top: 0.75em; margin-bottom: 0.75em; padding-left: 1.5em; }
:deep(.prose li) { margin-top: 0.25em; margin-bottom: 0.25em; }
:deep(.prose h1), :deep(.prose h2), :deep(.prose h3), :deep(.prose h4) { margin-top: 1.5em; margin-bottom: 0.75em; font-weight: 600; color: var(--color-text); }
:deep(.prose code) { background: var(--color-code-background); color: var(--color-code-text); padding: 0.2em 0.4em; border-radius: 4px; font-size: 0.9em; font-weight: 500; }
:deep(.prose pre) { background: var(--color-code-background); padding: 1em; border-radius: 8px; overflow-x: auto; }
:deep(.prose a) { color: var(--book-cloth); text-decoration: underline; }
:deep(.prose blockquote) { border-left: 3px solid var(--book-cloth); padding-left: 1em; margin-left: 0; font-style: italic; color: var(--cloud-dark); }
:deep(.prose strong) { font-weight: 600; }
</style>
