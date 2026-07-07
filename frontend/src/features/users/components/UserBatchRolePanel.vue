<template>
  <div class="space-y-4 rounded-2xl border bg-background p-4 shadow-sm">
    <div class="flex items-start gap-3">
      <div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
        <UserCog class="h-4 w-4" />
      </div>
      <div class="min-w-0 space-y-1">
        <h4 class="text-sm font-semibold text-foreground">
          {{ legacyT('批量修改用户角色') }}
        </h4>
        <p class="text-xs leading-relaxed text-muted-foreground">
          {{ legacyT('将所选用户统一调整为同一个角色。管理员角色拥有后台管理权限，请确认选择范围。') }}
        </p>
      </div>
    </div>

    <div class="grid gap-3 rounded-xl border border-border/70 bg-muted/25 p-3 sm:grid-cols-[10rem_minmax(0,1fr)] sm:items-center">
      <div>
        <Label class="text-sm font-medium">{{ legacyT('目标角色') }}</Label>
        <p class="mt-1 text-[11px] text-muted-foreground">
          {{ legacyT('对所有目标用户生效') }}
        </p>
      </div>
      <Select
        :model-value="modelValue"
        @update:model-value="$emit('update:modelValue', $event)"
      >
        <SelectTrigger class="h-10 w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="user">
            {{ legacyT('普通用户') }}
          </SelectItem>
          <SelectItem value="admin">
            {{ legacyT('管理员') }}
          </SelectItem>
          <SelectItem value="audit_admin">
            {{ legacyT('审计管理员') }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <div class="rounded-xl border border-amber-200/70 bg-amber-50/70 px-3 py-2.5 text-xs leading-relaxed text-amber-800 dark:border-amber-900/50 dark:bg-amber-950/30 dark:text-amber-200">
      {{ warningText }}
    </div>
  </div>
</template>

<script setup lang="ts">
import { Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui'
import { UserCog } from 'lucide-vue-next'
import { useI18n } from '@/i18n'
import type { UserRole } from '@/api/users'

defineProps<{
  modelValue: UserRole
  warningText: string
}>()

defineEmits<{
  'update:modelValue': [value: UserRole]
}>()

const { legacyT } = useI18n()
</script>
