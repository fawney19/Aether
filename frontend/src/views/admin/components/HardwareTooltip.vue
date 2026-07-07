<script setup lang="ts">
import type { ProxyNode } from '@/api/proxy-nodes'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'
import { Cpu } from 'lucide-vue-next'
import { computed } from 'vue'
import { formatCompactNumber } from '@/utils/format'
import { useI18n } from '@/i18n'

const props = defineProps<{ node: ProxyNode }>()
const { legacyT } = useI18n()

const hardwareInfo = computed<Record<string, unknown> | null>(() => {
  return normalizeHardwareInfo(props.node.hardware_info)
})

const hardwareRows = computed(() => {
  const info = hardwareInfo.value ?? {}
  const rows: Array<{ label: string; value: string }> = []

  const cpuObj = pickRecord(info.cpu, info.cpu_info, info.cpuInfo)
  const cpuCores = pickNumber(
    info.cpu_cores,
    info.cpuCores,
    info.cpu_count,
    info.cpuCount,
    cpuObj?.cores,
    cpuObj?.core_count,
    cpuObj?.coreCount
  )
  if (cpuCores != null) {
    rows.push({ label: 'CPU', value: `${cpuCores} cores` })
  }

  const memObj = pickRecord(info.memory, info.mem, info.ram)
  const memoryMb = pickNumber(
    info.total_memory_mb,
    info.totalMemoryMb,
    info.memory_total_mb,
    info.memoryTotalMb,
    info.memory_mb,
    info.memoryMb,
    memObj?.total_mb,
    memObj?.totalMb,
    memObj?.mb
  )
  if (memoryMb != null) {
    rows.push({ label: 'RAM', value: formatMemory(memoryMb) })
  }

  const osObj = pickRecord(info.os, info.os_info, info.osInfo, info.platform_info, info.platformInfo)
  const osInfo = pickString(
    info.os_info,
    info.osInfo,
    info.os,
    info.platform,
    osObj?.display,
    osObj?.name && osObj?.version ? `${String(osObj.name)} ${String(osObj.version)}` : null,
    osObj?.name
  )
  if (osInfo) {
    rows.push({ label: 'OS', value: osInfo })
  }

  const maxConcurrency = pickNumber(
    props.node.estimated_max_concurrency,
    info.estimated_max_concurrency,
    info.estimatedMaxConcurrency
  )
  if (maxConcurrency != null) {
    rows.push({
      label: 'Max Concurrency',
      value: `~${formatNumber(maxConcurrency)}`,
    })
  }

  const fdLimit = pickNumber(
    info.fd_limit,
    info.fdLimit,
    info.file_descriptor_limit,
    info.fileDescriptorLimit,
    info.ulimit_nofile,
    info.ulimitNofile
  )
  if (fdLimit != null) {
    rows.push({ label: 'FD Limit', value: formatNumber(fdLimit) })
  }

  return rows
})

const nativeTooltipText = computed(() =>
  hardwareRows.value.length > 0
    ? hardwareRows.value.map((row) => `${row.label}: ${row.value}`).join('\n')
    : legacyT('暂无硬件信息上报')
)

const showHardwareInfo = computed(
  () =>
    !props.node.is_manual
    && (hardwareInfo.value !== null || props.node.estimated_max_concurrency != null)
)

function formatMemory(mb: number | null) {
  if (mb == null) return '-'
  if (mb >= 1024) return `${(mb / 1024).toFixed(1)} GB`
  return `${mb} MB`
}

function formatNumber(n: number) {
  return formatCompactNumber(n, { fractionDigits: 1 })
}

function normalizeHardwareInfo(info: unknown): Record<string, unknown> | null {
  if (info == null) return null

  let current: unknown = info
  for (let i = 0; i < 3; i++) {
    if (typeof current !== 'string') break
    const text = current.trim()
    if (!text) return {}
    try {
      current = JSON.parse(text)
    } catch {
      return {}
    }
  }

  if (!current || typeof current !== 'object') {
    return {}
  }

  const obj = current as Record<string, unknown>
  const nested = pickRecord(obj.hardware_info, obj.hardwareInfo, obj.hardware)
  if (nested) return nested
  return obj
}

function pickNumber(...values: unknown[]): number | null {
  for (const value of values) {
    if (value == null) continue
    const parsed = typeof value === 'number' ? value : Number(value)
    if (Number.isFinite(parsed)) return parsed
  }
  return null
}

function pickRecord(...values: unknown[]): Record<string, unknown> | null {
  for (const value of values) {
    if (value && typeof value === 'object') {
      return value as Record<string, unknown>
    }
  }
  return null
}

function pickString(...values: unknown[]): string {
  for (const value of values) {
    if (value == null) continue
    const text = String(value).trim()
    if (text) return text
  }
  return ''
}

</script>

<template>
  <TooltipProvider
    v-if="showHardwareInfo"
    :delay-duration="0"
  >
    <Tooltip>
      <TooltipTrigger as-child>
        <button
          type="button"
          :aria-label="legacyT('硬件信息')"
          :title="nativeTooltipText"
          class="inline-flex items-center justify-center rounded-sm p-0.5 hover:bg-muted/60 transition-colors cursor-pointer"
        >
          <Cpu class="h-3.5 w-3.5 text-muted-foreground" />
        </button>
      </TooltipTrigger>
      <TooltipContent
        side="right"
        :side-offset="8"
        class="w-auto px-3 py-2 text-xs space-y-1"
      >
        <div
          v-if="hardwareRows.length === 0"
          class="text-muted-foreground"
        >
          {{ legacyT('暂无硬件信息上报') }}
        </div>
        <template v-else>
          <div
            v-for="row in hardwareRows"
            :key="row.label"
          >
            {{ row.label }}: {{ row.value }}
          </div>
        </template>
      </TooltipContent>
    </Tooltip>
  </TooltipProvider>
</template>
