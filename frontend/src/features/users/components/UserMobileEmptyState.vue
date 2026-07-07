<template>
  <div class="rounded-2xl border border-dashed border-border/60 bg-card/70 px-6 py-10 text-center">
    <Avatar class="mx-auto mb-3 h-12 w-12">
      <AvatarFallback class="bg-muted text-base font-semibold text-muted-foreground">
        U
      </AvatarFallback>
    </Avatar>
    <p class="text-sm font-medium text-foreground">
      {{ hasFilters ? legacyT('未找到匹配的用户') : legacyT('暂无用户') }}
    </p>
    <p
      v-if="hasFilters"
      class="mt-1 text-xs text-muted-foreground"
    >
      {{ legacyT('尝试调整筛选条件') }}
    </p>
  </div>
</template>

<script setup lang="ts">
import Avatar from '@/components/ui/avatar.vue'
import AvatarFallback from '@/components/ui/avatar-fallback.vue'
import { useI18n } from '@/i18n'

defineProps<{
  hasFilters: boolean
}>()

const { legacyT } = useI18n()
</script>
