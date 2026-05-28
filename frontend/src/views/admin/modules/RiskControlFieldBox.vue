<template>
  <div class="space-y-2">
    <Label class="text-xs text-muted-foreground">
      {{ label }}
    </Label>
    <slot />
  </div>
</template>

<script setup lang="ts">
import { Label } from '@/components/ui'

defineProps<{
  label: string
}>()
</script>
