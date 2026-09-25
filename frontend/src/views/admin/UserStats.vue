<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <div class="flex flex-col gap-4 xl:flex-row xl:items-start xl:justify-between">
      <div>
        <h1 class="text-lg font-semibold">
          {{ t('userStats.title') }}
        </h1>
        <p class="text-xs text-muted-foreground">
          {{ t('userStats.description') }}
        </p>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <Select v-model="scope">
          <SelectTrigger class="h-8 w-32 text-xs">
            <SelectValue :placeholder="t('userStats.scope.placeholder')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="user">
              {{ t('userStats.scope.user') }}
            </SelectItem>
            <SelectItem value="user_group">
              {{ t('userStats.scope.userGroup') }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Select v-model="selectedEntityId">
          <SelectTrigger class="h-8 w-52 text-xs">
            <SelectValue :placeholder="scope === 'user' ? t('userStats.select.user') : t('userStats.select.userGroup')" />
          </SelectTrigger>
          <SelectContent
            :search-threshold="0"
            :search-placeholder="scope === 'user' ? t('userStats.search.user') : t('userStats.search.userGroup')"
          >
            <SelectItem
              v-for="entity in allEntities"
              :key="entity.id"
              :value="entity.id"
            >
              {{ entity.name }}
            </SelectItem>
          </SelectContent>
        </Select>
        <Select v-model="compareEntityId">
          <SelectTrigger class="h-8 w-52 text-xs">
            <SelectValue :placeholder="t('userStats.compare.placeholder')" />
          </SelectTrigger>
          <SelectContent
            :search-threshold="0"
            :search-placeholder="scope === 'user' ? t('userStats.search.user') : t('userStats.search.userGroup')"
          >
            <SelectItem value="__none__">
              {{ t('userStats.compare.none') }}
            </SelectItem>
            <SelectItem
              v-for="entity in comparisonEntities"
              :key="`compare-${entity.id}`"
              :value="entity.id"
            >
              {{ entity.name }}
            </SelectItem>
          </SelectContent>
        </Select>
        <TimeRangePicker
          v-model="timeRange"
          :allow-hourly="true"
        />
      </div>
    </div>

    <div class="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <LeaderboardTable
        :title="scope === 'user' ? t('userStats.leaderboard.user') : t('userStats.leaderboard.userGroup')"
        :items="leaderboard"
        :metric="metric"
        :loading="leaderboardLoading"
        :show-member-count="scope === 'user_group'"
        selectable
        @update:metric="metric = $event"
        @select="selectLeaderboardItem"
      >
        <template #pagination>
          <div class="flex items-center justify-between border-t px-4 py-3 text-xs text-muted-foreground">
            <span>{{ t('userStats.pagination.summary', { total: leaderboardTotal, page: currentPage }) }}</span>
            <div class="flex gap-2">
              <Button
                variant="outline"
                size="sm"
                :disabled="leaderboardOffset === 0 || leaderboardLoading"
                @click="changeLeaderboardPage(-1)"
              >
                {{ t('userStats.pagination.previous') }}
              </Button>
              <Button
                variant="outline"
                size="sm"
                :disabled="!hasNextLeaderboardPage || leaderboardLoading"
                @click="changeLeaderboardPage(1)"
              >
                {{ t('userStats.pagination.next') }}
              </Button>
            </div>
          </div>
        </template>
      </LeaderboardTable>

      <Card class="space-y-3 p-4">
        <div>
          <h3 class="text-sm font-semibold">
            {{ scope === 'user' ? t('userStats.summary.user') : t('userStats.summary.userGroup') }}
          </h3>
          <p class="mt-0.5 truncate text-xs text-muted-foreground">
            {{ selectedEntityName || t('userStats.selectPrompt') }}
          </p>
        </div>
        <div
          v-if="summaryLoading"
          class="p-6"
        >
          <LoadingState />
        </div>
        <div
          v-else
          class="grid grid-cols-2 gap-3 text-sm"
        >
          <div>
            <div class="text-xs text-muted-foreground">
              {{ t('stats.metric.requests') }}
            </div>
            <div class="font-semibold">
              {{ usageSummary?.total_requests ?? 0 }}
            </div>
          </div>
          <div>
            <div class="text-xs text-muted-foreground">
              {{ t('stats.metric.tokens') }}
            </div>
            <div class="font-semibold">
              {{ formatTokens(usageSummary?.total_tokens ?? 0) }}
            </div>
          </div>
          <div>
            <div class="text-xs text-muted-foreground">
              {{ t('stats.metric.cost') }}
            </div>
            <div class="font-semibold">
              {{ formatCurrency(usageSummary?.total_cost ?? 0) }}
            </div>
          </div>
          <div>
            <div class="text-xs text-muted-foreground">
              {{ t('stats.metric.errorRate') }}
            </div>
            <div class="font-semibold">
              {{ usageSummary?.error_rate ?? 0 }}%
            </div>
          </div>
          <template v-if="scope === 'user_group'">
            <div>
              <div class="text-xs text-muted-foreground">
                {{ t('userStats.members.current') }}
              </div>
              <div class="font-semibold">
                {{ groupMemberCount }}
              </div>
            </div>
            <div>
              <div class="text-xs text-muted-foreground">
                {{ t('userStats.members.active') }}
              </div>
              <div class="font-semibold">
                {{ activeGroupMemberCount }}
              </div>
            </div>
          </template>
        </div>
      </Card>
    </div>

    <LeaderboardTable
      v-if="scope === 'user_group'"
      :title="t('userStats.memberLeaderboard')"
      :items="memberLeaderboard"
      :metric="metric"
      :loading="memberLeaderboardLoading"
      :show-metric-select="false"
      selectable
      @select="selectMember"
    />

    <Card class="space-y-4 p-4">
      <div>
        <h3 class="text-sm font-semibold">
          {{ scope === 'user' ? t('userStats.trend.user') : t('userStats.trend.userGroup') }}
        </h3>
        <p class="mt-0.5 truncate text-xs text-muted-foreground">
          {{ selectedEntityName || t('userStats.selectPrompt') }}
        </p>
      </div>
      <div
        v-if="seriesLoading"
        class="p-6"
      >
        <LoadingState />
      </div>
      <div
        v-else
        class="h-[280px]"
      >
        <LineChart :data="seriesChartData" />
      </div>
    </Card>

    <Card
      v-if="comparisonSeries.length > 0"
      class="space-y-4 p-4"
    >
      <h3 class="text-sm font-semibold">
        {{ scope === 'user' ? t('userStats.comparisonTrend.user') : t('userStats.comparisonTrend.userGroup') }}
      </h3>
      <div class="h-[280px]">
        <LineChart :data="comparisonChartData" />
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import {
  Button,
  Card,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from '@/components/ui'
import LineChart from '@/components/charts/LineChart.vue'
import { LoadingState, TimeRangePicker } from '@/components/common'
import { LeaderboardTable } from '@/components/stats'
import { adminApi, type LeaderboardItem } from '@/api/admin'
import { usersApi, type User, type UserGroup, type UserGroupMember } from '@/api/users'
import { usageApi } from '@/api/usage'
import { formatCurrency, formatTokens } from '@/utils/format'
import { useI18n } from '@/i18n'
import { getDateRangeFromPeriod } from '@/features/usage/composables'
import type { DateRangeParams } from '@/features/usage/types'

type StatsScope = 'user' | 'user_group'
type SelectableEntity = { id: string; name: string }

interface UsageSummary {
  total_requests: number
  total_tokens: number
  total_cost: number
  error_rate: number
}

interface TimeSeriesItem {
  date: string
  total_cost: number
}

const { t } = useI18n()

const PAGE_SIZE = 10
const timeRange = ref<DateRangeParams>(getDateRangeFromPeriod('last7days'))
const metric = ref<'requests' | 'tokens' | 'cost'>('requests')
const scope = ref<StatsScope>('user')

const users = ref<User[]>([])
const userGroups = ref<UserGroup[]>([])
const selectedUserId = ref('')
const selectedUserGroupId = ref('')
const compareUserId = ref('__none__')
const compareUserGroupId = ref('__none__')

const leaderboard = ref<LeaderboardItem[]>([])
const leaderboardTotal = ref(0)
const leaderboardOffset = ref(0)
const leaderboardLoading = ref(false)
const memberLeaderboard = ref<LeaderboardItem[]>([])
const memberLeaderboardLoading = ref(false)
const groupMemberCount = ref(0)
const activeGroupMemberCount = ref(0)
const usageSummary = ref<UsageSummary | null>(null)
const summaryLoading = ref(false)
const series = ref<TimeSeriesItem[]>([])
const comparisonSeries = ref<TimeSeriesItem[]>([])
const seriesLoading = ref(false)

let leaderboardRequestId = 0
let panelRequestId = 0
let leaderboardDebounceTimer: ReturnType<typeof setTimeout> | null = null
let panelDebounceTimer: ReturnType<typeof setTimeout> | null = null
let ready = false

const allEntities = computed<SelectableEntity[]>(() => scope.value === 'user'
  ? users.value.map(user => ({ id: user.id, name: user.username || user.email || user.id }))
  : [...userGroups.value.map(group => ({ id: group.id, name: group.name })),
    { id: '__ungrouped__', name: t('userStats.ungrouped') }])

const selectedEntityId = computed({
  get: () => scope.value === 'user' ? selectedUserId.value : selectedUserGroupId.value,
  set: (value: string) => {
    if (scope.value === 'user') selectedUserId.value = value
    else selectedUserGroupId.value = value
  }
})

const compareEntityId = computed({
  get: () => scope.value === 'user' ? compareUserId.value : compareUserGroupId.value,
  set: (value: string) => {
    if (scope.value === 'user') compareUserId.value = value
    else compareUserGroupId.value = value
  }
})

const comparisonEntities = computed(() => allEntities.value.filter(
  entity => entity.id !== selectedEntityId.value
))
const selectedEntityName = computed(() => allEntities.value.find(
  entity => entity.id === selectedEntityId.value
)?.name ?? '')
const comparedEntityName = computed(() => allEntities.value.find(
  entity => entity.id === compareEntityId.value
)?.name ?? '')
const currentPage = computed(() => Math.floor(leaderboardOffset.value / PAGE_SIZE) + 1)
const hasNextLeaderboardPage = computed(
  () => leaderboardOffset.value + leaderboard.value.length < leaderboardTotal.value
)

function buildTimeRangeParams() {
  return {
    start_date: timeRange.value.start_date,
    end_date: timeRange.value.end_date,
    preset: timeRange.value.preset,
    timezone: timeRange.value.timezone,
    tz_offset_minutes: timeRange.value.tz_offset_minutes,
    granularity: timeRange.value.granularity || 'day'
  }
}

function scopeParams(id: string) {
  return scope.value === 'user' ? { user_id: id } : { user_group_id: id }
}

function ensureSelectedEntity() {
  const entities = allEntities.value
  if (!entities.some(entity => entity.id === selectedEntityId.value)) {
    selectedEntityId.value = entities[0]?.id ?? ''
  }
  if (compareEntityId.value !== '__none__' && !entities.some(entity => entity.id === compareEntityId.value)) {
    compareEntityId.value = '__none__'
  }
}

async function loadEntities() {
  const [loadedUsers, groupsResponse] = await Promise.all([
    usersApi.getAllUsers(),
    usersApi.listUserGroups()
  ])
  users.value = loadedUsers
  userGroups.value = groupsResponse.items
  ensureSelectedEntity()
}

async function loadLeaderboard() {
  const requestId = ++leaderboardRequestId
  leaderboardLoading.value = true
  try {
    const params = {
      ...buildTimeRangeParams(),
      metric: metric.value,
      limit: PAGE_SIZE,
      offset: leaderboardOffset.value
    }
    const response = scope.value === 'user'
      ? await adminApi.getLeaderboardUsers(params)
      : await adminApi.getLeaderboardUserGroups(params)
    if (requestId !== leaderboardRequestId) return
    leaderboard.value = response.items
    leaderboardTotal.value = response.total
  } finally {
    if (requestId === leaderboardRequestId) leaderboardLoading.value = false
  }
}

async function loadPanels() {
  const selectedId = selectedEntityId.value
  const requestId = ++panelRequestId
  if (!selectedId) {
    usageSummary.value = null
    series.value = []
    comparisonSeries.value = []
    memberLeaderboard.value = []
    groupMemberCount.value = 0
    activeGroupMemberCount.value = 0
    return
  }
  summaryLoading.value = true
  seriesLoading.value = true
  memberLeaderboardLoading.value = scope.value === 'user_group'
  try {
    const primaryParams = { ...buildTimeRangeParams(), ...scopeParams(selectedId) }
    const shouldCompare = compareEntityId.value !== '__none__'
    const comparisonPromise: Promise<TimeSeriesItem[]> = shouldCompare
      ? adminApi.getTimeSeries({
        ...buildTimeRangeParams(),
        ...scopeParams(compareEntityId.value)
      })
      : Promise.resolve([])
    const memberPromise: Promise<{ items: LeaderboardItem[] }> = scope.value === 'user_group'
      ? adminApi.getLeaderboardUsers({
        ...buildTimeRangeParams(),
        metric: metric.value,
        user_group_id: selectedId,
        limit: PAGE_SIZE
      })
      : Promise.resolve({ items: [] })
    const groupMembersPromise: Promise<UserGroupMember[]> = scope.value === 'user_group' && selectedId !== '__ungrouped__'
      ? usersApi.listUserGroupMembers(selectedId)
      : Promise.resolve([])

    const [summary, primarySeries, compareSeries, members, groupMembers] = await Promise.all([
      usageApi.getUsageStats(primaryParams),
      adminApi.getTimeSeries(primaryParams),
      comparisonPromise,
      memberPromise,
      groupMembersPromise
    ])
    if (requestId !== panelRequestId) return
    usageSummary.value = { ...summary, error_rate: summary.error_rate ?? 0 }
    series.value = primarySeries
    comparisonSeries.value = compareSeries
    memberLeaderboard.value = members.items
    const ungroupedUsers = users.value.filter(user => user.groups?.length === 0)
    groupMemberCount.value = selectedId === '__ungrouped__'
      ? ungroupedUsers.length : groupMembers.filter(member => !member.is_deleted).length
    activeGroupMemberCount.value = selectedId === '__ungrouped__'
      ? ungroupedUsers.filter(user => user.is_active).length
      : groupMembers.filter(member => !member.is_deleted && member.is_active).length
  } finally {
    if (requestId === panelRequestId) {
      summaryLoading.value = false
      seriesLoading.value = false
      memberLeaderboardLoading.value = false
    }
  }
}

function selectLeaderboardItem(item: LeaderboardItem) {
  selectedEntityId.value = item.id
}

function selectMember(item: LeaderboardItem) {
  scope.value = 'user'
  selectedUserId.value = item.id
}

function changeLeaderboardPage(direction: -1 | 1) {
  leaderboardOffset.value = Math.max(0, leaderboardOffset.value + direction * PAGE_SIZE)
  void loadLeaderboard()
}

const seriesChartData = computed(() => ({
  labels: series.value.map(item => item.date),
  datasets: [{
    label: t('stats.metric.cost'),
    data: series.value.map(item => item.total_cost),
    borderColor: 'rgb(59, 130, 246)',
    tension: 0.25,
    pointRadius: 2
  }]
}))

const comparisonChartData = computed(() => ({
  labels: series.value.map(item => item.date),
  datasets: [
    {
      label: selectedEntityName.value || t('userStats.chart.current'),
      data: series.value.map(item => item.total_cost),
      borderColor: 'rgb(59, 130, 246)',
      tension: 0.25,
      pointRadius: 2
    },
    {
      label: comparedEntityName.value || t('userStats.chart.comparison'),
      data: comparisonSeries.value.map(item => item.total_cost),
      borderColor: 'rgb(234, 179, 8)',
      tension: 0.25,
      pointRadius: 2
    }
  ]
}))

function scheduleLeaderboardLoad() {
  if (!ready) return
  if (leaderboardDebounceTimer) clearTimeout(leaderboardDebounceTimer)
  leaderboardDebounceTimer = setTimeout(() => {
    leaderboardDebounceTimer = null
    void loadLeaderboard()
  }, 120)
}

function schedulePanelLoad() {
  if (!ready) return
  if (panelDebounceTimer) clearTimeout(panelDebounceTimer)
  panelDebounceTimer = setTimeout(() => {
    panelDebounceTimer = null
    void loadPanels()
  }, 120)
}

watch(scope, () => {
  leaderboardOffset.value = 0
  ensureSelectedEntity()
  scheduleLeaderboardLoad()
  schedulePanelLoad()
})
watch([timeRange, metric], () => {
  leaderboardOffset.value = 0
  scheduleLeaderboardLoad()
  schedulePanelLoad()
}, { deep: true })
watch([selectedEntityId, compareEntityId], schedulePanelLoad)

onMounted(async () => {
  await loadEntities()
  ready = true
  await Promise.all([loadLeaderboard(), loadPanels()])
})

onUnmounted(() => {
  if (leaderboardDebounceTimer) clearTimeout(leaderboardDebounceTimer)
  if (panelDebounceTimer) clearTimeout(panelDebounceTimer)
  leaderboardRequestId += 1
  panelRequestId += 1
})
</script>
