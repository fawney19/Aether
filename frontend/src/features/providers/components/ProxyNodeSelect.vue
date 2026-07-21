<template>
  <div class="space-y-1.5">
    <Select
      :model-value="modelValue"
      :disabled="proxyNodesStore.loading || nodeOptions.length === 0"
      @update:model-value="(v: string) => $emit('update:modelValue', v)"
    >
      <SelectTrigger :class="triggerClass">
        <SelectValue
          :placeholder="proxyNodesStore.loading
            ? legacyT('加载节点列表中...')
            : nodeOptions.length === 0
              ? legacyT('暂无可用节点')
              : legacyT('选择代理节点...')"
        />
      </SelectTrigger>
      <SelectContent>
        <SelectItem
          v-for="node in nodeOptions"
          :key="node.id"
          :value="node.id"
        >
          {{ node.name }}{{ node.region ? ` · ${formatRegion(node.region, '')}` : '' }} ({{ node.ip }}:{{ node.port }})
        </SelectItem>
      </SelectContent>
    </Select>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted } from 'vue'
import {
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
} from '@/components/ui'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import { useI18n } from '@/i18n'
import { formatRegion } from '@/utils/region'

const props = defineProps<{
  modelValue: string
  triggerClass?: string
}>()

defineEmits<{
  'update:modelValue': [value: string]
}>()

const proxyNodesStore = useProxyNodesStore()
const { legacyT } = useI18n()

/** 在线节点 + 保留当前已选节点（可能已离线） */
const nodeOptions = computed(() => {
  const online = proxyNodesStore.onlineNodes
  if (props.modelValue) {
    const found = online.find(n => n.id === props.modelValue)
    if (!found) {
      const allNode = proxyNodesStore.nodes.find(n => n.id === props.modelValue)
      if (allNode) return [allNode, ...online]
    }
  }
  return online
})

/** 供父组件调用：启用代理时懒加载节点列表 */
function ensureLoaded() {
  return proxyNodesStore.ensureLoaded()
}

onMounted(ensureLoaded)

defineExpose({ ensureLoaded })
</script>
