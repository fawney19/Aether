<template>
  <div
    v-if="result"
    class="rounded-xl border bg-muted/20 px-3 py-2 text-xs text-muted-foreground"
  >
    {{ label }}
    <span v-if="failuresLabel">
      {{ failuresLabel }}
    </span>
  </div>
</template>

<script setup lang="ts">
import type { UserBatchActionResponse } from '@/api/users'

defineProps<{
  result: UserBatchActionResponse | null
  label: string
  failuresLabel: string
}>()
</script>
