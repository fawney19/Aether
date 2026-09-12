<template>
  <Button
    variant="ghost"
    size="icon"
    class="h-8 w-5 touch-none select-none cursor-grab text-muted-foreground/50 hover:text-primary active:cursor-grabbing disabled:cursor-default"
    :disabled="disabled"
    :title="legacyT('拖动调整当前页展示顺序，也可使用方向键移动')"
    :aria-label="`${legacyT('调整展示顺序')}: ${providerName}`"
    data-provider-drag-handle
    @pointerdown.stop="$emit('pointerdown', $event)"
    @keydown.stop="$emit('keydown', $event)"
    @mousedown.stop
    @click.stop
    @dragstart.prevent
  >
    <GripVertical class="h-4 w-4" />
  </Button>
</template>

<script setup lang="ts">
import { GripVertical } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import { useI18n } from '@/i18n'

defineProps<{
  providerName: string
  disabled: boolean
}>()

defineEmits<{
  pointerdown: [event: PointerEvent]
  keydown: [event: KeyboardEvent]
}>()

const { legacyT } = useI18n()
</script>
