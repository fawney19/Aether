import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, shallowRef, type App } from 'vue'
import JsonContent from '../RequestDetailDrawer/JsonContent.vue'
import BodyConversationContent from '../RequestDetailDrawer/BodyConversationContent.vue'
import { BodyDocumentEngine } from '../../utils/body-document-engine'
import type { BodyDocument } from '../../utils/body-document'
import type { BodyJsonOptions } from '../../utils/body-document-protocol'

const apps: Array<{ app: App, root: HTMLElement }> = []
afterEach(() => { for (const { app, root } of apps.splice(0)) { app.unmount(); root.remove() } })

function mountBody(value: unknown) {
    const engine = new BodyDocumentEngine(value)
  const json = vi.fn(async (options: BodyJsonOptions) => engine.json(options))
  const bodyDocument = shallowRef({ json } as unknown as BodyDocument)
  const expandDepth = shallowRef(999)
  const errors: unknown[] = []
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({ setup: () => () => h(JsonContent, {
    data: null, bodyDocument: bodyDocument.value, expandDepth: expandDepth.value, viewMode: 'formatted', isDark: false, emptyMessage: '无数据',
    onLoadError: (error: unknown) => errors.push(error),
  }) }))
  app.mount(root)
  apps.push({ app, root })
  return { root, json, bodyDocument, expandDepth, engine, errors }
}
function button(root: HTMLElement, label: string) { return [...root.querySelectorAll('button')].find(button => button.textContent?.includes(label))! }

