import { ref, onUnmounted } from 'vue'

/**
 * 倒计时定时器 composable
 * 用于触发组件的定期响应式更新（如熔断探测倒计时）
 */
export function useCountdownTimer() {
  const tick = ref(0)
  let timer: ReturnType<typeof setInterval> | null = null

  function start() {
    if (timer) return
    timer = setInterval(() => {
      tick.value++
    }, 1000)
  }

  function stop() {
    if (timer) {
      clearInterval(timer)
      timer = null
    }
  }

  onUnmounted(stop)

  return { tick, start, stop }
}

/**
 * 格式化倒计时时间
 * @param diffMs 剩余毫秒数
 * @returns 格式化的倒计时字符串（如 "1:30" 或 "1:02:30"）
 */
export function formatCountdown(diffMs: number): string {
  const totalSeconds = Math.ceil(diffMs / 1000)
  if (totalSeconds <= 0) return '探测中'

  const hours = Math.floor(totalSeconds / 3600)
  const minutes = Math.floor((totalSeconds % 3600) / 60)
  const seconds = totalSeconds % 60

  if (hours > 0) {
    return `${hours}:${minutes.toString().padStart(2, '0')}:${seconds.toString().padStart(2, '0')}`
  }
  return `${minutes}:${seconds.toString().padStart(2, '0')}`
}

/**
 * 计算探测倒计时
 * @param nextProbeAt ISO 格式的探测时间字符串
 * @param _tick 响应式触发器（传入 tick.value 以触发响应式更新）
 * @returns 倒计时字符串，或 null（如果无需显示）
 */
export function getProbeCountdown(nextProbeAt: string | null | undefined, _tick: number): string | null {
  // _tick 参数用于触发响应式更新，实际使用时传入 tick.value
  void _tick

  if (!nextProbeAt) return null

  const nextProbe = new Date(nextProbeAt)
  const now = new Date()
  const diffMs = nextProbe.getTime() - now.getTime()

  if (diffMs > 0) {
    return formatCountdown(diffMs)
  }
  return '探测中'
}

/**
 * Codex 配额重置倒计时状态
 */
export interface CodexResetStatus {
  text: string
  isUrgent: boolean
  isCritical: boolean
  isExpired: boolean
}

/**
 * 计算 Codex 配额重置倒计时
 * @param resetAt 绝对重置时间（Unix 秒）
 * @param resetSecs 相对剩余秒数（用于 fallback）
 * @param updatedAt 元数据更新时间（Unix 秒）
 * @param _tick 响应式触发器（传入 tick.value 以触发响应式更新）
 * @param remainingPercent 当前窗口剩余额度百分比（0-100，100 表示满额不启动倒计时）
 */
export function getCodexResetCountdown(
  resetAt: number | null | undefined,
  resetSecs: number | null | undefined,
  updatedAt: number | null | undefined,
  _tick: number,
  remainingPercent?: number | null
): CodexResetStatus | null {
  void _tick

  if (remainingPercent != null) {
    const normalizedRemaining = Number(remainingPercent)
    if (Number.isFinite(normalizedRemaining) && normalizedRemaining >= 100) {
      return null
    }
  }

  const nowSec = Math.floor(Date.now() / 1000)
  let remaining: number

  if (resetAt != null && resetAt > 0) {
    // 优先绝对时间戳，避免相对秒数快照漂移。
    remaining = resetAt - nowSec
  } else if (resetSecs != null && resetSecs >= 0) {
    if (updatedAt != null && updatedAt > 0) {
      // 时钟偏移下 updatedAt 可能晚于当前时间，elapsed 需要下限钳制到 0。
      const elapsedSec = Math.max(nowSec - updatedAt, 0)
      remaining = resetSecs - elapsedSec
    } else {
      remaining = resetSecs
    }
  } else {
    return null
  }

  if (remaining <= 0) {
    return { text: '已重置', isUrgent: false, isCritical: false, isExpired: true }
  }

  const total = Math.floor(remaining)
  const days = Math.floor(total / 86400)
  const hours = Math.floor((total % 86400) / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  const pad = (n: number) => n.toString().padStart(2, '0')

  let text: string
  if (days > 0) {
    text = `${days}天 ${hours}:${pad(minutes)}:${pad(seconds)}`
  } else if (hours > 0) {
    text = `${hours}:${pad(minutes)}:${pad(seconds)}`
  } else {
    text = `${minutes}:${pad(seconds)}`
  }

  return {
    text,
    isUrgent: total < 3600,
    isCritical: total < 300,
    isExpired: false,
  }
}

/**
 * OAuth Token 状态信息
 */
export interface OAuthStatusInfo {
  text: string
  isExpired: boolean
  isExpiringSoon: boolean
  isInvalid: boolean  // Token 已失效（账号被封、授权撤销等）
  invalidReason?: string  // 失效原因
}

/**
 * 格式化 OAuth Token 过期倒计时
 * @param expiresAt Unix 时间戳（秒）
 * @param _tick 响应式触发器（传入 tick.value 以触发响应式更新）
 * @param invalidAt 失效时间戳（秒），可选
 * @param invalidReason 失效原因，可选
 * @returns 状态信息对象
 */
export function getOAuthExpiresCountdown(
  expiresAt: number | null | undefined,
  _tick: number,
  invalidAt?: number | null,
  invalidReason?: string | null
): OAuthStatusInfo | null {
  void _tick

  // 优先检查失效状态（失效比过期更严重）
  if (invalidAt != null) {
    return {
      text: '已失效',
      isExpired: false,
      isExpiringSoon: false,
      isInvalid: true,
      invalidReason: invalidReason || undefined
    }
  }

  if (expiresAt == null) return null

  const now = Math.floor(Date.now() / 1000)
  const diffSeconds = expiresAt - now

  if (diffSeconds <= 0) {
    return { text: '已过期', isExpired: true, isExpiringSoon: false, isInvalid: false }
  }

  // 24 小时内过期视为即将过期
  const isExpiringSoon = diffSeconds < 24 * 3600

  // 格式化时间
  const days = Math.floor(diffSeconds / 86400)
  const hours = Math.floor((diffSeconds % 86400) / 3600)
  const minutes = Math.floor((diffSeconds % 3600) / 60)

  let text: string
  if (days > 0) {
    text = `${days}天${hours}时`
  } else if (hours > 0) {
    text = `${hours}时${minutes}分`
  } else {
    text = `${minutes}分钟`
  }

  return { text, isExpired: false, isExpiringSoon, isInvalid: false }
}
