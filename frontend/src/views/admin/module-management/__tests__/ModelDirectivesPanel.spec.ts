import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, defineComponent, h, nextTick, ref, type App } from 'vue'

import ModelDirectivesPanel from '../ModelDirectivesPanel.vue'
import { createDefaultModelDirectivesConfig } from '../modelDirectivesConfig'

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = (tag: string) => defineComponent({
    inheritAttrs: false,
    setup(_, { attrs, slots }) {
      return () => h(tag, attrs, slots.default?.())
    },
  })
  return {
    Button: passthrough('button'),
    Select: defineComponent({
      inheritAttrs: false,
      props: { modelValue: String, disabled: Boolean },
      emits: ['update:modelValue'],
      setup(props, { emit, slots }) {
        return () => h('select', {
          value: props.modelValue,
          disabled: props.disabled,
          onChange: (event: Event) => emit(
            'update:modelValue',
            (event.target as HTMLSelectElement).value,
          ),
        }, slots.default?.())
      },
    }),
    SelectContent: passthrough('optgroup'),
    SelectItem: defineComponent({
      inheritAttrs: false,
      props: { value: { type: String, required: true } },
      setup(props, { slots }) {
        return () => h('option', { value: props.value }, slots.default?.())
      },
    }),
    SelectTrigger: defineComponent({ setup: () => () => null }),
    SelectValue: defineComponent({ setup: () => () => null }),
  }
})

vi.mock('@/components/ui/switch.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      inheritAttrs: false,
      props: { modelValue: Boolean, disabled: Boolean },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit }) {
        return () => h('button', {
          ...attrs,
          type: 'button',
          disabled: props.disabled,
          'aria-pressed': props.modelValue,
          onClick: () => emit('update:modelValue', !props.modelValue),
        })
      },
    }),
  }
})

vi.mock('@/components/ui/textarea.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      inheritAttrs: false,
      props: { modelValue: String, disabled: Boolean },
      emits: ['update:modelValue'],
      setup(props, { attrs, emit }) {
        return () => h('textarea', {
          ...attrs,
          value: props.modelValue,
          disabled: props.disabled,
          onInput: (event: Event) => emit(
            'update:modelValue',
            (event.target as HTMLTextAreaElement).value,
          ),
        })
      },
    }),
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountPanel(onSave?: (value: ReturnType<typeof createDefaultModelDirectivesConfig>) => void) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const config = ref(createDefaultModelDirectivesConfig())
  const app = createApp(defineComponent({
    setup() {
      return () => h(ModelDirectivesPanel, {
        config: config.value,
        loading: false,
        onSave,
      })
    },
  }))
  app.mount(root)
  mountedApps.push({ app, root })
  return { config, root }
}

function selectSuffix(select: HTMLSelectElement, suffix: string) {
  select.value = suffix
  select.dispatchEvent(new Event('change', { bubbles: true }))
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('ModelDirectivesPanel', () => {
  it('shows Codex ultra and authoritative custom suffixes in the OpenAI selector', async () => {
    const { config, root } = mountPanel()
    const suffixSelect = root.querySelectorAll('select').item(1) as HTMLSelectElement
    expect([...suffixSelect.options].map(option => option.value)).toContain('ultra')

    const current = config.value.reasoning_effort.api_formats['openai:responses']
    config.value = {
      ...config.value,
      reasoning_effort: {
        ...config.value.reasoning_effort,
        api_formats: {
          ...config.value.reasoning_effort.api_formats,
          'openai:responses': {
            ...current,
            suffixes: [...current.suffixes, 'vendor-future'],
            mappings: {
              ...current.mappings,
              'mapped-future': { vendor_option: true },
            },
          },
        },
      },
    }
    await nextTick()

    expect([...suffixSelect.options].map(option => option.value)).toEqual(expect.arrayContaining([
      'ultra',
      'vendor-future',
      'mapped-future',
    ]))
    selectSuffix(suffixSelect, 'ultra')
    await nextTick()
    expect(root.textContent).toContain('Codex Ultra 预设，请求推理强度为 max')
  })

  it('retains per-suffix drafts and validation state while switching suffixes', async () => {
    const { root } = mountPanel()
    const suffixSelect = root.querySelectorAll('select').item(1) as HTMLSelectElement
    const noneMapping = root.querySelector(
      'textarea[aria-label="OpenAI Responses none 映射参数"]',
    ) as HTMLTextAreaElement

    noneMapping.value = '{'
    noneMapping.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    const saveButton = root.querySelector(
      'button[aria-label="保存 OpenAI Responses 映射参数"]',
    ) as HTMLButtonElement
    saveButton.click()
    await nextTick()
    expect(root.textContent).toContain('JSON 格式无效，请修正后再保存')

    selectSuffix(suffixSelect, 'medium')
    await nextTick()
    selectSuffix(suffixSelect, 'none')
    await nextTick()

    const restoredDraft = root.querySelector(
      'textarea[aria-label="OpenAI Responses none 映射参数"]',
    ) as HTMLTextAreaElement
    expect(restoredDraft.value).toBe('{')
    expect(restoredDraft.getAttribute('aria-invalid')).toBe('true')
    expect(root.textContent).toContain('JSON 格式无效，请修正后再保存')
  })

  it('refreshes cached clean drafts when authoritative config changes', async () => {
    const { config, root } = mountPanel()
    const suffixSelect = root.querySelectorAll('select').item(1) as HTMLSelectElement

    selectSuffix(suffixSelect, 'medium')
    await nextTick()
    selectSuffix(suffixSelect, 'none')
    await nextTick()

    const current = config.value.reasoning_effort.api_formats['openai:responses']
    config.value = {
      ...config.value,
      reasoning_effort: {
        ...config.value.reasoning_effort,
        api_formats: {
          ...config.value.reasoning_effort.api_formats,
          'openai:responses': {
            ...current,
            mappings: {
              ...current.mappings,
              medium: { reasoning: { effort: 'high' } },
            },
          },
        },
      },
    }
    await nextTick()

    selectSuffix(suffixSelect, 'medium')
    await nextTick()
    const refreshedDraft = root.querySelector(
      'textarea[aria-label="OpenAI Responses medium 映射参数"]',
    ) as HTMLTextAreaElement
    expect(JSON.parse(refreshedDraft.value)).toEqual({ reasoning: { effort: 'high' } })
  })

  it('keeps a valid mapping draft retryable until the saved config becomes authoritative', async () => {
    const savedConfigs: Array<ReturnType<typeof createDefaultModelDirectivesConfig>> = []
    const { root } = mountPanel(value => savedConfigs.push(value))
    const mapping = root.querySelector(
      'textarea[aria-label="OpenAI Responses none 映射参数"]',
    ) as HTMLTextAreaElement
    mapping.value = '{"reasoning":{"effort":"medium"}}'
    mapping.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()

    const saveButton = root.querySelector(
      'button[aria-label="保存 OpenAI Responses 映射参数"]',
    ) as HTMLButtonElement
    saveButton.click()
    await nextTick()
    expect(savedConfigs).toHaveLength(1)
    expect(savedConfigs[0].reasoning_effort.api_formats['openai:responses'].mappings.none)
      .toEqual({ reasoning: { effort: 'medium' } })
    expect(saveButton.disabled).toBe(false)

    saveButton.click()
    await nextTick()
    expect(savedConfigs).toHaveLength(2)
  })
})
