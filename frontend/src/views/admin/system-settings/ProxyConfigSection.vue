<template>
  <CardSection
    title="网络代理"
    description="配置提供商出站请求的默认代理，仅影响大模型 API、余额查询、OAuth 等提供商请求"
  >
    <template #actions>
      <Button
        size="sm"
        :disabled="loading || !hasChanges"
        @click="$emit('save')"
      >
        {{ loading ? '保存中...' : '保存' }}
      </Button>
    </template>
    <div class="max-w-md">
      <Label class="block text-sm font-medium mb-1">
        默认代理节点
      </Label>
      <Select
        :model-value="proxyNodeId || '__direct__'"
        @update:model-value="(v: string) => $emit('update:proxyNodeId', v === '__direct__' ? null : v)"
      >
        <SelectTrigger>
          <SelectValue placeholder="直连（不使用代理）" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="__direct__">
            直连（不使用代理）
          </SelectItem>
          <SelectItem
            v-for="node in selectableNodes"
            :key="node.id"
            :value="node.id"
          >
            {{ node.name }}{{ node.region ? ` · ${node.region}` : '' }} ({{ node.ip }}:{{ node.port }})
          </SelectItem>
        </SelectContent>
      </Select>
      <p class="mt-1 text-xs text-muted-foreground">
        对未单独配置代理的提供商生效，覆盖大模型 API 请求、余额查询、OAuth 刷新等。不影响系统内部接口。
      </p>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import Button from '@/components/ui/button.vue'
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { CardSection } from '@/components/layout'

interface ProxyNode {
  id: string
  name: string
  region?: string | null
  ip: string
  port: number
}

const props = defineProps<{
  proxyNodeId: string | null
  onlineNodes: ProxyNode[]
  allNodes: ProxyNode[]
  loading: boolean
  hasChanges: boolean
}>()

defineEmits<{
  save: []
  'update:proxyNodeId': [value: string | null]
}>()

const selectableNodes = computed(() => {
  if (!props.proxyNodeId) {
    return props.onlineNodes
  }
  const exists = props.onlineNodes.some(node => node.id === props.proxyNodeId)
  if (exists) {
    return props.onlineNodes
  }
  const selected = props.allNodes.find(node => node.id === props.proxyNodeId)
  return selected ? [selected, ...props.onlineNodes] : props.onlineNodes
})
</script>
