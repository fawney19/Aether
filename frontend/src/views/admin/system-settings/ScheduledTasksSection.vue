<template>
  <CardSection
    title="定时任务"
    description="配置系统后台定时任务"
  >
    <div class="space-y-3">
      <template
        v-for="task in scheduledTasks"
        :key="task.id"
      >
        <div
          class="group relative rounded-xl border transition-all duration-300"
          :class="task.enabled
            ? 'border-primary/30 bg-primary/[0.02] shadow-sm shadow-primary/5'
            : 'border-border bg-card hover:border-border/80'"
        >
          <div class="flex items-center gap-4 p-4">
            <div class="shrink-0">
              <Switch
                :id="`enable-${task.id}`"
                :model-value="task.enabled"
                @update:model-value="task.onToggle"
              />
            </div>

            <div class="flex items-center gap-3 flex-1 min-w-0">
              <div
                class="w-9 h-9 rounded-lg flex items-center justify-center shrink-0 transition-colors duration-300"
                :class="task.enabled
                  ? 'bg-primary/10 text-primary'
                  : 'text-muted-foreground'"
              >
                <component
                  :is="task.icon"
                  class="w-4.5 h-4.5"
                />
              </div>
              <div class="flex-1 min-w-0">
                <h4 class="font-medium text-sm">
                  {{ task.title }}
                </h4>
                <p class="text-xs text-muted-foreground mt-0.5 truncate">
                  {{ task.description }}
                </p>
              </div>
            </div>

            <div
              v-if="task.enabled && task.hasTimeConfig"
              class="flex items-center gap-2 shrink-0"
            >
              <Clock class="w-4 h-4 text-muted-foreground" />
              <Select
                :model-value="task.hour"
                @update:model-value="(val: string) => task.updateTime(val, task.minute)"
              >
                <SelectTrigger class="w-16 h-8 text-xs px-2 justify-center gap-1">
                  <SelectValue placeholder="时" />
                </SelectTrigger>
                <SelectContent class="min-w-0">
                  <SelectItem
                    v-for="h in 24"
                    :key="h - 1"
                    :value="String(h - 1).padStart(2, '0')"
                  >
                    {{ String(h - 1).padStart(2, '0') }}
                  </SelectItem>
                </SelectContent>
              </Select>
              <span class="text-sm text-muted-foreground">:</span>
              <Select
                :model-value="task.minute"
                @update:model-value="(val: string) => task.updateTime(task.hour, val)"
              >
                <SelectTrigger class="w-16 h-8 text-xs px-2 justify-center gap-1">
                  <SelectValue placeholder="分" />
                </SelectTrigger>
                <SelectContent class="min-w-0">
                  <SelectItem
                    v-for="m in 60"
                    :key="m - 1"
                    :value="String(m - 1).padStart(2, '0')"
                  >
                    {{ String(m - 1).padStart(2, '0') }}
                  </SelectItem>
                </SelectContent>
              </Select>
              <template v-if="task.hasChanges">
                <Button
                  variant="ghost"
                  size="sm"
                  class="h-8 px-2.5 text-xs"
                  :disabled="task.loading"
                  @click="task.onCancel"
                >
                  <X class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="default"
                  size="sm"
                  class="h-8 px-2.5 text-xs"
                  :disabled="task.loading"
                  @click="task.onSave"
                >
                  <Check
                    v-if="!task.loading"
                    class="w-3.5 h-3.5"
                  />
                  <Loader2
                    v-else
                    class="w-3.5 h-3.5 animate-spin"
                  />
                </Button>
              </template>
            </div>
          </div>
        </div>
      </template>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { Clock, Check, Loader2, X } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Switch from '@/components/ui/switch.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { CardSection } from '@/components/layout'
import type { Component } from 'vue'

interface ScheduledTask {
  id: string
  icon: Component
  title: string
  description: string
  enabled: boolean
  hasTimeConfig: boolean
  hour: string
  minute: string
  updateTime: (hour: string, minute: string) => void
  hasChanges: boolean
  loading: boolean
  onToggle: (enabled: boolean) => void
  onSave: () => void
  onCancel: () => void
}

defineProps<{
  scheduledTasks: ScheduledTask[]
}>()
</script>
