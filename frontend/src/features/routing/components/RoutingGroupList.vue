<template>
  <div class="space-y-3">
    <div
      v-for="group in groups"
      :key="group.id"
      class="rounded-lg border border-border/60 bg-background px-4 py-3"
    >
      <div class="flex items-center justify-between gap-3">
        <div class="min-w-0">
          <p class="truncate text-sm font-medium text-foreground">
            {{ group.name }}
          </p>
          <p class="truncate text-xs text-muted-foreground">
            {{ group.description || '未填写描述' }}
          </p>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
export interface RoutingGroupListItem {
  id: string
  name: string
  description?: string | null
  enabled: boolean
}

defineProps<{
  groups: RoutingGroupListItem[]
}>()
</script>
