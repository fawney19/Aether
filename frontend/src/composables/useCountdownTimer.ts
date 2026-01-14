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
