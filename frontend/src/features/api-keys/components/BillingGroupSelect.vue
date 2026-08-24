<script setup lang="ts">
import { computed } from 'vue'
import {
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import type { UserBillingGroup } from '@/api/me'

const props = defineProps<{
  modelValue: string
  groups: UserBillingGroup[]
}>()

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

const selectedGroup = computed(() => props.groups.find(group => group.id === props.modelValue))
</script>

<template>
  <div class="space-y-2">
    <Label for="key-billing-group" class="text-sm font-semibold">计费用户组</Label>
    <Select :model-value="modelValue" @update:model-value="value => emit('update:modelValue', String(value))">
      <SelectTrigger id="key-billing-group" class="h-11 border-border/60">
        <SelectValue placeholder="请选择计费用户组" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem v-for="group in groups" :key="group.id" :value="group.id">
          {{ group.name }}（{{ group.sell_rate_multiplier }}×）
        </SelectItem>
      </SelectContent>
    </Select>
    <p class="text-xs text-muted-foreground">
      {{ selectedGroup ? `实际扣费为目录标价的 ${selectedGroup.sell_rate_multiplier} 倍` : '该密钥必须绑定一个当前有效用户组' }}
    </p>
  </div>
</template>
