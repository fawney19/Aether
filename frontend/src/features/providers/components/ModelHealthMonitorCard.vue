<template>
  <Card
    variant="default"
    class="overflow-hidden"
  >
    <HealthMonitorHeader
      v-model:lookback-hours="lookbackHours"
      :title="title"
      description="基于真实请求统计模型可用率、平均耗时、平均TTFB 与平均速度"
      :loading="loading"
      @refresh="refreshData"
    />

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
        <Bot class="w-12 h-12 mb-3 opacity-30" />
        <p>暂无模型健康监控数据</p>
        <p class="text-xs mt-1">
          模型尚未产生请求记录
        </p>
      </div>

      <div
        v-else
        class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-4"
      >
        <div
          v-for="monitor in monitors"
          :key="monitor.model"
          class="relative overflow-hidden rounded-xl border border-border/60 bg-card/60 p-4 transition-colors hover:border-primary/50"
        >
          <div
            class="absolute inset-x-0 top-0 h-px bg-gradient-to-r from-transparent via-primary/40 to-transparent"
          />
          <div class="flex items-start justify-between gap-3">
            <div class="flex min-w-0 items-center gap-3">
              <div
                class="flex h-11 w-11 flex-shrink-0 items-center justify-center rounded-xl border border-border/60 bg-muted/50"
              >
                <Bot class="h-5 w-5 text-muted-foreground" />
              </div>
              <h4 class="min-w-0 truncate text-sm font-semibold">
                {{ monitor.display_name || monitor.model }}
              </h4>
            </div>
            <Badge
              :variant="getHealthBadgeVariant(monitor)"
              class="shrink-0"
            >
              {{ getHealthLabel(monitor) }}
            </Badge>
          </div>

          <HealthMetricGrid
            class="mt-4"
            :avg-latency-ms="monitor.avg_latency_ms"
            :avg-first-byte-ms="monitor.avg_first_byte_ms"
            :avg-tps="monitor.avg_tps"
            :total-attempts="monitor.total_attempts"
            :success-rate="monitor.success_rate"
          />

          <div
            class="mt-4 flex items-center justify-between gap-3 text-[11px] uppercase tracking-wide text-muted-foreground"
          >
            <span>History (60pts)</span>
            <span class="truncate normal-case tracking-normal">
              {{ getModelMetaText(monitor) }}
            </span>
          </div>

          <HealthStatusTimeline
            class="mt-2"
            :timeline="monitor.timeline"
            :timeline-details="monitor.timeline_details"
            :time-range-start="monitor.time_range_start"
            :time-range-end="monitor.time_range_end"
            :generated-at="generatedAt"
            :lookback-hours="parseInt(lookbackHours)"
            entity-label="模型"
            :entity-name="monitor.model"
          />

          <div class="mt-4 flex justify-end">
            <Button
              variant="outline"
              size="sm"
              @click="openDetails(monitor)"
            >
              查看详情
            </Button>
          </div>
        </div>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from "vue";
import { Bot, Loader2 } from "lucide-vue-next";
import Card from "@/components/ui/card.vue";
import Badge from "@/components/ui/badge.vue";
import Button from "@/components/ui/button.vue";
import HealthMetricGrid from "./HealthMetricGrid.vue";
import HealthMonitorHeader from "./HealthMonitorHeader.vue";
import HealthStatusTimeline from "./HealthStatusTimeline.vue";
import {
  getModelStatusMonitor,
  getPublicModelStatusMonitor,
} from "@/api/endpoints/health";
import type { ModelStatusMonitor } from "@/api/endpoints/types";
import { useToast } from "@/composables/useToast";
import { parseApiError } from "@/utils/errorParser";
import type {
  HealthMonitorDetailTarget,
  HealthMonitorSectionSummary,
} from "./health-monitor-utils";
import {
  formatCompactNumber,
  getHealthBadgeVariant,
  getHealthLabel,
  summarizeHealthMonitorItems,
} from "./health-monitor-utils";

const props = withDefaults(
  defineProps<{
    title?: string;
    isAdmin?: boolean;
    showProviderInfo?: boolean;
  }>(),
  {
    title: "模型健康监控",
    isAdmin: false,
    showProviderInfo: false,
  },
);

const emit = defineEmits<{
  viewDetails: [target: HealthMonitorDetailTarget];
  summaryUpdated: [summary: HealthMonitorSectionSummary];
}>();

const { error: showError } = useToast();

const loading = ref(false);
const loadingMonitors = ref(false);
const monitors = ref<ModelStatusMonitor[]>([]);
const generatedAt = ref<string | null>(null);
const lookbackHours = ref("6");

async function loadMonitors() {
  loadingMonitors.value = true;
  try {
    const params = {
      lookback_hours: parseInt(lookbackHours.value),
      model_limit: 12,
      per_model_limit: 100,
    };

    const data = props.isAdmin
      ? await getModelStatusMonitor(params)
      : await getPublicModelStatusMonitor(params);
    monitors.value = data.models || [];
    generatedAt.value = data.generated_at || null;
    emitSummary();
  } catch (err: unknown) {
    showError(parseApiError(err, "加载模型健康监控数据失败"), "错误");
  } finally {
    loadingMonitors.value = false;
  }
}

async function refreshData() {
  loading.value = true;
  try {
    await loadMonitors();
  } finally {
    loading.value = false;
  }
}

function getModelMetaText(monitor: ModelStatusMonitor) {
  const attempts = `${formatCompactNumber(monitor.total_attempts)} 次请求`;
  if (props.showProviderInfo && typeof monitor.provider_count === "number") {
    return `${monitor.provider_count} 个提供商 / ${attempts}`;
  }
  return attempts;
}

function openDetails(monitor: ModelStatusMonitor) {
  emit("viewDetails", {
    lookbackHours: parseInt(lookbackHours.value),
    source: {
      kind: "model",
      value: monitor.model,
      title: monitor.display_name || monitor.model,
      metaText: getModelDetailMetaText(monitor),
      totalAttempts: monitor.total_attempts,
      successCount: monitor.success_count,
      failedCount: monitor.failed_count,
      successRate: monitor.success_rate,
      avgLatencyMs: monitor.avg_latency_ms,
      avgFirstByteMs: monitor.avg_first_byte_ms,
      avgTps: monitor.avg_tps,
      timeline: monitor.timeline || null,
      timelineDetails: monitor.timeline_details || null,
      timeRangeStart: monitor.time_range_start || null,
      timeRangeEnd: monitor.time_range_end || null,
    },
  });
}

function getModelDetailMetaText(monitor: ModelStatusMonitor) {
  if (props.showProviderInfo && typeof monitor.provider_count === "number") {
    return `${monitor.provider_count} 个提供商`;
  }
  return null;
}

function emitSummary() {
  emit("summaryUpdated", summarizeHealthMonitorItems(monitors.value));
}

watch(lookbackHours, () => {
  loadMonitors();
});

onMounted(() => {
  refreshData();
});
</script>
