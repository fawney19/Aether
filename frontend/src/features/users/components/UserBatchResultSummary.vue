<template>
  <div
    v-if="result"
    class="rounded-xl border bg-muted/20 px-3 py-2 text-xs text-muted-foreground"
  >
    {{ label }}
    <span v-if="failuresLabel">
      {{ failuresLabel }}
    </span>
    <details v-if="result.interrupted" class="mt-2 border-t border-border/60 pt-2">
      <summary class="cursor-pointer select-none">{{ legacyT('查看批次中断详情') }}</summary>
      <div class="mt-2 max-h-32 space-y-1 overflow-auto break-all">
        <p>{{ legacyT('已完成用户 ID') }}：{{ userIds(result.completed_user_ids) }}</p>
        <p>{{ legacyT('结果待核对用户 ID') }}：{{ userIds(result.uncertain_user_ids) }}</p>
        <p>{{ legacyT('尚未执行用户 ID') }}：{{ userIds(result.unprocessed_user_ids) }}</p>
      </div>
    </details>
  </div>
</template>

<script setup lang="ts">
import type { UserBatchActionResponse } from '@/api/users'
import { useI18n } from '@/i18n'

const { legacyT } = useI18n()

function userIds(ids?: string[]): string {
  return ids && ids.length > 0 ? ids.join(', ') : '-'
}

defineProps<{
  result: UserBatchActionResponse | null
  label: string
  failuresLabel: string
}>()
</script>
