<template>
  <Dialog
    :open="open"
    :title="legacyT('连接事件')"
    :description="description"
    size="lg"
    @update:open="$emit('update:open', $event)"
  >
    <div class="space-y-3">
      <div
        v-if="node"
        class="grid grid-cols-3 gap-3 text-sm"
      >
        <div class="bg-muted/40 rounded-lg px-3 py-2 text-center">
          <span class="block text-foreground/60 text-xs">{{ legacyT('失败请求') }}</span>
          <span class="tabular-nums font-medium">{{ formatProxyNodeNumber(node.failed_requests || 0) }}</span>
        </div>
        <div class="bg-muted/40 rounded-lg px-3 py-2 text-center">
          <span class="block text-foreground/60 text-xs">{{ legacyT('DNS 失败') }}</span>
          <span class="tabular-nums font-medium">{{ formatProxyNodeNumber(node.dns_failures || 0) }}</span>
        </div>
        <div class="bg-muted/40 rounded-lg px-3 py-2 text-center">
          <span class="block text-foreground/60 text-xs">{{ legacyT('流错误') }}</span>
          <span class="tabular-nums font-medium">{{ formatProxyNodeNumber(node.stream_errors || 0) }}</span>
        </div>
      </div>

      <div
        v-if="loading"
        class="py-8 text-center text-muted-foreground text-sm"
      >
        {{ legacyT('加载中...') }}
      </div>
      <div
        v-else-if="events.length === 0"
        class="py-8 text-center text-muted-foreground text-sm"
      >
        {{ legacyT('暂无连接事件记录') }}
      </div>
      <div
        v-else
        class="max-h-80 overflow-y-auto space-y-1.5"
      >
        <div
          v-for="event in events"
          :key="event.id"
          class="flex items-center gap-2 px-3 py-2 rounded-lg bg-muted/30 text-sm"
        >
          <Badge
            :variant="proxyNodeEventTypeVariant(event.event_type)"
            class="text-[10px] px-1.5 py-0 shrink-0"
          >
            {{ legacyT(proxyNodeEventTypeLabel(event.event_type)) }}
          </Badge>
          <span class="text-muted-foreground truncate flex-1">{{ proxyNodeEventDetail(event) }}</span>
          <span class="text-xs text-muted-foreground/70 tabular-nums shrink-0">{{ formatProxyNodeTime(event.created_at, locale) }}</span>
        </div>
      </div>
    </div>
    <template #footer>
      <Button
        variant="outline"
        @click="$emit('update:open', false)"
      >
        {{ legacyT('关闭') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Badge, Button, Dialog } from '@/components/ui'
import type { ProxyNode, ProxyNodeEvent } from '@/api/proxy-nodes'
import { useI18n } from '@/i18n'
import {
  formatProxyNodeNumber,
  formatProxyNodeTime,
  proxyNodeEventDetail,
  proxyNodeEventTypeLabel,
  proxyNodeEventTypeVariant,
} from './proxy-node-display'

const props = defineProps<{
  open: boolean
  node: ProxyNode | null
  events: ProxyNodeEvent[]
  loading: boolean
}>()

defineEmits<{
  'update:open': [value: boolean]
}>()

const { legacyT, locale } = useI18n()

const description = computed(() => {
  if (!props.node) return ''
  return locale.value === 'en-US'
    ? `${props.node.name} connection history`
    : `${props.node.name} 的连接历史`
})
</script>
