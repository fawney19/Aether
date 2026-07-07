<template>
  <div class="flex items-center gap-3">
    <Avatar class="h-10 w-10 flex-shrink-0 ring-2 ring-background shadow-md">
      <AvatarFallback class="bg-primary text-sm font-bold text-white">
        {{ row.user.username.charAt(0).toUpperCase() }}
      </AvatarFallback>
    </Avatar>
    <div class="min-w-0 flex-1">
      <div class="mb-1 flex items-center gap-1.5">
        <div
          class="truncate text-sm font-semibold"
          :title="row.user.username"
        >
          {{ row.user.username }}
        </div>
        <Badge
          :variant="row.roleBadgeVariant"
          class="h-5 flex-shrink-0 px-1.5 py-0 text-[10px] font-medium"
        >
          {{ legacyT(row.roleLabel) }}
        </Badge>
      </div>
      <div
        class="truncate text-xs text-muted-foreground"
        :title="row.user.email || '-'"
      >
        {{ row.user.email || '-' }}
      </div>
      <div
        v-if="showGroups && row.user.groups?.length"
        class="mt-1 flex flex-wrap gap-1"
      >
        <Badge
          v-for="group in row.user.groups"
          :key="group.id"
          variant="outline"
          class="h-5 px-1.5 py-0 text-[10px]"
        >
          {{ group.name }}
        </Badge>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import Badge from '@/components/ui/badge.vue'
import Avatar from '@/components/ui/avatar.vue'
import AvatarFallback from '@/components/ui/avatar-fallback.vue'
import { useI18n } from '@/i18n'
import type { UserManagementRow } from './user-management-types'

withDefaults(defineProps<{
  row: UserManagementRow
  showGroups?: boolean
}>(), {
  showGroups: true,
})

const { legacyT } = useI18n()
</script>
