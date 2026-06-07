import { computed, ref, watch } from 'vue'
import { useAuthStore } from '@/stores/auth'
import { useToast } from '@/composables/useToast'
import { adminApi, type CheckUpdateResponse, type ReleaseEntry, type SystemUpdateCapabilityResponse, type UpdateTaskStatusResponse } from '@/api/admin'
import { parseApiError } from '@/utils/errorParser'
import { buildUpdateErrorStatus } from '@/utils/updateStatus'

export type SystemUpdatePhase = 'download' | 'restart' | 'reconnecting'

const showUpdateDialog = ref(false)
const updateInfo = ref<CheckUpdateResponse | null>(null)
const versionStatus = ref<CheckUpdateResponse | null>(null)
const loadingVersionStatus = ref(false)
const applyingSystemUpdate = ref(false)
const updateSupported = ref(true)
const updateExecutionMode = ref('manual')
const updateStrategy = ref('manual')
const updateCapabilityMessage = ref<string | null>(null)
const dockerUpdateCommand = ref<string | null>(null)
const dockerUpdateStatus = ref<string | null>(null)
const reconnectMessage = ref('等待服务恢复...')
const rollbackAvailable = ref(false)
const rollingBack = ref(false)
const updateTaskStatus = ref<UpdateTaskStatusResponse | null>(null)
const updateDialogMode = ref<'latest' | 'selected'>('latest')
const systemUpdatePhase = ref<SystemUpdatePhase>(readStoredSystemUpdatePhase())
const preparedUpdateVersion = ref<string | null>(
  readSessionStorageItem('aether_prepared_update_version')
)

const SOURCE_BUILD_UPDATE_HINT = '当前为源码构建，请使用 git pull 后重新编译。'
const SOURCE_BUILD_RELEASE_HINT = '当前为源码构建，请手动切换到对应标签后重新编译。'
const MANUAL_UPDATE_HINT = '当前部署策略不支持在线自更新，请手动下载 Release 或使用安装脚本更新。'

let versionStatusLoadPromise: Promise<CheckUpdateResponse | null> | null = null
let updateStatusPollTimer: number | null = null

const updateProgressPercent = computed(() => updateTaskStatus.value?.progress_percent ?? null)
const updateProgressText = computed(() => formatUpdateProgressText(updateTaskStatus.value))
const updateOutputTail = computed(() => {
  const explicit = updateTaskStatus.value?.output_tail
  if (Array.isArray(explicit)) return explicit
  return updateTaskStatus.value?.output?.split('\n').filter(Boolean) ?? []
})
const updateDialogTitle = computed(() => {
  if (updateDialogMode.value === 'selected') {
    return updateSupported.value ? '切换版本' : '版本详情'
  }
  return '发现新版本'
})
const updateDialogVersionLabel = computed(() => {
  if (updateDialogMode.value === 'selected') {
    return updateSupported.value ? '目标版本' : '版本标签'
  }
  return '最新版本'
})
const updateDialogReleaseLinkLabel = computed(() => {
  if (updateDialogMode.value === 'selected') return '查看标签页'
  return updateSupported.value ? '查看更新' : '查看发布'
})

watch(systemUpdatePhase, (val) => {
  setSessionStorageItem('aether_update_phase', val)
})
watch(preparedUpdateVersion, (val) => {
  if (val) {
    setSessionStorageItem('aether_prepared_update_version', val)
  } else {
    removeSessionStorageItem('aether_prepared_update_version')
  }
})

function readStoredSystemUpdatePhase(): SystemUpdatePhase {
  const stored = readSessionStorageItem('aether_update_phase')
  if (stored === 'restart' || stored === 'reconnecting') return stored
  return 'download'
}

function readSessionStorageItem(key: string): string | null {
  try {
    return sessionStorage.getItem(key)
  } catch {
    return null
  }
}

function setSessionStorageItem(key: string, value: string) {
  try {
    sessionStorage.setItem(key, value)
  } catch {
    // Keep the state in memory when sessionStorage is unavailable.
  }
}

function removeSessionStorageItem(key: string) {
  try {
    sessionStorage.removeItem(key)
  } catch {
    // Keep the state in memory when sessionStorage is unavailable.
  }
}

function formatUpdateProgressText(status: UpdateTaskStatusResponse | null): string {
  if (!status) return updateExecutionMode.value === 'docker' ? '正在执行 Docker 更新脚本...' : '正在下载更新包...'
  const label = status.progress_label
    ? status.phase.startsWith('downloading')
      ? `正在下载${status.progress_label}`
      : status.progress_label
    : formatUpdateTaskPhase(status.phase)
  const downloaded = status.downloaded_bytes
  const total = status.total_bytes
  if (typeof downloaded === 'number' && typeof total === 'number' && total > 0) {
    return `${label} ${formatFileSize(downloaded)} / ${formatFileSize(total)}`
  }
  if (typeof downloaded === 'number' && downloaded > 0) {
    return `${label} ${formatFileSize(downloaded)}`
  }
  return label
}

