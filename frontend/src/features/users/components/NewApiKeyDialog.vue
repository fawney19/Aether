<template>
  <Dialog
    :model-value="open"
    size="lg"
    @update:model-value="(value) => !value && $emit('close')"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-emerald-100 dark:bg-emerald-900/30">
            <CheckCircle class="h-5 w-5 text-emerald-600 dark:text-emerald-400" />
          </div>
          <div class="min-w-0 flex-1">
            <h3 class="text-lg font-semibold leading-tight text-foreground">
              {{ legacyT('创建成功') }}
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ legacyT('请妥善保管, 切勿泄露给他人.') }}
            </p>
          </div>
        </div>
      </div>
    </template>

    <div class="space-y-4">
      <div class="space-y-2">
        <Label class="text-sm font-medium">API Key</Label>
        <div class="flex items-center gap-2">
          <Input
            type="text"
            :model-value="apiKey"
            readonly
            class="h-11 flex-1 bg-muted/50 font-mono text-sm"
            @click="selectApiKey"
          />
          <Button
            class="h-11"
            @click="$emit('copy')"
          >
            {{ legacyT('复制') }}
          </Button>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        class="h-10 px-5"
        @click="$emit('close')"
      >
        {{ legacyT('确定') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { CheckCircle } from 'lucide-vue-next'
import { Button, Dialog, Input, Label } from '@/components/ui'
import { useI18n } from '@/i18n'

defineProps<{
  open: boolean
  apiKey: string
}>()

defineEmits<{
  close: []
  copy: []
}>()

const { legacyT } = useI18n()

function selectApiKey(event: MouseEvent) {
  const target = event.target as HTMLInputElement | null
  target?.select?.()
}
</script>
