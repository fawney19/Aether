import { describe, expect, it } from 'vitest'
import { createApp, nextTick } from 'vue'

import ThemeModeButton from '@/components/common/ThemeModeButton.vue'
import { useDarkMode } from '@/composables/useDarkMode'
import { createI18n } from '@/i18n'

describe('ThemeModeButton', () => {
  it('cycles the shared theme mode from the single button entry point', async () => {
    const { setThemeMode } = useDarkMode()
    setThemeMode('system')
    await nextTick()

    const root = document.createElement('div')
    document.body.appendChild(root)
    const app = createApp(ThemeModeButton)
    app.use(createI18n())
    app.mount(root)

    const button = root.querySelector('button')
    expect(button).toBeTruthy()
    expect(button?.getAttribute('title')).toBe('跟随系统')

    button?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await nextTick()

    expect(button?.getAttribute('title')).toBe('浅色模式')
    expect(localStorage.getItem('theme')).toBe('light')

    button?.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await nextTick()

    expect(button?.getAttribute('title')).toBe('深色模式')
    expect(localStorage.getItem('theme')).toBe('dark')

    app.unmount()
    root.remove()
  })
})
