import { afterEach, describe, expect, it, vi } from 'vitest'
import { BodyDocument } from '../body-document'
import { BODY_WORKER_TIMEOUT, type BodyWorkerResponse } from '../body-document-protocol'

class FakeWorker {
  static instances: FakeWorker[] = []
  onmessage: ((event: MessageEvent<BodyWorkerResponse>) => void) | null = null
  onerror: (() => void) | null = null
  onmessageerror: (() => void) | null = null
  postMessage = vi.fn()
  terminate = vi.fn()
  constructor() { FakeWorker.instances.push(this) }
  reply(data: BodyWorkerResponse) { this.onmessage?.({ data } as MessageEvent<BodyWorkerResponse>) }
}

afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); FakeWorker.instances = [] })

function start() {
  vi.stubGlobal('Worker', FakeWorker)
  const bytes = new ArrayBuffer(16)
  const controller = new AbortController()
  const loading = BodyDocument.load(bytes, 'gzip', controller.signal)
  const worker = FakeWorker.instances[FakeWorker.instances.length - 1]
  return { bytes, controller, loading, worker }
}

describe('body worker lifecycle', () => {
  it('transfers compressed bytes and receives only a summary before requesting pages', async () => {
    const { bytes, loading, worker } = start()
    expect(worker.postMessage).toHaveBeenCalledWith({ id: 1, action: 'load', bytes, encoding: 'gzip' }, [bytes])
    worker.reply({ id: 1, ok: true, result: { byteLength: 100_000 } })
    const document = await loading
    expect(document.byteLength).toBe(100_000)
    const page = document.json({ page: 0 })
    worker.reply({ id: 2, ok: true, result: { lines: [], hasNext: true } })
    await expect(page).resolves.toEqual({ lines: [], hasNext: true })
    document.dispose()
    expect(worker.terminate).toHaveBeenCalledOnce()
  })

  it('terminates in-progress decompression on abort and ignores a late response', async () => {
    const { loading, controller, worker } = start()
    const rejected = expect(loading).rejects.toHaveProperty('name', 'AbortError')
    controller.abort()
    worker.reply({ id: 1, ok: true, result: { byteLength: 500 } })
    await rejected
    expect(worker.terminate).toHaveBeenCalledOnce()
  })

  it('does not return a disposed handle if abort races with the load response', async () => {
    const { loading, controller, worker } = start()
    const rejected = expect(loading).rejects.toHaveProperty('name', 'AbortError')
    worker.reply({ id: 1, ok: true, result: { byteLength: 500 } })
    controller.abort()
    await rejected
    expect(worker.terminate).toHaveBeenCalledOnce()
  })

  it('terminates workers that fail, cannot deserialize, or time out', async () => {
    vi.useFakeTimers()
    for (const mode of ['error', 'messageerror', 'timeout']) {
      const { loading, worker } = start()
      const rejected = expect(loading).rejects.toHaveProperty('code', mode === 'timeout' ? 'timeout' : 'worker_failed')
      if (mode === 'error') worker.onerror?.()
      else if (mode === 'messageerror') worker.onmessageerror?.()
      else await vi.advanceTimersByTimeAsync(BODY_WORKER_TIMEOUT)
      await rejected
      expect(worker.terminate).toHaveBeenCalledOnce()
    }
  })

  it('does not silently fall back to parsing on the main thread', async () => {
    vi.stubGlobal('Worker', undefined)
    await expect(BodyDocument.load(new ArrayBuffer(1), 'json')).rejects.toHaveProperty('code', 'unsupported')
  })
})
