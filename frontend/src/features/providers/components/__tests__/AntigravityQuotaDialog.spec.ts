import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h } from 'vue'

import AntigravityQuotaDialog from '@/features/providers/components/AntigravityQuotaDialog.vue'
import type { QuotaStatusSnapshot, UpstreamMetadata } from '@/api/endpoints/types'
import { createI18n, setI18nLocale, type Locale } from '@/i18n'

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string) => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h('div', [
        slots.headerActions?.(),
        slots.default?.(),
        slots.footer?.(),
      ])
    },
  })

  return {
    Dialog: passthrough('DialogStub'),
    DropdownMenu: passthrough('DropdownMenuStub'),
    DropdownMenuTrigger: passthrough('DropdownMenuTriggerStub'),
    DropdownMenuContent: passthrough('DropdownMenuContentStub'),
    DropdownMenuItem: defineComponent({
      name: 'DropdownMenuItemStub',
      emits: ['select'],
      setup(_, { emit, slots }) {
        return () => h('button', { type: 'button', onClick: () => emit('select') }, slots.default?.())
      },
    }),
  }
})

vi.mock('@/components/ui/button.vue', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    default: defineComponent({
      name: 'ButtonStub',
      setup(_, { attrs, slots }) {
        return () => h('button', { ...attrs, type: 'button' }, slots.default?.())
      },
    }),
  }
})

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })

  return {
    BarChart3: Icon,
    Loader2: Icon,
    Play: Icon,
  }
})

vi.mock('@/api/endpoints/providers', () => ({
  testModel: vi.fn(),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => ({
    error: vi.fn(),
    success: vi.fn(),
  }),
}))

vi.mock('@/utils/errorParser', () => ({
  parseApiError: (value: unknown) => String(value),
}))

function mount(
  metadata: UpstreamMetadata,
  quotaSnapshot?: QuotaStatusSnapshot,
  locale: Locale = 'zh-CN',
) {
  setI18nLocale(locale)
  const root = document.createElement('div')
  document.body.appendChild(root)

  const app = createApp(defineComponent({
    setup() {
      return () => h(AntigravityQuotaDialog, {
        open: true,
        metadata,
        quotaSnapshot,
        keyName: 'Key-1',
      })
    },
  }))
  app.use(createI18n())
  app.mount(root)

  return {
    root,
    unmount: () => {
      app.unmount()
      root.remove()
    },
  }
}

describe('AntigravityQuotaDialog', () => {
  afterEach(() => {
    setI18nLocale('zh-CN')
  })

  it('renders only compact localized quota-group windows in Chinese', () => {
    const { root, unmount } = mount({}, {
      code: 'ok',
      provider_type: 'antigravity',
      exhausted: false,
      windows: [{
        code: 'group:0:gemini-weekly',
        label: 'Gemini Models · Weekly Limit Remaining',
        scope: 'quota_group',
        quota_group_label: 'Gemini models',
        bucket_id: 'gemini-weekly',
        window: 'weekly',
        used_ratio: 0.1,
        remaining_ratio: 0.9,
      }, {
        code: 'group:1:3p-5h',
        label: 'Claude and GPT models · 5 hour',
        scope: 'quota_group',
        quota_group_label: 'Claude and GPT models',
        bucket_id: '3p-5h',
        window: '5h',
        used_ratio: 0.75,
        remaining_ratio: 0.25,
      }, {
        code: 'group:1:3p-weekly',
        label: 'Claude and GPT models · Weekly Limit Remaining',
        scope: 'quota_group',
        quota_group_label: 'Claude and GPT models',
        bucket_id: '3p-weekly',
        window: 'weekly',
        used_ratio: 0.2,
        remaining_ratio: 0.8,
      }, {
        code: 'model:gemini-3.7-flash-tiered',
        label: 'Gemini 3.7 Flash',
        scope: 'model',
        model: 'gemini-3.7-flash-tiered',
        used_ratio: 0.05,
        remaining_ratio: 0.95,
      }],
    })
    const text = root.textContent || ''

    expect(text).toContain('Gemini Models · 周')
    expect(text).toContain('90.0%')
    expect(text).toContain('Claude and GPT models · 5小时')
    expect(text).toContain('25.0%')
    expect(text).toContain('Claude and GPT models · 周')
    expect(text).toContain('80.0%')
    expect(text).not.toContain('Weekly Limit Remaining')
    expect(text).not.toContain('Gemini额度')
    expect(text).not.toContain('Claude & ChatGPT')
    expect(text).not.toContain('Gemini 3.7 Flash')

    unmount()
  })

  it('uses Weekly and 5 Hours for the English locale', () => {
    const { root, unmount } = mount({}, {
      code: 'ok',
      provider_type: 'antigravity',
      exhausted: false,
      windows: [{
        code: 'group:0:gemini-weekly',
        label: 'Gemini Models · Weekly Limit Remaining',
        scope: 'quota_group',
        quota_group_label: 'Gemini Models',
        bucket_id: 'gemini-weekly',
        window: 'weekly',
        remaining_ratio: 0.9,
      }, {
        code: 'group:1:3p-5h',
        label: 'Claude and GPT models · 5 hour',
        scope: 'quota_group',
        quota_group_label: 'Claude and GPT models',
        bucket_id: '3p-5h',
        window: '5h',
        remaining_ratio: 0.25,
      }],
    }, 'en-US')
    const text = root.textContent || ''

    expect(text).toContain('Gemini Models · Weekly')
    expect(text).toContain('Claude and GPT models · 5 Hours')
    expect(text).not.toContain('Weekly Limit Remaining')

    unmount()
  })

  it('does not fall back to the removed model-family summaries', () => {
    const { root, unmount } = mount({
      antigravity: {
        quota_by_model: {
          'gemini-3-flash-agent': {
            display_name: 'Gemini 3.5 Flash (High)',
            remaining_fraction: 0.9,
            used_percent: 10,
          },
          'claude-opus-4-6-thinking': {
            display_name: 'Claude Opus 4.6 (Thinking)',
            remaining_fraction: 1,
            used_percent: 0,
          },
        },
      },
    })

    expect(root.textContent).toContain('暂无配额数据')
    expect(root.textContent).not.toContain('Gemini额度')
    expect(root.textContent).not.toContain('Claude & ChatGPT')
    expect(root.textContent).not.toContain('Gemini 3.5 Flash (High)')
    expect(root.textContent).not.toContain('Claude Opus 4.6 (Thinking)')

    unmount()
  })
})
