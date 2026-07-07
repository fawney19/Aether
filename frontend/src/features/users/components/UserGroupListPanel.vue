<template>
  <div class="rounded-xl border border-border/70 bg-muted/20 p-3">
    <div class="mb-3 flex items-center justify-between gap-2">
      <Label class="text-sm font-semibold">{{ legacyT('分组') }}</Label>
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        :title="legacyT('新建分组')"
        @click="$emit('create')"
      >
        <Plus class="h-4 w-4" />
      </Button>
    </div>

    <div
      v-if="loading"
      class="rounded-lg border border-dashed border-border/70 px-3 py-8 text-center text-xs text-muted-foreground"
    >
      {{ legacyT('正在加载...') }}
    </div>
    <div
      v-else-if="groups.length === 0"
      class="rounded-lg border border-dashed border-border/70 px-3 py-8 text-center text-xs text-muted-foreground"
    >
      {{ legacyT('暂无分组') }}
    </div>
    <div
      v-else
      class="max-h-60 space-y-1.5 overflow-y-auto lg:max-h-none lg:overflow-visible"
    >
      <button
        v-for="group in groups"
        :key="group.id"
        type="button"
        :class="groupButtonClass(group.id)"
        @click="$emit('select', group.id)"
      >
        <span class="min-w-0 flex-1 text-left">
          <span class="flex items-center gap-1.5">
            <span class="truncate text-sm font-medium">{{ group.name }}</span>
            <Badge
              v-if="group.is_default"
              variant="secondary"
              class="h-5 px-1.5 py-0 text-[10px]"
            >
              {{ legacyT('默认') }}
            </Badge>
          </span>
        </span>
        <ChevronRight class="h-4 w-4 shrink-0 text-muted-foreground" />
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ChevronRight, Plus } from 'lucide-vue-next'
import { Badge, Button, Label } from '@/components/ui'
import { cn } from '@/lib/utils'
import { useI18n } from '@/i18n'
import type { UserGroup } from '@/api/users'

const props = defineProps<{
  loading: boolean
  groups: UserGroup[]
  selectedGroupId: string | null
}>()

defineEmits<{
  create: []
  select: [groupId: string]
}>()

const { legacyT } = useI18n()

function groupButtonClass(groupId: string): string {
  return cn(
    'flex w-full items-center gap-2 rounded-lg border px-3 py-2 transition-colors',
    props.selectedGroupId === groupId
      ? 'border-primary/50 bg-primary/10'
      : 'border-transparent hover:border-border hover:bg-background',
  )
}
</script>
