<template>
  <div class="space-y-4">
    <div class="space-y-2">
      <Label class="text-sm font-medium">{{ legacyT('名称') }}</Label>
      <Input
        :model-value="name"
        class="h-10"
        :placeholder="legacyT('例如：生产团队')"
        @update:model-value="$emit('update:name', $event)"
      />
    </div>

    <div class="space-y-2">
      <Label class="text-sm font-medium">{{ legacyT('成员') }}</Label>
      <MultiSelect
        :model-value="memberUserIds"
        :options="userOptions"
        :search-threshold="0"
        :disabled="membersDisabled"
        :placeholder="legacyT('选择用户')"
        :empty-text="legacyT('暂无用户')"
        :no-results-text="legacyT('未找到匹配用户')"
        @update:model-value="$emit('update:memberUserIds', $event)"
      />
    </div>
  </div>
</template>

<script setup lang="ts">
import { Input, Label } from '@/components/ui'
import { MultiSelect } from '@/components/common'
import { useI18n } from '@/i18n'
import type { UserSelectOption } from './user-management-types'

defineProps<{
  name: string
  memberUserIds: string[]
  userOptions: UserSelectOption[]
  membersDisabled: boolean
}>()

defineEmits<{
  'update:name': [value: string]
  'update:memberUserIds': [value: string[]]
}>()

const { legacyT } = useI18n()
</script>
