<template>
  <section class="space-y-4">
    <RoutingModelPolicyEditor
      :model-policies="config.model_policies"
      @update:model-policies="updateModelPolicies"
    />
  </section>
</template>

<script setup lang="ts">
import { computed } from 'vue'

import RoutingModelPolicyEditor from './RoutingModelPolicyEditor.vue'
import { normalizeRoutingGroupConfig, type RoutingGroupConfig, type RoutingModelPolicy } from '../utils/routingPolicy'

const props = defineProps<{
  config: RoutingGroupConfig
}>()

const emit = defineEmits<{
  'update:config': [value: RoutingGroupConfig]
}>()

const config = computed(() => normalizeRoutingGroupConfig(props.config))

function updateModelPolicies(modelPolicies: RoutingModelPolicy[]) {
  emit('update:config', {
    ...config.value,
    model_policies: modelPolicies,
  })
}
</script>
