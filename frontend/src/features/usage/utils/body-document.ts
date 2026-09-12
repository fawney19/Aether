import {
  BODY_WORKER_TIMEOUT, BodyDocumentError,
  type BodyConversationOptions, type BodyConversationPage, type BodyDocumentSummary,
  type BodyEncoding, type BodyJsonOptions, type BodyJsonPage,
  type BodyWorkerCommand, type BodyWorkerResponse, type BodyWorkerResult,
} from './body-document-protocol'

export class BodyDocument {
  readonly kind = 'usage-body-document'
  private sequence = 0
  private disposed = false
  private pending = new Map<number, { resolve: (result: BodyWorkerResult) => void, reject: (error: Error) => void, timer: ReturnType<typeof setTimeout> }>()
  byteLength = 0

  private constructor(private readonly worker: Worker) {
    worker.onmessage = ({ data }: MessageEvent<BodyWorkerResponse>) => {
      const pending = this.pending.get(data.id)
      if (!pending) return
      clearTimeout(pending.timer)
      this.pending.delete(data.id)
      if (data.ok) pending.resolve(data.result)
      else pending.reject(new BodyDocumentError(data.code))
    }
    worker.onerror = () => this.dispose(new BodyDocumentError('worker_failed'))
    worker.onmessageerror = () => this.dispose(new BodyDocumentError('worker_failed'))
  }

  static async load(bytes: ArrayBuffer, encoding: BodyEncoding, signal?: AbortSignal): Promise<BodyDocument> {
    if (signal?.aborted) throw new DOMException('Aborted', 'AbortError')
    if (typeof Worker === 'undefined') throw new BodyDocumentError('unsupported')
    let document: BodyDocument
    try {
      document = new BodyDocument(new Worker(new URL('./body-document.worker.ts', import.meta.url), { type: 'module' }))
    } catch {
      throw new BodyDocumentError('worker_failed')
    }
    const abort = () => document.dispose()
    signal?.addEventListener('abort', abort, { once: true })
    try {
      const result = await document.request<BodyDocumentSummary>({ action: 'load', bytes, encoding }, [bytes])
      if (signal?.aborted) throw new DOMException('Aborted', 'AbortError')
      document.byteLength = result.byteLength
      return document
    } catch (error) {
      document.dispose()
      throw error
    } finally {
      signal?.removeEventListener('abort', abort)
    }
  }

  private request<Result extends BodyWorkerResult>(command: BodyWorkerCommand, transfer: Transferable[] = []): Promise<Result> {
    if (this.disposed) return Promise.reject(new BodyDocumentError('worker_failed'))
    const id = ++this.sequence
    return new Promise<Result>((resolve, reject) => {
      const timer = setTimeout(() => this.dispose(new BodyDocumentError('timeout')), BODY_WORKER_TIMEOUT)
      this.pending.set(id, { resolve: value => resolve(value as Result), reject, timer })
      try {
        this.worker.postMessage({ ...command, id }, transfer)
      } catch {
        this.dispose(new BodyDocumentError('worker_failed'))
      }
    })
  }

  json(options: BodyJsonOptions): Promise<BodyJsonPage> {
    return this.request({ action: 'json', options })
  }

  conversation(options: BodyConversationOptions): Promise<BodyConversationPage> {
    return this.request({ action: 'conversation', options })
  }

  copy(conversation?: BodyConversationOptions): Promise<string> {
    return this.request({ action: 'copy', conversation })
  }

  dispose(error: Error = new DOMException('Aborted', 'AbortError')) {
    if (this.disposed) return
    this.disposed = true
    this.worker.terminate()
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer)
      pending.reject(error)
    }
    this.pending.clear()
  }
}
