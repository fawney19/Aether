import { describe, expect, it } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref } from 'vue'

import JsonImportInput from '@/components/common/JsonImportInput.vue'
import { createI18n } from '@/i18n'

describe('JsonImportInput', () => {
  it('keeps both mode panels in one layout track while switching modes', async () => {
    const value = ref('')
    const Host = defineComponent({
      setup() {
        return () => h(JsonImportInput, {
          modelValue: value.value,
          'onUpdate:modelValue': (nextValue: string) => {
            value.value = nextValue
          },
        })
      },
    })
    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(Host)
    app.use(createI18n())
    app.mount(root)

    const panels = root.querySelector<HTMLElement>('[data-testid="json-import-mode-panels"]')
    const filePanel = root.querySelector<HTMLElement>('[data-testid="json-import-file-panel"]')
    const manualPanel = root.querySelector<HTMLElement>('[data-testid="json-import-manual-panel"]')
    expect(panels?.classList.contains('grid')).toBe(true)
    expect(filePanel?.getAttribute('aria-hidden')).toBe('false')
    expect(filePanel?.hasAttribute('inert')).toBe(false)
    expect(manualPanel?.getAttribute('aria-hidden')).toBe('true')
    expect(manualPanel?.hasAttribute('inert')).toBe(true)

    root.querySelector<HTMLButtonElement>('[data-testid="json-import-mode-toggle"]')?.click()
    await nextTick()

    expect(root.querySelector('[data-testid="json-import-file-panel"]')).toBe(filePanel)
    expect(root.querySelector('[data-testid="json-import-manual-panel"]')).toBe(manualPanel)
    expect(filePanel?.getAttribute('aria-hidden')).toBe('true')
    expect(filePanel?.hasAttribute('inert')).toBe(true)
    expect(manualPanel?.getAttribute('aria-hidden')).toBe('false')
    expect(manualPanel?.hasAttribute('inert')).toBe(false)

    const textarea = root.querySelector<HTMLTextAreaElement>('textarea')
    if (!textarea) throw new Error('Expected manual textarea')
    textarea.value = 'credential-value'
    textarea.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(value.value).toBe('credential-value')

    root.querySelector<HTMLButtonElement>('[data-testid="json-import-mode-toggle"]')?.click()
    await nextTick()
    expect(value.value).toBe('')
    expect(filePanel?.hasAttribute('inert')).toBe(false)
    expect(manualPanel?.hasAttribute('inert')).toBe(true)

    app.unmount()
    root.remove()
  })
})