function formatUpdateTaskPhase(phase: string): string {
  switch (phase) {
    case 'running':
      return '正在执行 Docker 更新脚本'
    case 'downloading':
      return '正在下载更新包'
    case 'downloading_checksum':
      return '正在下载校验文件'
    case 'verifying':
      return '正在校验更新包'
    case 'extracting':
      return '正在解压更新包'
    case 'prepared':
      return updateExecutionMode.value === 'docker' ? 'Docker 更新已执行' : '更新包已准备完成'
    default:
      return '正在准备更新'
  }
}

function formatFileSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${bytes} B`
}

async function refreshUpdateTaskStatus() {
  try {
    updateTaskStatus.value = await adminApi.getUpdateStatus()
  } catch {
    // Keep the last snapshot while the service is restarting.
  }
}

async function waitForPreparedUpdate(): Promise<UpdateTaskStatusResponse> {
  const deadline = Date.now() + 10 * 60 * 1000
  while (Date.now() < deadline) {
    await new Promise(resolve => setTimeout(resolve, 1000))
    await refreshUpdateTaskStatus()
    const status = updateTaskStatus.value
    if (status?.phase === 'prepared') return status
    if (status?.phase === 'failed') {
      throw new Error(status.error || '下载更新失败')
    }
  }
  throw new Error('下载更新超时')
}

function startUpdateStatusPolling() {
  stopUpdateStatusPolling()
  void refreshUpdateTaskStatus()
  updateStatusPollTimer = window.setInterval(() => {
    void refreshUpdateTaskStatus()
  }, 1000)
}

function stopUpdateStatusPolling() {
  if (updateStatusPollTimer !== null) {
    window.clearInterval(updateStatusPollTimer)
    updateStatusPollTimer = null
  }
}

function shouldShowUpdatePrompt(latestVersion: string): boolean {
  const ignoreData = localStorage.getItem('aether_update_ignore')
  if (!ignoreData) return true

  try {
    const { version, until } = JSON.parse(ignoreData)
    if (version === latestVersion && Date.now() < until) {
      return false
    }
  } catch {
    // Invalid ignore payloads should not suppress prompts.
  }
  return true
}

async function loadVersionStatus(force = false) {
  const authStore = useAuthStore()
  if (authStore.user?.role !== 'admin') return null
  if (versionStatusLoadPromise) return versionStatusLoadPromise

  loadingVersionStatus.value = true
  versionStatusLoadPromise = (async () => {
    try {
      const [status, capability] = await Promise.all([
        adminApi.checkUpdate(force),
        adminApi.getSystemUpdateCapability().catch(() => null),
      ])
      if (capability) {
        applyUpdateCapability(capability)
      }
      versionStatus.value = updateSupported.value === false && status.has_update
        ? {
            ...status,
            updatable: false,
            update_blocker: updateUnsupportedMessage(SOURCE_BUILD_UPDATE_HINT),
          }
        : status
      syncSystemUpdatePhase(versionStatus.value)
      return versionStatus.value
    } catch (error) {
      versionStatus.value = buildUpdateErrorStatus(versionStatus.value, error)
      return versionStatus.value
    } finally {
      loadingVersionStatus.value = false
      versionStatusLoadPromise = null
    }
  })()

  return versionStatusLoadPromise
}

function applyUpdateCapability(capability: SystemUpdateCapabilityResponse) {
  const canExecute = capability.can_execute_update ?? capability.enabled ?? capability.supported
  updateSupported.value = canExecute
  rollbackAvailable.value = capability.supported && capability.rollback_available
  updateStrategy.value = capability.update_strategy || capability.strategy || 'manual'
  updateExecutionMode.value = capability.execution_mode || (capability.supported ? 'self' : updateStrategy.value)
  updateCapabilityMessage.value = capability.message || null
  dockerUpdateCommand.value = capability.docker_update_command || null
  dockerUpdateStatus.value = capability.docker_update_status || null
}

function updateUnsupportedMessage(fallback = MANUAL_UPDATE_HINT): string {
  return updateCapabilityMessage.value || fallback
}

function syncSystemUpdatePhase(status: CheckUpdateResponse | null) {
  if (systemUpdatePhase.value === 'reconnecting') return
  if (systemUpdatePhase.value === 'restart') {
    if (!preparedUpdateVersion.value) {
      systemUpdatePhase.value = 'download'
    }
    return
  }
  if (!status?.has_update) {
    systemUpdatePhase.value = 'download'
    preparedUpdateVersion.value = null
  }
}

function handleVersionRefresh() {
  void loadVersionStatus(true)
}

function openVersionReleasePage() {
  if (versionStatus.value?.release_url) {
    window.open(versionStatus.value.release_url, '_blank', 'noopener,noreferrer')
  }
}

function buildUpdateInfoFromRelease(release: ReleaseEntry): CheckUpdateResponse {
  const currentVersion =
    versionStatus.value?.current_version ||
    updateInfo.value?.current_version ||
    __APP_VERSION__ ||
    ''
  const canUpdate = updateSupported.value
  return {
    current_version: currentVersion,
    latest_version: release.version,
    has_update: !release.is_current,
    updatable: canUpdate && !release.is_current && release.updatable,
    update_blocker: release.is_current
      ? '当前已是这个版本'
      : !canUpdate
        ? updateUnsupportedMessage(SOURCE_BUILD_RELEASE_HINT)
        : release.update_blocker,
    release_url: release.release_url,
    release_notes: release.release_notes,
    published_at: release.published_at,
    error: null,
  }
}

function openReleaseUpdateDialog(release: ReleaseEntry) {
  updateDialogMode.value = 'selected'
  updateInfo.value = buildUpdateInfoFromRelease(release)
  if (systemUpdatePhase.value !== 'reconnecting') {
    systemUpdatePhase.value = 'download'
    preparedUpdateVersion.value = null
  }
  showUpdateDialog.value = true
}

async function handleApplySystemUpdate() {
  if (applyingSystemUpdate.value) return
  const { success, error: showError } = useToast()
  applyingSystemUpdate.value = true
  try {
    const capability = await adminApi.getSystemUpdateCapability()
    applyUpdateCapability(capability)
    if (!updateSupported.value) {
      showError(updateUnsupportedMessage('不支持在线更新'), '不支持在线更新')
      return
    }

    const targetStatus = updateInfo.value || versionStatus.value
    if (targetStatus?.has_update && targetStatus.updatable === false) {
      showError(targetStatus.update_blocker || '当前版本暂不支持在线更新', '无法在线更新')
      return
    }

    if (updateExecutionMode.value === 'docker') {
      updateTaskStatus.value = null
      startUpdateStatusPolling()
      try {
        const result = await adminApi.applySystemUpdate(targetStatus?.latest_version || null)
        success(result.message || 'Docker 更新已启动')
        systemUpdatePhase.value = 'reconnecting'
        reconnectMessage.value = 'Docker 正在重建应用服务...'
        showUpdateDialog.value = true
        applyingSystemUpdate.value = false
        await pollHealthUntilReady()
      } finally {
        stopUpdateStatusPolling()
      }
      return
    }

    if (systemUpdatePhase.value === 'download') {
      const targetVersion = targetStatus?.latest_version || null
      updateTaskStatus.value = null
      startUpdateStatusPolling()
      try {
        const result = await adminApi.prepareSystemUpdate(targetVersion)
        const finalStatus = await waitForPreparedUpdate()
        preparedUpdateVersion.value = targetVersion
        systemUpdatePhase.value = 'restart'
        success(finalStatus.output || result.message || '更新包已下载完成，请点击“立即重启”完成安装')
      } finally {
        stopUpdateStatusPolling()
        void refreshUpdateTaskStatus()
      }
      return
    }

    const result = await adminApi.applySystemUpdate(preparedUpdateVersion.value)
    success(result.message || '一键重启已启动')
    systemUpdatePhase.value = 'reconnecting'
    reconnectMessage.value = '服务正在重启...'
    showUpdateDialog.value = true
    applyingSystemUpdate.value = false
    await pollHealthUntilReady()
  } catch (err) {
    const fallback = systemUpdatePhase.value === 'download' ? '启动更新失败' : '启动重启失败'
    showError(parseApiError(err, fallback))
  } finally {
    applyingSystemUpdate.value = false
  }
}

async function handleRollback() {
  if (rollingBack.value) return
  const { success, error: showError } = useToast()
  rollingBack.value = true
  try {
    const result = await adminApi.rollbackSystemUpdate()
    success(result.message || '回滚已启动')
    systemUpdatePhase.value = 'reconnecting'
    reconnectMessage.value = '正在回滚到上一版本...'
    showUpdateDialog.value = true
    rollingBack.value = false
    await pollHealthUntilReady()
  } catch (err) {
    showError(parseApiError(err, '回滚失败'))
  } finally {
    rollingBack.value = false
  }
}

async function pollHealthUntilReady() {
  const maxAttempts = 60
  const intervalMs = 2000

  for (let i = 0; i < maxAttempts; i++) {
    if (i < 3) {
      reconnectMessage.value = i === 0 ? '服务正在重启...' : `服务正在重启... (${i * 2}s)`
      await new Promise(r => setTimeout(r, intervalMs))
      continue
    }

    const elapsed = i * 2
    reconnectMessage.value = `等待服务恢复... (${elapsed}s)`
    try {
      const resp = await fetch('/_gateway/health', {
        method: 'GET',
        signal: AbortSignal.timeout(3000),
      })
      if (resp.ok) {
        reconnectMessage.value = '服务已恢复，正在刷新...'
        await new Promise(r => setTimeout(r, 500))
        window.location.replace(buildFreshReloadUrl())
        return
      }
    } catch {
      // Expected while the app service is down.
    }

    if (i > 15) {
      try {
        const status = await adminApi.getUpdateStatus()
        if (status.phase === 'failed' && status.error) {
          reconnectMessage.value = `更新失败: ${status.error}`
          systemUpdatePhase.value = 'download'
          return
        }
      } catch {
        // Service may still be down.
      }
    }

    await new Promise(r => setTimeout(r, intervalMs))
  }

  reconnectMessage.value = '等待超时，请手动刷新页面'
  systemUpdatePhase.value = 'download'
}

function buildFreshReloadUrl(): string {
  const url = new URL(window.location.href)
  url.searchParams.set('__aether_reload', Date.now().toString())
  return url.toString()
}

function showDebugUpdateDialog() {
  const currentVersion = versionStatus.value?.current_version || __APP_VERSION__ || '0.7.0-rc28'
  updateDialogMode.value = 'latest'
  updateInfo.value = {
    current_version: currentVersion,
    latest_version: 'v0.7.0-rc99',
    has_update: true,
    release_url: 'https://github.com/fawney19/Aether/releases',
    release_notes: [
      "### What's Changed",
      '- 调整版本更新提示样式',
      '- 修复开发分支版本误判',
      '- 统一版本号显示格式',
    ].join('\n'),
    published_at: new Date().toISOString(),
    updatable: true,
    update_blocker: null,
    error: null,
  }
  systemUpdatePhase.value = 'download'
  preparedUpdateVersion.value = null
  showUpdateDialog.value = true
}

function showDebugVersionStatus(hasUpdate = true) {
  const currentVersion = versionStatus.value?.current_version || __APP_VERSION__ || '0.7.0-rc28'
  versionStatus.value = {
    current_version: currentVersion,
    latest_version: hasUpdate ? 'v0.7.0-rc99' : currentVersion,
    has_update: hasUpdate,
    release_url: hasUpdate ? 'https://github.com/fawney19/Aether/releases' : null,
    release_notes: hasUpdate
      ? [
        "### What's Changed",
        '- 调整版本更新提示样式',
        '- 修复开发分支版本误判',
        '- 统一版本号显示格式',
      ].join('\n')
      : null,
    published_at: hasUpdate ? new Date().toISOString() : null,
    updatable: hasUpdate,
    update_blocker: null,
    error: null,
  }
  systemUpdatePhase.value = 'download'
  preparedUpdateVersion.value = null
}

async function checkForUpdate() {
  const authStore = useAuthStore()
  if (!authStore.canOperateAdmin) return

  const sessionKey = 'aether_update_checked'
  if (sessionStorage.getItem(sessionKey)) return
  sessionStorage.setItem(sessionKey, '1')

  const result = versionStatus.value ?? await loadVersionStatus()
  if (result?.has_update && result.latest_version && shouldShowUpdatePrompt(result.latest_version)) {
    updateDialogMode.value = 'latest'
    updateInfo.value = result
    showUpdateDialog.value = true
  }
}

export function useSystemUpdate() {
  return {
    showUpdateDialog,
    updateInfo,
    versionStatus,
    loadingVersionStatus,
    applyingSystemUpdate,
    updateSupported,
    updateExecutionMode,
    updateStrategy,
    dockerUpdateCommand,
    dockerUpdateStatus,
    reconnectMessage,
    rollbackAvailable,
    rollingBack,
    updateTaskStatus,
    updateOutputTail,
    updateDialogMode,
    systemUpdatePhase,
    updateProgressText,
    updateProgressPercent,
    updateDialogTitle,
    updateDialogVersionLabel,
    updateDialogReleaseLinkLabel,
    loadVersionStatus,
    handleVersionRefresh,
    openVersionReleasePage,
    openReleaseUpdateDialog,
    handleApplySystemUpdate,
    handleRollback,
    checkForUpdate,
    refreshUpdateTaskStatus,
    showDebugUpdateDialog,
    showDebugVersionStatus,
  }
}
