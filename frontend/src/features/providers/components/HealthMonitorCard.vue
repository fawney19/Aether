<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <!-- 标题和筛选器 -->
    <div class="px-6 py-3.5 border-b border-border/60">
      <div class="flex items-center justify-between gap-4">
        <div>
          <h3 class="text-base font-semibold">
            {{ title }}
          </h3>
          <p class="mt-1 text-xs text-muted-foreground">
            基于真实请求统计端点可用率、请求成功率与健康历史
          </p>
        </div>
        <div class="flex items-center gap-3">
          <Label class="text-xs text-muted-foreground">回溯时间：</Label>
          <Select
            v-model="lookbackHours"
          >
            <SelectTrigger class="w-28 h-8 text-xs border-border/60">
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
          <RefreshButton
            :loading="loading"
            @click="refreshData"
          />
        </div>
      </div>
    </div>

    <!-- 内容区域 -->
    <div class="p-6">
      <div
        v-if="loadingMonitors"
        class="flex items-center justify-center py-12"
      >
        <Loader2 class="w-6 h-6 animate-spin text-muted-foreground" />
        <span class="ml-2 text-muted-foreground">加载中...</span>
      </div>

      <div
        v-else-if="monitors.length === 0"
        class="flex flex-col items-center justify-center py-12 text-muted-foreground"
      >
        <Activity class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无健康监控数据</p>
        <p class="text-xs mt-1">
          端点尚未产生请求记录
        </p>
      </div>

      <div
        v-else
        class="space-y-3"
      >
        <div
          v-for="monitor in monitors"
          :key="monitor.api_format"
          class="border border-border/60 rounded-lg p-4 hover:border-primary/50 transition-colors"
        >
          <!-- 响应式布局：窄屏上下两行，宽屏左右结构 -->
          <div class="flex flex-col sm:flex-row sm:gap-6 sm:items-center">
            <!-- 第一行/左侧：信息区域 -->
            <div class="sm:w-52 flex-shrink-0 space-y-1.5 mb-3 sm:mb-0">
              <!-- API 格式标签和成功率 -->
              <div class="flex items-center gap-2 flex-wrap">
                <Badge
                  variant="outline"
                  class="font-mono text-xs whitespace-nowrap"
                >
                  {{ formatApiFormat(monitor.api_format) }}
                </Badge>
                <Badge
                  v-if="monitor.total_attempts > 0"
                  :variant="getSuccessRateVariant(monitor.success_rate)"
                  class="text-xs whitespace-nowrap"
                >
                  {{ (monitor.success_rate * 100).toFixed(0) }}%
                </Badge>
                <!-- 提供商信息（仅管理员可见）- 窄屏时显示在同一行 -->
                <span
                  v-if="showProviderInfo && 'provider_count' in monitor"
                  class="text-xs text-muted-foreground sm:hidden"
                >
                  {{ monitor.provider_count }} 个提供商 / {{ monitor.key_count }} 个密钥
                </span>
              </div>

              <!-- 提供商信息（仅管理员可见）- 宽屏时显示在下方 -->
              <div
                v-if="showProviderInfo && 'provider_count' in monitor"
                class="text-xs text-muted-foreground hidden sm:block"
              >
                {{ monitor.provider_count }} 个提供商 / {{ monitor.key_count }} 个密钥
              </div>
            </div>

            <!-- 第二行/右侧：时间线区域 -->
            <div class="flex-1 min-w-0 sm:flex sm:justify-end">
              <div class="w-full sm:max-w-5xl">
                <EndpointHealthTimeline
                  :monitor="monitor"
                  :lookback-hours="parseInt(lookbackHours)"
                />
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { Activity, Loader2 } from 'lucide-vue-next'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import EndpointHealthTimeline from './EndpointHealthTimeline.vue'
import { getEndpointStatusMonitor, getPublicEndpointStatusMonitor } from '@/api/endpoints/health'
import type { EndpointStatusMonitor, PublicEndpointStatusMonitor } from '@/api/endpoints/types'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { formatApiFormat } from '@/api/endpoints/types/api-format'

const props = withDefaults(defineProps<{
  title?: string
  isAdmin?: boolean
  showProviderInfo?: boolean
}>(), {
  title: '健康监控',
  isAdmin: false,
  showProviderInfo: false
})

const { error: showError } = useToast()

const loading = ref(false)
const loadingMonitors = ref(false)
const monitors = ref<(EndpointStatusMonitor | PublicEndpointStatusMonitor)[]>([])
const lookbackHours = ref('6')

async function loadMonitors() {
  loadingMonitors.value = true
  try {
    const params = {
      lookback_hours: parseInt(lookbackHours.value),
      per_format_limit: 100
    }

    if (props.isAdmin) {
      const data = await getEndpointStatusMonitor(params)
      monitors.value = data.formats || []
    } else {
      const data = await getPublicEndpointStatusMonitor(params)
      monitors.value = data.formats || []
    }
  } catch (err: unknown) {
    showError(parseApiError(err, '加载健康监控数据失败'), '错误')
  } finally {
    loadingMonitors.value = false
  }
}

async function refreshData() {
  loading.value = true
  try {
    await loadMonitors()
  } finally {
    loading.value = false
  }
}

function getSuccessRateVariant(rate: number): 'default' | 'secondary' | 'destructive' | 'outline' {
  if (rate >= 0.95) return 'default'
  if (rate >= 0.8) return 'secondary'
  return 'destructive'
}

watch(lookbackHours, () => {
  loadMonitors()
})

onMounted(() => {
  refreshData()
})
</script>
