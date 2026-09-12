import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

const vscodexApiMock = vi.hoisted(() => ({
  listDevices: vi.fn(),
  createPairing: vi.fn(),
  createWsTicket: vi.fn(),
  deleteDevice: vi.fn(),
}))

vi.mock('@/api/vscodex', () => ({
  vscodexApi: vscodexApiMock,
}))

vi.mock('@/i18n', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/i18n')>()
  const { ref } = await import('vue')
  const locale = ref('zh-CN')
  return {
    ...actual,
    useI18n: () => ({
      locale,
      t: (key: string) => key,
      legacyT: (value: string) => value,
    }),
  }
})

vi.mock('@/composables/useDarkMode', async () => {
  const { ref } = await import('vue')
  const isDark = ref(false)
  return {
    useDarkMode: () => ({ isDark }),
  }
})

vi.mock('@/components/common', () => ({
  LoadingState: defineComponent({
    name: 'LoadingStateStub',
    setup: () => () => h('div', 'loading'),
  }),
}))

vi.mock('@/components/ui', () => ({
  Button: defineComponent({
    name: 'ButtonStub',
    inheritAttrs: false,
    setup(_, { attrs, slots }) {
      return () => h('button', attrs, slots.default?.())
    },
  }),
}))

import VscodeControl from '../VscodeControl.vue'

const mountedApps: Array<{ app: App; root: HTMLElement }> = []

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((promiseResolve, promiseReject) => {
    resolve = promiseResolve
    reject = promiseReject
  })
  return { promise, resolve, reject }
}

async function settle() {
  for (let index = 0; index < 6; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

async function mountControl() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(VscodeControl)
  app.mount(root)
  mountedApps.push({ app, root })
  await settle()

  const frame = root.querySelector<HTMLIFrameElement>('[data-testid="vscodex-frame"]')
  if (!frame?.contentWindow) throw new Error('Expected VS Code control iframe to be mounted')
  return { app, root, frame, target: frame.contentWindow }
}

function dispatchFrameMessage(options: {
  source: MessageEventSource | null
  origin?: string
  type: string
  version?: number
}) {
  window.dispatchEvent(new MessageEvent('message', {
    data: { v: options.version ?? 1, type: options.type },
    origin: options.origin ?? window.location.origin,
    source: options.source,
  }))
}

beforeEach(() => {
  vi.clearAllMocks()
  vscodexApiMock.listDevices.mockResolvedValue([
    {
      id: 'device-1',
      name: 'Studio Mac',
      status: 'online',
      last_seen_at: null,
      created_at: null,
    },
  ])
  vscodexApiMock.createPairing.mockResolvedValue({
    code: 'PAIR-1234',
    expires_at: null,
    expires_in_seconds: 300,
  })
  vscodexApiMock.createWsTicket.mockResolvedValue({
    ticket: 'ticket-1',
    ws_url: 'wss://aether.example/api/vscodex/ws',
    expires_at: null,
  })
  vscodexApiMock.deleteDevice.mockResolvedValue(undefined)
})

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
})

describe('VscodeControl iframe bridge', () => {
  it('confirms and revokes a device credential from the control header', async () => {
    vscodexApiMock.listDevices
      .mockResolvedValueOnce([{
        id: 'device-1',
        name: 'Studio Mac',
        status: 'online',
        last_seen_at: null,
        created_at: null,
      }])
      .mockResolvedValueOnce([])
    const confirm = vi.spyOn(window, 'confirm').mockReturnValue(true)
    const { root } = await mountControl()

    const revoke = root.querySelector<HTMLButtonElement>('[data-testid="vscodex-revoke-device"]')
    revoke?.click()
    await settle()

    expect(confirm).toHaveBeenCalledOnce()
    expect(vscodexApiMock.deleteDevice).toHaveBeenCalledWith('device-1')
    expect(root.querySelector('[data-testid="vscodex-pairing-state"]')).not.toBeNull()
    confirm.mockRestore()
  })

  it('accepts ready messages only from the exact same-origin iframe window', async () => {
    const { frame, target } = await mountControl()
    const postMessage = vi.spyOn(target, 'postMessage')

    dispatchFrameMessage({
      source: target,
      origin: 'https://untrusted.example',
      type: 'aether-vscodex/ready',
    })
    dispatchFrameMessage({
      source: window,
      type: 'aether-vscodex/ready',
    })
    dispatchFrameMessage({
      source: target,
      type: 'aether-vscodex/ready',
      version: 2,
    })
    await settle()

    expect(frame.src).toContain('/aether-vscodex/index.html?embed=aether')
    expect(vscodexApiMock.createWsTicket).not.toHaveBeenCalled()

    dispatchFrameMessage({
      source: target,
      type: 'aether-vscodex/ready',
    })
    await settle()

    expect(vscodexApiMock.createWsTicket).toHaveBeenCalledOnce()
    expect(vscodexApiMock.createWsTicket).toHaveBeenCalledWith('device-1')
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        v: 1,
        type: 'aether-vscodex/context',
        locale: 'zh-CN',
        theme: 'light',
      }),
      window.location.origin,
    )
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({
        v: 1,
        type: 'aether-vscodex/connect',
        deviceId: 'device-1',
        ticket: 'ticket-1',
        wsUrl: 'wss://aether.example/api/vscodex/ws',
      }),
      window.location.origin,
    )
  })

  it('deduplicates an in-flight ticket request and allows renewal after it settles', async () => {
    const firstTicket = deferred<{
      ticket: string
      ws_url: string
      expires_at: null
    }>()
    vscodexApiMock.createWsTicket
      .mockReturnValueOnce(firstTicket.promise)
      .mockResolvedValueOnce({
        ticket: 'ticket-2',
        ws_url: 'wss://aether.example/api/vscodex/ws',
        expires_at: null,
      })
    const { target } = await mountControl()

    dispatchFrameMessage({ source: target, type: 'aether-vscodex/ready' })
    dispatchFrameMessage({ source: target, type: 'aether-vscodex/request-ticket' })
    await settle()
    expect(vscodexApiMock.createWsTicket).toHaveBeenCalledTimes(1)

    firstTicket.resolve({
      ticket: 'ticket-1',
      ws_url: 'wss://aether.example/api/vscodex/ws',
      expires_at: null,
    })
    await settle()
    dispatchFrameMessage({ source: target, type: 'aether-vscodex/request-ticket' })
    await settle()

    expect(vscodexApiMock.createWsTicket).toHaveBeenCalledTimes(2)
  })

  it('disconnects on unmount and ignores a ticket that resolves afterward', async () => {
    const pendingTicket = deferred<{
      ticket: string
      ws_url: string
      expires_at: null
    }>()
    vscodexApiMock.createWsTicket.mockReturnValueOnce(pendingTicket.promise)
    const { app, root, target } = await mountControl()
    const postMessage = vi.spyOn(target, 'postMessage')

    dispatchFrameMessage({ source: target, type: 'aether-vscodex/ready' })
    await settle()
    app.unmount()
    root.remove()
    mountedApps.splice(0, 1)

    expect(postMessage).toHaveBeenCalledWith(
      { v: 1, type: 'aether-vscodex/disconnect' },
      window.location.origin,
    )

    postMessage.mockClear()
    pendingTicket.resolve({
      ticket: 'stale-ticket',
      ws_url: 'wss://aether.example/api/vscodex/ws',
      expires_at: null,
    })
    await settle()

    expect(postMessage).not.toHaveBeenCalled()
  })
})
