<template>
  <CardSection
    title="监控容量"
    description="为远程或托管 Redis / PostgreSQL 手动填写总容量，便于监控面板计算剩余空间"
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
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <div>
        <Label
          for="redis-memory-total-gb"
          class="block text-sm font-medium"
        >
          Redis 总内存 (GB)
        </Label>
        <Input
          id="redis-memory-total-gb"
          :model-value="redisMemoryTotalGB"
          type="number"
          step="0.1"
          min="0"
          placeholder="0"
          class="mt-1"
          @update:model-value="$emit('update:redisMemoryTotalGB', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          用于远程 Redis 未上报完整容量时计算剩余内存，0 表示继续自动探测
        </p>
      </div>

      <div>
        <Label
          for="postgres-storage-total-gb"
          class="block text-sm font-medium"
        >
          PostgreSQL 总空间 (GB)
        </Label>
        <Input
          id="postgres-storage-total-gb"
          :model-value="postgresStorageTotalGB"
          type="number"
          step="0.1"
          min="0"
          placeholder="0"
          class="mt-1"
          @update:model-value="$emit('update:postgresStorageTotalGB', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          监控面板会用“总空间 - 当前数据库体积”估算剩余空间，0 表示未设置
        </p>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import { CardSection } from '@/components/layout'

defineProps<{
  redisMemoryTotalGB: number
  postgresStorageTotalGB: number
  loading: boolean
  hasChanges: boolean
}>()

defineEmits<{
  save: []
  'update:redisMemoryTotalGB': [value: number]
  'update:postgresStorageTotalGB': [value: number]
}>()
</script>
