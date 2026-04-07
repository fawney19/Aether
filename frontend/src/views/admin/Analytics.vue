<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0">
    <div class="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
      <div class="min-w-0">
        <h1 class="text-lg font-semibold">
          数据分析
        </h1>
        <p class="text-xs text-muted-foreground">
          用量报表、模型报表与全局性能洞察
        </p>
      </div>
    </div>

    <!-- Tabs -->
    <Tabs v-model="filters.activeTab.value">
      <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <TabsList class="tabs-button-list">
          <TabsTrigger value="detail">
            用量报表
          </TabsTrigger>
          <TabsTrigger value="models">
            模型报表
          </TabsTrigger>
          <TabsTrigger value="leaderboard">
            排行榜
          </TabsTrigger>
          <TabsTrigger value="performance">
            性能
          </TabsTrigger>
        </TabsList>

        <div class="flex flex-wrap items-center gap-2 sm:justify-end">
          <TimeRangePicker
            v-model="filters.timeRange.value"
            :show-granularity="false"
            :include-auto-granularity="true"
            :compact="true"
          />
          <template v-if="filters.activeTab.value === 'leaderboard'">
            <Select v-model="leaderboardDimension">
              <SelectTrigger class="h-8 w-full text-xs sm:w-32">
                <SelectValue placeholder="维度" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="users">
                  用户
                </SelectItem>
                <SelectItem value="api_keys">
                  API Key
                </SelectItem>
              </SelectContent>
            </Select>
          </template>
          <template v-else-if="showEntityFilters">
            <Select v-model="selectedUserValue">
              <SelectTrigger class="h-8 w-full text-xs sm:w-36">
                <SelectValue placeholder="全部用户">
                  <span
                    class="block min-w-0 truncate"
                    :title="selectedUserLabel"
                  >
                    {{ selectedUserLabel }}
                  </span>
                </SelectValue>
              </SelectTrigger>
              <SelectContent class="w-[min(18rem,var(--radix-select-trigger-width))]">
                <SelectItem value="__all__">
                  全部用户
                </SelectItem>
                <SelectItem
                  v-for="option in userOptions"
                  :key="option.value"
                  :value="option.value"
                  :text-value="option.label"
                >
                  {{ option.label }}
                </SelectItem>
              </SelectContent>
            </Select>
            <Select v-model="selectedProviderValue">
              <SelectTrigger class="h-8 w-full text-xs sm:w-36">
                <SelectValue placeholder="全部供应商">
                  <span
                    class="block min-w-0 truncate"
                    :title="selectedProviderLabel"
                  >
                    {{ selectedProviderLabel }}
                  </span>
                </SelectValue>
              </SelectTrigger>
              <SelectContent class="w-[min(18rem,var(--radix-select-trigger-width))]">
                <SelectItem value="__all__">
                  全部供应商
                </SelectItem>
                <SelectItem
                  v-for="option in providerOptions"
                  :key="option.value"
                  :value="option.value"
                  :text-value="option.label"
                >
                  {{ option.label }}
                </SelectItem>
              </SelectContent>
            </Select>
            <Select v-model="selectedApiKeyValue">
              <SelectTrigger class="h-8 w-full text-xs sm:w-36">
                <SelectValue placeholder="全部 Key">
                  <span
                    class="block min-w-0 truncate"
                    :title="selectedApiKeyLabel"
                  >
                    {{ selectedApiKeyLabel }}
                  </span>
                </SelectValue>
              </SelectTrigger>
              <SelectContent class="w-[min(18rem,var(--radix-select-trigger-width))]">
                <SelectItem value="__all__">
                  全部 Key
                </SelectItem>
                <SelectItem
                  v-for="option in apiKeyOptions"
                  :key="option.value"
                  :value="option.value"
                  :text-value="option.label"
                >
                  {{ option.label }}
                </SelectItem>
              </SelectContent>
            </Select>
          </template>
        </div>
      </div>

      <div class="mt-4">
        <ReportsTab
          v-if="isReportsTab"
          :mode="reportMode"
          :selected-user-label="selectedUserExportLabel"
          :selected-api-key-label="selectedApiKeyExportLabel"
        />
        <LeaderboardTab
          v-else-if="filters.activeTab.value === 'leaderboard'"
          :dimension="leaderboardDimension"
        />
        <PerformanceTab v-else-if="filters.activeTab.value === 'performance'" />
      </div>
    </Tabs>
  </div>
</template>

