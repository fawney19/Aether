<template>
  <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
    <div class="min-w-0">
      <h4 class="truncate text-base font-semibold text-foreground">
        {{ legacyT(editing ? '编辑分组' : '新建分组') }}
      </h4>
      <p class="text-xs text-muted-foreground">
        {{ legacyT(isDefault ? '当前为所有用户的默认组' : '通过额外分组配置访问限制') }}
      </p>
    </div>
    <div
      v-if="editing"
      class="flex items-center gap-1"
    >
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        :class="isDefault ? 'text-emerald-500 hover:text-emerald-500' : ''"
        :disabled="saving || isDefault"
        :title="legacyT(isDefault ? '默认注册组' : '设为默认注册组')"
        @click="$emit('setDefault')"
      >
        <BadgeCheck class="h-4 w-4" />
      </Button>
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8"
        :disabled="saving || isDefault"
        :title="legacyT('删除分组')"
        @click="$emit('delete')"
      >
        <Trash2 class="h-4 w-4" />
      </Button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { BadgeCheck, Trash2 } from 'lucide-vue-next'
import { Button } from '@/components/ui'
import { useI18n } from '@/i18n'

defineProps<{
  editing: boolean
  isDefault: boolean
  saving: boolean
}>()

defineEmits<{
  setDefault: []
  delete: []
}>()

const { legacyT } = useI18n()
</script>
