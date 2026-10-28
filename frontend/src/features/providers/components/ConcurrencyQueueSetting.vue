<template>
  <div class="mt-2 space-y-1">
    <div class="flex min-h-11 items-center justify-between gap-2">
      <Label
        :for="id"
        class="flex min-h-11 items-center text-xs cursor-pointer"
      >{{ legacyT('并发满时排队') }}</Label>
      <Switch
        :id="id"
        :model-value="modelValue"
        :aria-label="legacyT('并发满时排队')"
        :aria-describedby="`${id}-description`"
        class="focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2"
        @update:model-value="$emit('update:modelValue', $event)"
      />
    </div>
    <p
      :id="`${id}-description`"
      class="text-xs text-muted-foreground"
    >
      {{ legacyT('关闭后不等待，尝试其他密钥') }}
    </p>
  </div>
</template>

<script setup lang="ts">
import { useId } from 'vue'
import { Label, Switch } from '@/components/ui'
import { useI18n } from '@/i18n'

defineProps<{ modelValue: boolean }>()
defineEmits<{ 'update:modelValue': [value: boolean] }>()

const id = useId()
const { legacyT } = useI18n()
</script>
