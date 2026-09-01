import { computed, ref, type Component, type Ref } from 'vue'
import { CalendarCheck, Gauge, RefreshCw } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { adminApi } from '@/api/admin'
import { log } from '@/utils/logger'
import type { SystemConfig } from './useSystemConfig'

const REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_DEFAULT = 5
const REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MIN = 1
const REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MAX = 60

interface ScheduledTaskScheduleBase {
  hasChanges: boolean
  loading: boolean
  onSave: () => Promise<void>
  onCancel: () => void
}

export interface ScheduledTaskTimeSchedule extends ScheduledTaskScheduleBase {
  kind: 'time'
  hour: string
  minute: string
  updateTime: (hour: string, minute: string) => void
}

export interface ScheduledTaskIntervalSchedule extends ScheduledTaskScheduleBase {
  kind: 'interval'
  minutes: number
  minMinutes: number
  maxMinutes: number
  updateMinutes: (minutes: number) => void
}

export interface ScheduledTask {
  id: string
  icon: Component
  title: string
  description: string
  enabled: boolean
  onToggle: (enabled: boolean) => Promise<void>
  schedule: ScheduledTaskTimeSchedule | ScheduledTaskIntervalSchedule | null
}

export function useScheduledTasks(systemConfig: Ref<SystemConfig>) {
  const { success, error } = useToast()

  const checkinConfigLoading = ref(false)
  const remoteQuotaSyncConfigLoading = ref(false)
  const previousCheckinTime = ref('')
  const previousRemoteQuotaSyncIntervalSeconds = ref(
    REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_DEFAULT * 60
  )

  function initPreviousValues() {
    previousCheckinTime.value = systemConfig.value.provider_checkin_time
    previousRemoteQuotaSyncIntervalSeconds.value =
      systemConfig.value.provider_remote_quota_sync_interval_seconds
  }

  const checkinHour = computed(() => {
    const time = systemConfig.value.provider_checkin_time
    if (!time || !time.includes(':')) return '01'
    return time.split(':')[0]
  })

  const checkinMinute = computed(() => {
    const time = systemConfig.value.provider_checkin_time
    if (!time || !time.includes(':')) return '05'
    return time.split(':')[1]
  })

  function updateCheckinTime(hour: string, minute: string) {
    systemConfig.value.provider_checkin_time = `${hour}:${minute}`
  }

  const hasCheckinTimeChanged = computed(() => {
    return systemConfig.value.provider_checkin_time !== previousCheckinTime.value
  })

  const remoteQuotaSyncIntervalMinutes = computed(() => {
    return systemConfig.value.provider_remote_quota_sync_interval_seconds / 60
  })

  function updateRemoteQuotaSyncInterval(minutes: number) {
    systemConfig.value.provider_remote_quota_sync_interval_seconds = minutes * 60
  }

  const hasRemoteQuotaSyncIntervalChanged = computed(() => {
    return systemConfig.value.provider_remote_quota_sync_interval_seconds !==
      previousRemoteQuotaSyncIntervalSeconds.value
  })

  async function handleProviderCheckinToggle(enabled: boolean) {
    const previousValue = systemConfig.value.enable_provider_checkin
    systemConfig.value.enable_provider_checkin = enabled
    try {
      await adminApi.updateSystemConfig(
        'enable_provider_checkin',
        enabled,
        '是否启用 Provider 自动签到任务'
      )
      success(enabled ? '已启用自动签到' : '已禁用自动签到')
    } catch (err) {
      error('保存配置失败')
      log.error('保存自动签到配置失败:', err)
      systemConfig.value.enable_provider_checkin = previousValue
    }
  }

  async function handleProviderRemoteQuotaSyncToggle(enabled: boolean) {
    const previousValue = systemConfig.value.enable_provider_remote_quota_sync
    systemConfig.value.enable_provider_remote_quota_sync = enabled
    try {
      await adminApi.updateSystemConfig(
        'enable_provider_remote_quota_sync',
        enabled,
        '是否允许 Provider 远程额度自动与手动同步应用'
      )
      success(enabled ? '已启用远程额度同步' : '已暂停远程额度同步')
    } catch (err) {
      error('保存配置失败')
      log.error('保存远程额度同步开关失败:', err)
      systemConfig.value.enable_provider_remote_quota_sync = previousValue
    }
  }

  async function handleOAuthTokenRefreshToggle(enabled: boolean) {
    const previousValue = systemConfig.value.enable_oauth_token_refresh
    systemConfig.value.enable_oauth_token_refresh = enabled
    try {
      await adminApi.updateSystemConfig(
        'enable_oauth_token_refresh',
        enabled,
        '是否启用 OAuth Token 自动刷新任务'
      )
      success(enabled ? '已启用 OAuth Token 自动刷新' : '已禁用 OAuth Token 自动刷新')
    } catch (err) {
      error('保存配置失败')
      log.error('保存 OAuth Token 自动刷新配置失败:', err)
      systemConfig.value.enable_oauth_token_refresh = previousValue
    }
  }

  function handleCheckinTimeCancel() {
    systemConfig.value.provider_checkin_time = previousCheckinTime.value
  }

  function handleRemoteQuotaSyncIntervalCancel() {
    systemConfig.value.provider_remote_quota_sync_interval_seconds =
      previousRemoteQuotaSyncIntervalSeconds.value
  }

  async function handleCheckinTimeSave() {
    const newTime = systemConfig.value.provider_checkin_time
    if (!newTime || !/^\d{2}:\d{2}$/.test(newTime)) {
      error('请输入有效的时间格式 (HH:MM)')
      return
    }

    checkinConfigLoading.value = true
    try {
      await adminApi.updateSystemConfig(
        'provider_checkin_time',
        newTime,
        'Provider 自动签到执行时间（HH:MM 格式）'
      )
      previousCheckinTime.value = newTime
      success(`签到时间已设置为 ${newTime}`)
    } catch (err) {
      error('保存签到时间失败')
      log.error('保存签到时间失败:', err)
    } finally {
      checkinConfigLoading.value = false
    }
  }

  async function handleRemoteQuotaSyncIntervalSave() {
    const seconds = systemConfig.value.provider_remote_quota_sync_interval_seconds
    const minutes = seconds / 60
    if (
      !Number.isInteger(minutes) ||
      minutes < REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MIN ||
      minutes > REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MAX
    ) {
      error(
        `同步间隔必须是 ${REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MIN} 到 ${REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MAX} 之间的整数分钟`
      )
      return
    }

    remoteQuotaSyncConfigLoading.value = true
    try {
      await adminApi.updateSystemConfig(
        'provider_remote_quota_sync_interval_seconds',
        seconds,
        'Provider 远程额度自动同步间隔（秒）'
      )
      previousRemoteQuotaSyncIntervalSeconds.value = seconds
      success(`远程额度同步间隔已设置为 ${minutes} 分钟`)
    } catch (err) {
      error('保存远程额度同步间隔失败')
      log.error('保存远程额度同步间隔失败:', err)
    } finally {
      remoteQuotaSyncConfigLoading.value = false
    }
  }

  const scheduledTasks = computed<ScheduledTask[]>(() => [
    {
      id: 'provider-checkin',
      icon: CalendarCheck,
      title: 'Provider 自动签到',
      description: '自动执行已配置 Provider 的签到任务',
      enabled: systemConfig.value.enable_provider_checkin,
      onToggle: handleProviderCheckinToggle,
      schedule: {
        kind: 'time',
        hour: checkinHour.value,
        minute: checkinMinute.value,
        updateTime: updateCheckinTime,
        hasChanges: hasCheckinTimeChanged.value,
        loading: checkinConfigLoading.value,
        onSave: handleCheckinTimeSave,
        onCancel: handleCheckinTimeCancel,
      },
    },
    {
      id: 'provider-remote-quota-sync',
      icon: Gauge,
      title: 'Provider 远程额度同步',
      description: '定期同步已启用的 Sub2API 套餐；关闭后手动同步也不会应用额度',
      enabled: systemConfig.value.enable_provider_remote_quota_sync,
      onToggle: handleProviderRemoteQuotaSyncToggle,
      schedule: {
        kind: 'interval',
        minutes: remoteQuotaSyncIntervalMinutes.value,
        minMinutes: REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MIN,
        maxMinutes: REMOTE_QUOTA_SYNC_INTERVAL_MINUTES_MAX,
        updateMinutes: updateRemoteQuotaSyncInterval,
        hasChanges: hasRemoteQuotaSyncIntervalChanged.value,
        loading: remoteQuotaSyncConfigLoading.value,
        onSave: handleRemoteQuotaSyncIntervalSave,
        onCancel: handleRemoteQuotaSyncIntervalCancel,
      },
    },
    {
      id: 'oauth-token-refresh',
      icon: RefreshCw,
      title: 'OAuth Token 自动刷新',
      description: '主动刷新即将过期的 OAuth Token（动态调度）',
      enabled: systemConfig.value.enable_oauth_token_refresh,
      onToggle: handleOAuthTokenRefreshToggle,
      schedule: null,
    },
  ])

  return {
    checkinConfigLoading,
    remoteQuotaSyncConfigLoading,
    scheduledTasks,
    initPreviousValues,
  }
}
