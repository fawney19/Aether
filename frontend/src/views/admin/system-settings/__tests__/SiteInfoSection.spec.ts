import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, type App } from 'vue'

import SiteInfoSection from '../SiteInfoSection.vue'

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    success: vi.fn(),
    error: vi.fn(),
  }),
}))

vi.mock('@/components/layout', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    CardSection: defineComponent({
      name: 'CardSectionStub',
      props: {
        title: String,
        description: String,
      },
      setup(props, { slots }) {
        return () => h('section', [
          h('h2', props.title),
          h('p', props.description),
          slots.actions?.(),
          slots.default?.(),
        ])
      },
    }),
  }
})

vi.mock('@/components/ui/button.vue', () => ({
  default: defineComponent({
    name: 'ButtonStub',
    setup(_, { slots }) {
      return () => h('button', slots.default?.())
    },
  }),
}))

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountSection() {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(SiteInfoSection, {
    siteName: 'Aether',
    siteSubtitle: 'AI Gateway',
    showGithubLink: true,
    guideMode: 'builtin',
    guideCustomType: 'url',
    guideUrl: '',
    guideHtml: '',
    loading: false,
    hasChanges: true,
    onSave: vi.fn(),
    'onUpdate:siteName': vi.fn(),
    'onUpdate:siteSubtitle': vi.fn(),
    'onUpdate:showGithubLink': vi.fn(),
    'onUpdate:guideMode': vi.fn(),
    'onUpdate:guideCustomType': vi.fn(),
    'onUpdate:guideUrl': vi.fn(),
    'onUpdate:guideHtml': vi.fn(),
  })
  app.mount(root)
  mountedApps.push({ app, root })
  return { root }
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  document.body.innerHTML = ''
})

describe('SiteInfoSection', () => {
  it('renders site name, github visibility and guide mode fields', async () => {
    const { root } = mountSection()
    await nextTick()

    expect(root.textContent).toContain('站点名称')
    expect(root.textContent).toContain('站点副标题')
    expect(root.textContent).toContain('显示 GitHub 图标')
    expect(root.textContent).toContain('文档页')
  })
})
