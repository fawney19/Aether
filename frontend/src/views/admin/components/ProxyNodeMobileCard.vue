<template>
  <div class="p-4 sm:p-5">
    <div class="flex items-start justify-between mb-2">
      <div>
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class="inline-flex h-5 w-5 items-center justify-center rounded-md text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 shrink-0"
            :title="expanded ? legacyT('收起数据') : legacyT('展开数据')"
            @click="$emit('toggle-details')"
          >
            <ChevronDown
              v-if="expanded"
              class="h-3.5 w-3.5"
            />
            <ChevronRight
              v-else
              class="h-3.5 w-3.5"
            />
          </button>
          <span class="font-semibold text-sm">{{ node.name }}</span>
          <Badge
            v-if="node.is_manual"
            variant="outline"
            class="text-[10px] px-1.5 py-0"
          >
            {{ legacyT('手动') }}
          </Badge>
          <Badge
            v-if="node.tunnel_mode"
            variant="outline"
            class="text-[10px] px-1.5 py-0"
          >
            Tunnel
          </Badge>
          <Badge
            v-if="schedulingBadge"
            :variant="schedulingBadge.variant"
            class="text-[10px] px-1.5 py-0"
          >
            {{ legacyT(schedulingBadge.label) }}
          </Badge>
          <HardwareTooltip :node="node" />
        </div>
        <code class="text-xs text-muted-foreground">{{ proxyNodeAddress(node) }}</code>
        <div
          v-if="!node.is_manual"
          class="text-[11px] text-muted-foreground mt-1"
        >
          {{ legacyT('版本:') }} {{ proxyNodeVersion(node) }}
        </div>
      </div>
      <Badge
        :variant="proxyNodeStatusVariant(node.status)"
        :title="legacyT(proxyNodeStatusTitle(node))"
        class="text-xs"
      >
        {{ legacyT(proxyNodeStatusLabel(node)) }}
      </Badge>
    </div>

    <div class="grid grid-cols-4 gap-2 text-xs text-muted-foreground mb-3">
      <div>
        <span class="block text-foreground/60">{{ legacyT('区域') }}</span>
        <span>{{ legacyT(formatProxyNodeRegion(node.region)) }}</span>
      </div>
      <div>
        <span class="block text-foreground/60">{{ legacyT('总请求') }}</span>
        <span class="tabular-nums">{{ formatProxyNodeNumber(node.total_requests) }}</span>
      </div>
      <div>
        <span class="block text-foreground/60">{{ legacyT('失败率') }}</span>
        <span
          class="tabular-nums"
          :class="proxyNodeFailureRate(node) > 5 ? 'text-destructive font-medium' : ''"
        >{{ formatProxyNodeFailureRate(node) }}</span>
      </div>
      <div>
        <span class="block text-foreground/60">{{ legacyT('延迟') }}</span>
        <span class="tabular-nums">{{ node.avg_latency_ms != null ? `${node.avg_latency_ms.toFixed(0)}ms` : '-' }}</span>
      </div>
    </div>

    <div class="flex items-center justify-between">
      <span class="text-xs text-muted-foreground">{{ formatProxyNodeTime(node.last_heartbeat_at, locale) }}</span>
      <div class="flex flex-wrap items-center justify-end gap-1">
        <Button
          variant="ghost"
          size="sm"
          class="h-7 px-2 text-xs"
          :disabled="testing"
          @click="$emit('test')"
        >
          <Loader2
            v-if="testing"
            class="h-3 w-3 mr-1 animate-spin"
          />
          <Activity
            v-else
            class="h-3 w-3 mr-1"
          />
          {{ testing ? legacyT('测试中') : legacyT('测试') }}
        </Button>
        <Button
          v-if="node.is_manual"
          variant="ghost"
          size="sm"
          class="h-7 px-2 text-xs"
          @click="$emit('edit')"
        >
          <SquarePen class="h-3 w-3 mr-1" />
          {{ legacyT('编辑') }}
        </Button>
        <Button
          v-if="!node.is_manual"
          variant="ghost"
          size="sm"
          class="h-7 px-2 text-xs"
          @click="$emit('config')"
        >
          <Settings class="h-3 w-3 mr-1" />
          {{ legacyT('配置') }}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          class="h-7 px-2 text-xs"
          @click="$emit('delete')"
        >
          <Trash2 class="h-3 w-3 mr-1" />
          {{ legacyT('删除') }}
        </Button>
      </div>
    </div>

    <div
      v-if="expanded"
      class="mt-4 -mx-4 sm:-mx-5"
    >
      <ProxyNodeDataPanel
        :node="node"
        :state="detailState"
        @refresh="$emit('refresh-details')"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Activity, ChevronDown, ChevronRight, Loader2, Settings, SquarePen, Trash2 } from 'lucide-vue-next'
import { Badge, Button } from '@/components/ui'
import type { ProxyNode } from '@/api/proxy-nodes'
import { useI18n } from '@/i18n'
import HardwareTooltip from './HardwareTooltip.vue'
import ProxyNodeDataPanel from './ProxyNodeDataPanel.vue'
import {
  formatProxyNodeFailureRate,
  formatProxyNodeNumber,
  formatProxyNodeRegion,
  formatProxyNodeTime,
  proxyNodeAddress,
  proxyNodeFailureRate,
  proxyNodeSchedulingBadge,
  proxyNodeStatusLabel,
  proxyNodeStatusTitle,
  proxyNodeStatusVariant,
  proxyNodeVersion,
} from './proxy-node-display'
import type { ProxyNodeDetailState } from './proxy-node-types'

const props = defineProps<{
  node: ProxyNode
  expanded: boolean
  testing: boolean
  detailState?: ProxyNodeDetailState | null
}>()

defineEmits<{
  'toggle-details': []
  'refresh-details': []
  test: []
  edit: []
  config: []
  delete: []
}>()

const { legacyT, locale } = useI18n()
const schedulingBadge = computed(() => proxyNodeSchedulingBadge(props.node))
</script>
