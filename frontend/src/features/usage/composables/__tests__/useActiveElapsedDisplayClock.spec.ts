import { afterEach, describe, expect, it, vi } from 'vitest'
import { nextTick, ref } from 'vue'
import { useActiveElapsedDisplayClock } from '../useActiveElapsedDisplayClock'

type TestRecord = {
  status: string
}

afterEach(() => {
  vi.useRealTimers()
})

describe('useActiveElapsedDisplayClock', () => {
  it('ticks only while visible active records exist', async () => {
    vi.useFakeTimers()
    let nowMs = 1_000

    const records = ref<TestRecord[]>([])
    const isPageVisible = ref(true)
    const serverClockOffsetMs = ref(0)
    const hasServerClockOffset = ref(false)
    const clock = useActiveElapsedDisplayClock({
      records,
      isPageVisible,
      serverClockOffsetMs,
      hasServerClockOffset,
      resolveStatus: record => record.status,
      intervalMs: 250,
      now: () => nowMs,
    })

    expect(clock.hasVisibleActiveRecords.value).toBe(false)
    expect(clock.displayNowMs.value).toBe(1_000)

    nowMs = 1_250
    vi.advanceTimersByTime(250)
    expect(clock.displayNowMs.value).toBe(1_000)

    records.value = [{ status: 'streaming' }]
    await nextTick()
    expect(clock.hasVisibleActiveRecords.value).toBe(true)
    expect(clock.displayNowMs.value).toBe(1_250)

    nowMs = 1_500
    vi.advanceTimersByTime(250)
    expect(clock.displayNowMs.value).toBe(1_500)

    isPageVisible.value = false
    await nextTick()
    nowMs = 1_750
    vi.advanceTimersByTime(250)
    expect(clock.displayNowMs.value).toBe(1_500)

    clock.stopActiveElapsedDisplayClock()
  })

  it('applies server clock offset to the display time', () => {
    vi.useFakeTimers()

    const clock = useActiveElapsedDisplayClock({
      records: ref<TestRecord[]>([{ status: 'pending' }]),
      isPageVisible: ref(true),
      serverClockOffsetMs: ref(-11_000),
      hasServerClockOffset: ref(true),
      resolveStatus: record => record.status,
      intervalMs: 250,
      now: () => 2_000,
    })

    expect(clock.calibratedDisplayNowMs.value).toBe(-9_000)
    clock.stopActiveElapsedDisplayClock()
  })

  it('stops when active records disappear', async () => {
    vi.useFakeTimers()
    let nowMs = 3_000

    const records = ref<TestRecord[]>([{ status: 'pending' }])
    const clock = useActiveElapsedDisplayClock({
      records,
      isPageVisible: ref(true),
      serverClockOffsetMs: ref(0),
      hasServerClockOffset: ref(false),
      resolveStatus: record => record.status,
      intervalMs: 250,
      now: () => nowMs,
    })

    nowMs = 3_250
    vi.advanceTimersByTime(250)
    expect(clock.displayNowMs.value).toBe(3_250)

    records.value = [{ status: 'completed' }]
    await nextTick()
    nowMs = 3_500
    vi.advanceTimersByTime(250)
    expect(clock.displayNowMs.value).toBe(3_250)

    clock.stopActiveElapsedDisplayClock()
  })
})
