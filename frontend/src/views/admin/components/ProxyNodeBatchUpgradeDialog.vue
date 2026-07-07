<template>
  <Dialog
    :model-value="open"
    :title="legacyT('批量升级')"
    :description="legacyT('给所有 tunnel 节点写入升级目标，节点会在下次心跳自动领取')"
    :icon="Settings"
    size="sm"
    @update:model-value="$emit('update:open', $event)"
  >
    <form
      class="space-y-4"
      @submit.prevent="$emit('submit')"
    >
      <div class="space-y-1.5">
        <Label>{{ legacyT('目标版本') }}</Label>
        <Input
          :model-value="version"
          :placeholder="legacyT('例如 0.2.3')"
          @update:model-value="$emit('update:version', String($event))"
        />
        <p class="text-xs text-muted-foreground">
          {{ legacyT('gateway 只会写入 `upgrade_to` 目标版本，不再维护分波 rollout 或确认状态。') }}
        </p>
      </div>
    </form>
    <template #footer>
      <Button
        variant="outline"
        @click="$emit('update:open', false)"
      >
        {{ legacyT('取消') }}
      </Button>
      <Button
        :disabled="upgrading || !version.trim()"
        @click="$emit('submit')"
      >
        {{ upgrading ? legacyT('下发中...') : legacyT('确认下发') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { Settings } from 'lucide-vue-next'
import { Button, Dialog, Input, Label } from '@/components/ui'
import { useI18n } from '@/i18n'

defineProps<{
  open: boolean
  version: string
  upgrading: boolean
}>()

defineEmits<{
  'update:open': [value: boolean]
  'update:version': [value: string]
  submit: []
}>()

const { legacyT } = useI18n()
</script>
