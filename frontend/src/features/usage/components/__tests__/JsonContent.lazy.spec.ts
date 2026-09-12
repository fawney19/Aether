import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, shallowRef, type App } from 'vue'
import JsonContent from '../RequestDetailDrawer/JsonContent.vue'
import JsonContentPanel from '../JsonContentPanel.vue'
import { JSON_PAGE_SIZE, JSON_SCROLL_CHUNK_SIZE, JSON_TEXT_CHUNK_SIZE } from '../../utils/json-viewer'

const mountedApps: Array<{ app: App, root: HTMLElement }> = []
afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

function mountJson(initialData: unknown, expandDepth = 999) {
  const data = shallowRef(initialData)
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(defineComponent({
    setup: () => () => h(JsonContent, { data: data.value, expandDepth, isDark: false, viewMode: 'formatted', emptyMessage: '无数据' }),
  }))
  app.mount(root)
  mountedApps.push({ app, root })
  return { root, data }
}

describe('JsonContent lazy rendering', () => {
  it('scrolls continuously to the last line and back with bounded DOM and complete data', async () => {
    const values = Array.from({ length: 1000 }, (_value, index) => `value-${index}`)
    const { root, data } = mountJson(values)
    await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(JSON_SCROLL_CHUNK_SIZE))
    expect(root.textContent).not.toMatch(/上一页|下一页|第 1 页/)
    const viewport = root.querySelector<HTMLElement>('.virtual-body-scroll')!
    for (let index = 1; index <= 20; index += 1) {
      viewport.scrollTop = index * 1000
      viewport.dispatchEvent(new Event('scroll'))
      await vi.waitFor(() => expect(root.querySelector(`[data-body-chunk="${index}"] .json-line`)).not.toBeNull())
      expect(root.querySelectorAll('.json-line').length).toBeLessThanOrEqual(JSON_PAGE_SIZE)
    }
    expect(root.textContent).toContain('value-999')
    expect(root.textContent).not.toContain('value-0"')
    viewport.scrollTop = 0
    viewport.dispatchEvent(new Event('scroll'))
    await vi.waitFor(() => expect(root.querySelector('.line-number')?.textContent).toBe('1'))
    expect(root.textContent).toContain('value-0')
    expect(data.value).toEqual(values)
    data.value = { replacement: true }
    await vi.waitFor(() => expect(root.textContent).toContain('replacement'))
    expect(root.querySelector<HTMLElement>('.virtual-body-scroll')?.scrollTop).toBe(0)
  })

  it('opens one layer per bracket click after collapsing all descendants', async () => {
    const { root } = mountJson({ messages: [{ content: { text: 'hidden content' } }] }, 0)
    await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(3))
    for (const lineCount of [5, 7, 9]) {
      expect(root.textContent).not.toContain('hidden content')
      root.querySelector<HTMLElement>('.line-content.clickable-collapsed')!.click()
      await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(lineCount))
    }
    await vi.waitFor(() => expect(root.textContent).toContain('hidden content'))
    expect(root.querySelector('button[aria-label="展开节点"]')).toBeNull()
  })

  it('collapses every layer through the toolbar and keeps expand-all working', async () => {
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(defineComponent({ setup: () => () => h(JsonContentPanel, {
      data: { messages: [{ content: { text: 'deep content' } }] }, isDark: false,
    }) }))
    app.mount(root)
    mountedApps.push({ app, root })
    await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(3))
    for (let cycle = 0; cycle < 2; cycle += 1) {
      root.querySelector<HTMLButtonElement>('button[title="展开全部"]')!.click()
      await vi.waitFor(() => expect(root.textContent).toContain('deep content'))
      expect(root.querySelector('button[aria-label="展开节点"]')).toBeNull()
      root.querySelector<HTMLButtonElement>('button[title="收缩全部"]')!.click()
      await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(3))
      for (const lineCount of [5, 7]) {
        root.querySelector<HTMLButtonElement>('button[aria-label="展开节点"]')!.click()
        await vi.waitFor(() => expect(root.querySelectorAll('.json-line')).toHaveLength(lineCount))
        expect(root.textContent).not.toContain('deep content')
      }
    }
  })

  it('preserves an explicitly folded child when its parent is reopened', async () => {
    const { root } = mountJson({ messages: [{ content: { text: 'hidden content' } }] })
    const nodeButton = (key: string) => [...root.querySelectorAll('.json-line')]
      .find(line => line.querySelector('.token-key')?.textContent === JSON.stringify(key))!
      .querySelector<HTMLButtonElement>('button')!
    await vi.waitFor(() => expect(root.textContent).toContain('hidden content'))
    nodeButton('content').click()
    await vi.waitFor(() => expect(nodeButton('content').getAttribute('aria-expanded')).toBe('false'))
    nodeButton('messages').click()
    await vi.waitFor(() => expect(nodeButton('messages').getAttribute('aria-expanded')).toBe('false'))
    nodeButton('messages').click()
    await vi.waitFor(() => expect(nodeButton('messages').getAttribute('aria-expanded')).toBe('true'))
    expect(nodeButton('content').getAttribute('aria-expanded')).toBe('false')
    expect(root.textContent).not.toContain('hidden content')
  })

  it('shows complete long strings without an extra expansion button', async () => {
    const content = `${'x'.repeat(JSON_TEXT_CHUNK_SIZE * 10)  }末尾🙂`
    const { root, data } = mountJson({ content })
    await vi.waitFor(() => expect(root.textContent).toContain('末尾🙂'))
    const displayed = [...root.querySelectorAll('.token-string')].map(element => element.textContent).join('')
    expect(displayed).toBe(JSON.stringify(content))
    expect(root.textContent).not.toMatch(/显示更多|继续显示|剩余.*字符/)
    expect(data.value).toEqual({ content })
  })

  it('automatically streams complete raw text and escapes HTML in JSON strings', async () => {
    const text = `${'x'.repeat(100_000)  }RAW-END`
    const { root } = mountJson(text)
    await vi.waitFor(() => expect(root.querySelector('pre')?.textContent).toHaveLength(16_000))
    const chunks = [root.querySelector('pre')!.textContent]
    const viewport = root.querySelector<HTMLElement>('.virtual-body-scroll')!
    for (let index = 1; index <= 6; index += 1) {
      viewport.scrollTop = index * 1000
      viewport.dispatchEvent(new Event('scroll'))
      await vi.waitFor(() => expect(root.querySelector(`[data-body-chunk="${index}"] pre`)).not.toBeNull())
      chunks.push(root.querySelector(`[data-body-chunk="${index}"] pre`)!.textContent)
    }
    expect(chunks.join('')).toBe(text)
    expect(root.textContent).toContain('RAW-END')
    expect(root.querySelector('button')).toBeNull()
    const escaped = mountJson({ content: '<img src=x onerror="alert(1)">'.repeat(200) }).root
    await vi.waitFor(() => expect(escaped.querySelector('.token-string')).not.toBeNull())
    expect(escaped.querySelector('img')).toBeNull()
    expect(escaped.textContent).toContain('<img')
  })

  it.each([false, 0])('renders %s as a value rather than an empty body', async value => {
    const { root } = mountJson(value)
    await vi.waitFor(() => expect(root.textContent).toContain(String(value)))
    expect(root.textContent).not.toContain('无数据')
  })
})