<script setup lang="ts">
import { computed, provide, ref, watch, onMounted } from 'vue'
import { Tabs, TabsList, TabsTrigger, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui'
import { TimeRangePicker } from '@/components/common'
import { analyticsApi, type AnalyticsFilterOption } from '@/api/analytics'
import { useAnalyticsFilters } from '@/composables/useAnalyticsFilters'
import type { LeaderboardDimension } from '@/composables/analytics/useLeaderboardData'
import ReportsTab from './components/analytics/ReportsTab.vue'
import LeaderboardTab from './components/analytics/LeaderboardTab.vue'
import PerformanceTab from './components/analytics/PerformanceTab.vue'

const filters = useAnalyticsFilters()
provide('analyticsFilters', filters)

const userOptions = ref<AnalyticsFilterOption[]>([])
const providerOptions = ref<AnalyticsFilterOption[]>([])
const apiKeyOptions = ref<AnalyticsFilterOption[]>([])
const leaderboardDimension = ref<LeaderboardDimension>('users')
const supportedTabs = new Set(['detail', 'models', 'leaderboard', 'performance'])
const isReportsTab = computed(() => filters.activeTab.value === 'detail' || filters.activeTab.value === 'models')
const showEntityFilters = computed(() => isReportsTab.value)
const reportMode = computed<'detail' | 'models'>(() => (
  filters.activeTab.value === 'models' ? 'models' : 'detail'
))
let suppressNextUserFilterLoad = false
let suppressNextProviderFilterLoad = false
let suppressNextApiKeyFilterLoad = false
const selectedUserValue = computed({
  get: () => filters.userFilter.value[0] ?? '__all__',
  set: (value: string) => {
    filters.userFilter.value = value && value !== '__all__' ? [value] : []
  },
})
const selectedUserLabel = computed(() => (
  selectedUserValue.value === '__all__'
    ? '全部用户'
    : userOptions.value.find(option => option.value === selectedUserValue.value)?.label || '全部用户'
))
const selectedProviderValue = computed({
  get: () => filters.providerFilter.value[0] ?? '__all__',
  set: (value: string) => {
    filters.providerFilter.value = value && value !== '__all__' ? [value] : []
  },
})
const selectedProviderLabel = computed(() => (
  selectedProviderValue.value === '__all__'
    ? '全部供应商'
    : providerOptions.value.find(option => option.value === selectedProviderValue.value)?.label || '全部供应商'
))
const selectedApiKeyValue = computed({
  get: () => filters.apiKeyFilter.value[0] ?? '__all__',
  set: (value: string) => {
    filters.apiKeyFilter.value = value && value !== '__all__' ? [value] : []
  },
})
const selectedApiKeyLabel = computed(() => (
  selectedApiKeyValue.value === '__all__'
    ? '全部 Key'
    : apiKeyOptions.value.find(option => option.value === selectedApiKeyValue.value)?.label || '全部 Key'
))
const selectedUserExportLabel = computed(() => (
  selectedUserValue.value === '__all__' ? '' : selectedUserLabel.value
))
const selectedApiKeyExportLabel = computed(() => (
  selectedApiKeyValue.value === '__all__' ? '' : selectedApiKeyLabel.value
))

function hasOption(options: AnalyticsFilterOption[], value: string): boolean {
  return options.some(option => option.value === value)
}

function isSelectableProviderOption(option: AnalyticsFilterOption): boolean {
  return option.value !== 'pending' && option.value !== 'unknown'
}

watch(
  () => filters.activeTab.value,
  (tab) => {
    if (!supportedTabs.has(tab)) {
      filters.activeTab.value = 'detail'
    }
  },
  { immediate: true },
)

async function loadFilterOptions() {
  if (!showEntityFilters.value) return
  const response = await analyticsApi.getFilterOptions({
    scope: { kind: 'global' },
    time_range: filters.getTimeRangeParams(),
    filters: {
      user_ids: filters.userFilter.value,
      provider_names: filters.providerFilter.value,
      api_key_ids: filters.apiKeyFilter.value,
    },
  }).catch(() => null)

  if (!response) return
  userOptions.value = response.users ?? []
  providerOptions.value = (response.providers ?? []).filter(isSelectableProviderOption)
  apiKeyOptions.value = response.api_keys ?? []

  if (filters.userFilter.value.length > 0 && !hasOption(userOptions.value, filters.userFilter.value[0])) {
    suppressNextUserFilterLoad = true
    filters.userFilter.value = []
  }
  if (filters.providerFilter.value.length > 0 && !hasOption(providerOptions.value, filters.providerFilter.value[0])) {
    suppressNextProviderFilterLoad = true
    filters.providerFilter.value = []
  }
  if (filters.apiKeyFilter.value.length > 0 && !hasOption(apiKeyOptions.value, filters.apiKeyFilter.value[0])) {
    suppressNextApiKeyFilterLoad = true
    filters.apiKeyFilter.value = []
  }
}

watch(
  () => [
    showEntityFilters.value,
    filters.getTimeRangeParams(),
    filters.userFilter.value.slice(),
    filters.providerFilter.value.slice(),
    filters.apiKeyFilter.value.slice(),
  ],
  ([shouldShow]) => {
    if (!shouldShow) return
    if (suppressNextUserFilterLoad) {
      suppressNextUserFilterLoad = false
      return
    }
    if (suppressNextProviderFilterLoad) {
      suppressNextProviderFilterLoad = false
      return
    }
    if (suppressNextApiKeyFilterLoad) {
      suppressNextApiKeyFilterLoad = false
      return
    }
    void loadFilterOptions()
  },
  { deep: true },
)

onMounted(() => {
  if (showEntityFilters.value) {
    void loadFilterOptions()
  }
})
</script>
