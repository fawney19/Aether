import { BodyDocumentEngine, decodeBody } from './body-document-engine'
import { BodyDocumentError, type BodyWorkerRequest, type BodyWorkerResponse, type BodyWorkerResult } from './body-document-protocol'

const scope = globalThis as unknown as {
  onmessage: (event: MessageEvent<BodyWorkerRequest>) => void
  postMessage: (message: BodyWorkerResponse) => void
}
let document: BodyDocumentEngine | undefined

scope.onmessage = async ({ data: request }) => {
  try {
    let result: BodyWorkerResult
    if (request.action === 'load') {
      const decoded = await decodeBody(request.bytes, request.encoding)
      document = new BodyDocumentEngine(decoded.value)
      result = { byteLength: decoded.byteLength }
    } else {
      if (!document) throw new BodyDocumentError('worker_failed')
      switch (request.action) {
        case 'json': result = document.json(request.options); break
        case 'conversation': result = document.conversationPage(request.options); break
        case 'copy': result = document.copy(request.conversation); break
      }
    }
    scope.postMessage({ id: request.id, ok: true, result })
  } catch (error) {
    scope.postMessage({ id: request.id, ok: false, code: error instanceof BodyDocumentError ? error.code : 'worker_failed' })
  }
}
