import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, reactive, type App } from 'vue'
import EndpointRulesRevealDialog from '../EndpointRulesRevealDialog.vue'
import type { ProviderEndpointRules } from '@/api/endpoints'

const api = vi.hoisted(() => ({ revealEndpointRules: vi.fn() }))
vi.mock('@/api/endpoints', () => api)
vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    Dialog: defineComponent({
      props: { modelValue: Boolean },
      setup: (props, { slots }) => () => props.modelValue
        ? h('section', [slots.default?.(), slots.footer?.()]) : null,
    }),
    Button: defineComponent({ setup: (_props, { slots }) => () => h('button', slots.default?.()) }),
    Textarea: defineComponent({
      props: { modelValue: String },
      setup: (props) => () => h('textarea', { value: props.modelValue }),
    }),
  }
})

const mounted: Array<{ app: App, root: HTMLElement }> = []
const savedRules: ProviderEndpointRules = {
  header_rules: [{ action: 'set', key: 'x-auth', value: 'request-secret' }],
  body_rules: [{ action: 'set', path: 'auth.token', value: 'body-secret' }],
  response_header_rules: [{ action: 'set', key: 'x-auth', value: 'response-secret' }],
}

async function settle() {
  for (let index = 0; index < 5; index += 1) {
    await Promise.resolve()
    await nextTick()
  }
}

function mountDialog(open = true) {
  const props = reactive({ modelValue: open, endpointId: 'endpoint-1' as string | null })
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({
    setup: () => () => h(EndpointRulesRevealDialog, {
      ...props,
      'onUpdate:modelValue': (value: boolean) => { props.modelValue = value },
    }),
  }))
  app.mount(root)
  mounted.push({ app, root })
  return { props, root, app }
}

beforeEach(() => {
  api.revealEndpointRules.mockReset().mockResolvedValue(savedRules)
})

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('endpoint rule reveal', () => {
  it('fetches only on demand and displays saved rules read-only', async () => {
    const { props, root } = mountDialog(false)
    await settle()
    expect(api.revealEndpointRules).not.toHaveBeenCalled()
    props.modelValue = true
    await settle()
    expect(api.revealEndpointRules).toHaveBeenCalledOnce()
    expect(api.revealEndpointRules.mock.calls[0][0]).toBe('endpoint-1')
    const textarea = root.querySelector('textarea')!
    expect(JSON.parse(textarea.value)).toEqual(savedRules)
    expect(textarea.readOnly).toBe(true)
  })

  it('clears plaintext on close and fetches again instead of caching', async () => {
    const { props, root } = mountDialog()
    await settle()
    const signal = api.revealEndpointRules.mock.calls[0][1] as AbortSignal
    root.querySelector('button')!.click()
    await settle()
    expect(signal.aborted).toBe(true)
    expect(root.querySelector('textarea')).toBeNull()
    api.revealEndpointRules.mockResolvedValue({ ...savedRules, body_rules: [] })
    props.modelValue = true
    await settle()
    expect(api.revealEndpointRules).toHaveBeenCalledTimes(2)
    expect(JSON.parse(root.querySelector('textarea')!.value).body_rules).toEqual([])
  })

  it('ignores an old endpoint response after switching endpoints', async () => {
    let resolveFirst!: (rules: ProviderEndpointRules) => void
    api.revealEndpointRules.mockImplementationOnce(() => new Promise(resolve => { resolveFirst = resolve }))
    const { props, root } = mountDialog()
    await settle()
    const oldSignal = api.revealEndpointRules.mock.calls[0][1] as AbortSignal
    props.endpointId = 'endpoint-2'
    api.revealEndpointRules.mockResolvedValue({ header_rules: [], body_rules: [], response_header_rules: [] })
    await settle()
    expect(oldSignal.aborted).toBe(true)
    resolveFirst(savedRules)
    await settle()
    expect(JSON.parse(root.querySelector('textarea')!.value).header_rules).toEqual([])
  })

  it('ignores an in-flight response after the dialog closes', async () => {
    let resolveRequest!: (rules: ProviderEndpointRules) => void
    api.revealEndpointRules.mockImplementationOnce(() => new Promise(resolve => { resolveRequest = resolve }))
    const { props, root } = mountDialog()
    await settle()
    props.modelValue = false
    await settle()
    resolveRequest(savedRules)
    await settle()
    expect(root.querySelector('textarea')).toBeNull()
    expect(root.innerHTML).not.toContain('secret')
  })

  it('does not expose API error details in the dialog', async () => {
    api.revealEndpointRules.mockRejectedValue(new Error('Bearer upstream-secret'))
    const { root } = mountDialog()
    await settle()
    expect(root.querySelector('[role="alert"]')).not.toBeNull()
    expect(root.innerHTML).not.toContain('upstream-secret')
  })
})
