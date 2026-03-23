<template>
  <div class="flex flex-wrap items-center gap-2">
    <Select
      v-model="selectedPreset"
    >
      <SelectTrigger :class="presetTriggerClass">
        <SelectValue placeholder="选择时间段" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="today">
          今天
        </SelectItem>
        <SelectItem value="last7days">
          最近7天
        </SelectItem>
        <SelectItem value="last30days">
          最近30天
        </SelectItem>
        <SelectItem value="last180days">
          最近180天
        </SelectItem>
        <SelectItem value="last1year">
          最近一年
        </SelectItem>
        <SelectItem value="custom">
          自定义
        </SelectItem>
      </SelectContent>
    </Select>

    <div
      v-if="selectedPreset === 'custom'"
      class="flex items-center gap-2"
    >
      <Input
        v-model="startDate"
        type="date"
        :class="dateInputClass"
      />
      <span class="text-xs text-muted-foreground">至</span>
      <Input
        v-model="endDate"
        type="date"
        :class="dateInputClass"
      />
    </div>

    <Select
      v-if="showGranularity"
      v-model="selectedGranularity"
    >
      <SelectTrigger :class="granularityTriggerClass">
        <SelectValue placeholder="粒度" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem
          v-if="includeAutoGranularity"
          value="auto"
        >
          自动
        </SelectItem>
        <SelectItem
          v-if="allowHourly && canUseHourly"
          value="hour"
        >
          小时
        </SelectItem>
        <SelectItem value="day">
          天
        </SelectItem>
        <SelectItem value="week">
          周
        </SelectItem>
        <SelectItem value="month">
          月
        </SelectItem>
      </SelectContent>
    </Select>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Input
} from '@/components/ui'
import type { DateRangeParams } from '@/features/usage/types'

type PickerGranularity = NonNullable<DateRangeParams['granularity']>

const props = defineProps<{
  modelValue: DateRangeParams
  showGranularity?: boolean
  allowHourly?: boolean
  includeAutoGranularity?: boolean
  compact?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: DateRangeParams]
}>()

function getDefaultGranularity(): PickerGranularity {
  return props.includeAutoGranularity ? 'auto' : 'day'
}

const selectedPreset = ref(props.modelValue.preset || 'today')
const startDate = ref(props.modelValue.start_date || '')
const endDate = ref(props.modelValue.end_date || '')
const selectedGranularity = ref<PickerGranularity>(props.modelValue.granularity || getDefaultGranularity())

const showGranularity = computed(() => props.showGranularity !== false)
const allowHourly = computed(() => props.allowHourly === true)
const includeAutoGranularity = computed(() => props.includeAutoGranularity === true)
const compact = computed(() => props.compact === true)
const presetTriggerClass = computed(() =>
  compact.value
    ? 'h-8 w-28 text-xs border-border/60'
    : 'h-8 w-32 text-xs border-border/60',
)
const dateInputClass = computed(() =>
  compact.value
    ? 'h-8 w-32 text-xs border-border/60'
    : 'h-8 w-36 text-xs border-border/60',
)
const granularityTriggerClass = computed(() =>
  compact.value
    ? 'h-8 w-20 text-xs border-border/60'
    : 'h-8 w-24 text-xs border-border/60',
)

const canUseHourly = computed(() => {
  if (selectedPreset.value === 'today') return true
  if (selectedPreset.value === 'custom' && startDate.value && endDate.value) {
    return startDate.value === endDate.value
  }
  return false
})

// 记录上次 emit 的值，避免重复触发
let lastEmittedValue: string | null = null

function buildEmitValue(): DateRangeParams {
  const timezone = Intl.DateTimeFormat().resolvedOptions().timeZone
  const tz_offset_minutes = -new Date().getTimezoneOffset()

  if (selectedPreset.value === 'custom') {
    const start = startDate.value <= endDate.value ? startDate.value : endDate.value
    const end = endDate.value >= startDate.value ? endDate.value : startDate.value
    return {
      start_date: start,
      end_date: end,
      granularity: selectedGranularity.value,
      timezone,
      tz_offset_minutes
    }
  }

  return {
    preset: selectedPreset.value,
    granularity: selectedGranularity.value,
    timezone,
    tz_offset_minutes
  }
}

function getValueKey(value: DateRangeParams): string {
  // 只比较核心字段，忽略 timezone 和 tz_offset_minutes（这些每次都会重新计算）
  if (value.preset) {
    return `preset:${value.preset}:${value.granularity}`
  }
  return `custom:${value.start_date}:${value.end_date}:${value.granularity}`
}

watch(() => props.modelValue, (value) => {
  if (value.preset) selectedPreset.value = value.preset
  if (value.start_date !== undefined) startDate.value = value.start_date || ''
  if (value.end_date !== undefined) endDate.value = value.end_date || ''
  selectedGranularity.value = value.granularity || getDefaultGranularity()
  // 同步更新 lastEmittedValue，避免外部设置值后触发重复 emit
  lastEmittedValue = getValueKey(value)
}, { deep: true })

watch([selectedPreset, startDate, endDate, selectedGranularity], () => {
  if (!allowHourly.value || !canUseHourly.value) {
    if (selectedGranularity.value === 'hour') {
      selectedGranularity.value = includeAutoGranularity.value ? 'auto' : 'day'
    }
  }

  if (selectedPreset.value === 'custom') {
    if (!startDate.value || !endDate.value) return
  }

  const newValue = buildEmitValue()
  const newKey = getValueKey(newValue)

  // 只有当值真正变化时才 emit，避免初始化时的重复触发
  if (newKey !== lastEmittedValue) {
    lastEmittedValue = newKey
    emit('update:modelValue', newValue)
  }
}, { immediate: true })
</script>