describe('worker-backed body views', () => {
  it('resets every layer on collapse-all and reopens worker-backed nodes one layer at a time', async () => {
    const value = { messages: [{ content: { text: 'deep worker content' } }] }
    const { root, json, expandDepth, engine } = mountBody(value)
    await vi.waitFor(() => expect(root.textContent).toContain('deep worker content'))
    for (let cycle = 0; cycle < 2; cycle += 1) {
      expandDepth.value = 0
      await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(3))
      expect(json).toHaveBeenLastCalledWith(expect.objectContaining({ expandDepth: 0, foldOverrides: new Map() }))
      for (const lineCount of [5, 7, 9]) {
        expect(root.textContent).not.toContain('deep worker content')
        root.querySelector<HTMLButtonElement>('button[aria-label="展开节点"]')!.click()
        await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(lineCount))
      }
      expect(root.textContent).toContain('deep worker content')
      expandDepth.value = 999
      await vi.waitFor(() => expect(json).toHaveBeenLastCalledWith(expect.objectContaining({ expandDepth: 999, foldOverrides: new Map() })))
      await vi.waitFor(() => expect(root.textContent).toContain('deep worker content'))
      expect(root.querySelector('button[aria-label="展开节点"]')).toBeNull()
    }
    expect(JSON.parse(engine.copy())).toEqual(value)
  })

  it('automatically reads worker chunks while scrolling and preserves the complete copy', async () => {
    const values = Array.from({ length: 1000 }, (_value, index) => `value-${index}`)
    const { root, json, engine } = mountBody(values)
    await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(50))
    expect(root.textContent).not.toMatch(/上一页|下一页|第 1 页/)
    const viewport = root.querySelector<HTMLElement>('.virtual-body-scroll')!
    for (let index = 1; index <= 20; index += 1) {
      viewport.scrollTop = index * 1000
      viewport.dispatchEvent(new Event('scroll'))
      await vi.waitFor(() => expect(root.querySelector(`[data-body-chunk="${index}"] .json-line`)).not.toBeNull())
      expect(root.querySelectorAll('.json-line').length).toBeLessThanOrEqual(200)
    }
    expect(root.textContent).toContain('value-999')
    expect(json).toHaveBeenLastCalledWith(expect.objectContaining({ page: 20, pageSize: 50 }))
    expect(JSON.parse(engine.copy())).toEqual(values)
    viewport.scrollTop = 0
    viewport.dispatchEvent(new Event('scroll'))
    await vi.waitFor(() => expect(root.querySelector('.line-number')?.textContent).toBe('1'))
  })

  it('streams every part of a long string without a show-more button', async () => {
    const text = `${'x'.repeat(300_000)  }STRING-END`
    const { root, json, engine } = mountBody({ text })
    await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(50))
    expect(root.textContent).not.toMatch(/显示更多|继续显示|STRING-END/)
    const viewport = root.querySelector<HTMLElement>('.virtual-body-scroll')!
    for (let index = 1; index <= 3; index += 1) {
      viewport.scrollTop = index * 1000
      viewport.dispatchEvent(new Event('scroll'))
      await vi.waitFor(() => expect(root.querySelector(`[data-body-chunk="${index}"] .json-line`)).not.toBeNull())
      expect(root.querySelectorAll('.json-line').length).toBeLessThanOrEqual(200)
    }
    expect(root.textContent).toContain('STRING-END')
    expect(json).toHaveBeenLastCalledWith(expect.objectContaining({ page: 3, pageSize: 50 }))
    expect(JSON.parse(engine.copy())).toEqual({ text })
  })

  it('folds nodes further down without resetting scroll position or dropping later content', async () => {
    const values = Array.from({ length: 300 }, (_value, index) => ({ content: `message-${index}` }))
    const { root, json, engine } = mountBody(values)
    await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(50))
    const viewport = root.querySelector<HTMLElement>('.virtual-body-scroll')!
    for (let index = 1; index <= 3; index += 1) {
      viewport.scrollTop = index * 1000
      viewport.dispatchEvent(new Event('scroll'))
      await vi.waitFor(() => expect(root.querySelector(`[data-body-chunk="${index}"] .json-line`)).not.toBeNull())
    }
    root.querySelector<HTMLButtonElement>('[data-body-chunk="3"] button[aria-label="折叠节点"]')!.click()
    await vi.waitFor(() => expect(root.querySelector('[data-body-chunk="3"] button[aria-label="展开节点"]')).not.toBeNull())
    expect(viewport.scrollTop).toBe(3000)
    expect(json).toHaveBeenLastCalledWith(expect.objectContaining({ page: 3, foldOverrides: expect.any(Map) }))
    viewport.scrollTop = 4000
    viewport.dispatchEvent(new Event('scroll'))
    await vi.waitFor(() => expect(root.querySelector('[data-body-chunk="4"] .json-line')).not.toBeNull())
    expect(JSON.parse(engine.copy())).toEqual(values)
  })

  it('ignores obsolete worker replies and propagates active worker failures', async () => {
    const { root, bodyDocument, errors } = mountBody({ initial: true })
    await vi.waitFor(() => expect(root.textContent).toContain('initial'))
    let complete!: (value: ReturnType<BodyDocumentEngine['json']>) => void
    bodyDocument.value = { json: () => new Promise(resolve => { complete = resolve }) } as unknown as BodyDocument
    await nextTick()
    bodyDocument.value = { json: async () => new BodyDocumentEngine({ replacement: true }).json() } as unknown as BodyDocument
    await vi.waitFor(() => expect(root.textContent).toContain('replacement'))
    complete(new BodyDocumentEngine({ stale: true }).json())
    await nextTick()
    expect(root.textContent).not.toContain('stale')
    const error = new Error('worker failed')
    bodyDocument.value = { json: async () => { throw error } } as unknown as BodyDocument
    await vi.waitFor(() => expect(errors).toEqual([error]))
  })

  it('scrolls through bounded conversation previews without page controls or nested scrollbars', async () => {
    const conversation = vi.fn(async ({ page }: { page: number }) => ({ result: { blocks: [{ type: 'text', content: `message ${page}` }] }, hasNext: page < 8, truncated: true }))
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({ setup: () => () => h(BodyConversationContent, {
      bodyDocument: { conversation } as unknown as BodyDocument, kind: 'request', apiFormat: 'openai:chat', emptyMessage: '无数据',
    }) }))
    app.mount(root)
    apps.push({ app, root })
    await vi.waitFor(() => expect(root.textContent).toContain('message 0'))
    expect(root.textContent).toContain('复制仍保留完整内容')
    expect(root.textContent).not.toMatch(/上一页|下一页|对话第/)
    const viewport = root.querySelector<HTMLElement>('.virtual-body-scroll')!
    for (let index = 1; index <= 8; index += 1) {
      viewport.scrollTop = index * 1000
      viewport.dispatchEvent(new Event('scroll'))
      await vi.waitFor(() => expect(root.textContent).toContain(`message ${index}`))
      expect(root.querySelectorAll('[data-body-chunk]').length).toBeLessThanOrEqual(4)
    }
    expect(root.textContent).not.toContain('message 0')
    expect(root.querySelectorAll('.overflow-y-auto')).toHaveLength(0)
    viewport.scrollTop = 0
    viewport.dispatchEvent(new Event('scroll'))
    await vi.waitFor(() => expect(root.textContent).toContain('message 0'))
    button(root, '显示更多').click()
    await vi.waitFor(() => expect(conversation).toHaveBeenLastCalledWith(expect.objectContaining({ page: 0, previewLimit: 128_000 })))
  })
})
