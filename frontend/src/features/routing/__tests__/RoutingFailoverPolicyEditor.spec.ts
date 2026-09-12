import { afterEach, describe, expect, it } from 'vitest'
import { createApp, h, nextTick, ref, type App } from 'vue'
import RoutingFailoverPolicyEditor from '../components/RoutingFailoverPolicyEditor.vue'
import { normalizeRoutingFailoverPolicy, type RoutingFailoverPolicy } from '../utils/routingFailover'

const mounted: Array<{ app: App, root: HTMLElement }> = []

function mountEditor() {
  const policy = ref(normalizeRoutingFailoverPolicy())
  const editor = ref<{ commitJsonDrafts: () => boolean } | null>(null)
  const pending = ref(false)
  const generation = ref(0)
  const disabled = ref(false)
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp({
    setup: () => () => h(RoutingFailoverPolicyEditor, {
      ref: editor,
      key: generation.value,
      disabled: disabled.value,
      modelValue: policy.value,
      'onUpdate:modelValue': (value: RoutingFailoverPolicy) => { policy.value = value },
      onPendingChange: (value: boolean) => { pending.value = value },
    }),
  })
  app.mount(root)
  mounted.push({ app, root })
  return { root, policy, editor, pending, generation, disabled }
}

async function input(element: HTMLInputElement | HTMLTextAreaElement, value: string) {
  element.value = value
  element.dispatchEvent(new Event('input', { bubbles: true }))
  await nextTick()
}

function control<T extends HTMLElement>(root: HTMLElement, label: string): T {
  const element = root.querySelector<T>(`[aria-label="${label}"]`)
  if (!element) throw new Error(`Missing control: ${label}`)
  return element
}

afterEach(() => {
  for (const { app, root } of mounted.splice(0)) {
    app.unmount()
    root.remove()
  }
})

describe('RoutingFailoverPolicyEditor', () => {
  it.each(['{}', '{"success_failover_pattern":[]}', '{"failover_rules":{"error_stop_patterns":[]}}'])('rejects missing JSON sections instead of silently clearing rules: %s', async draft => {
    const { root, policy, editor } = mountEditor()
    policy.value.failover_rules.success_failover_patterns = [{ pattern: 'capacity', status_codes: [] }]
    await nextTick()
    control<HTMLButtonElement>(root, '切到成功转移规则 JSON').click()
    await nextTick()
    await input(control<HTMLTextAreaElement>(root, '成功转移规则 JSON'), draft)
    expect(editor.value?.commitJsonDrafts()).toBe(false)
    expect(policy.value.failover_rules.success_failover_patterns).toEqual([{ pattern: 'capacity', status_codes: [] }])
  })

  it('accepts a named JSON section nested in a complete policy', async () => {
    const { root, policy, editor } = mountEditor()
    control<HTMLButtonElement>(root, '切到成功转移规则 JSON').click()
    await nextTick()
    await input(control<HTMLTextAreaElement>(root, '成功转移规则 JSON'), '{"failover_rules":{"success_failover_patterns":[{"pattern":"capacity"}]}}')
    expect(editor.value?.commitJsonDrafts()).toBe(true)
    expect(policy.value.failover_rules.success_failover_patterns).toEqual([{ pattern: 'capacity', status_codes: [] }])
  })

  it('keeps status drafts attached to their rules after a row is deleted', async () => {
    const { root, policy, editor } = mountEditor()
    for (const index of [1, 2]) {
      control<HTMLButtonElement>(root, '添加错误终止规则').click()
      await nextTick()
      await input(control<HTMLInputElement>(root, `终止规则 ${index} 状态码`), index === 1 ? '400,' : '429, 503')
    }
    control<HTMLButtonElement>(root, '删除错误终止规则 1').click()
    await nextTick()
    expect(control<HTMLInputElement>(root, '终止规则 1 状态码').value).toBe('429, 503')
    expect(editor.value?.commitJsonDrafts()).toBe(true)
    expect(policy.value.failover_rules.error_stop_patterns).toEqual([{ pattern: '', status_codes: [429, 503] }])
  })

  it('disables JSON mode switches and formatting while a save is in flight', async () => {
    const { root, policy, editor, disabled } = mountEditor()
    control<HTMLButtonElement>(root, '切到成功转移规则 JSON').click()
    await nextTick()
    await input(control<HTMLTextAreaElement>(root, '成功转移规则 JSON'), '[{"pattern":"capacity"}]')
    disabled.value = true
    await nextTick()
    expect(control<HTMLButtonElement>(root, '切回成功转移规则表单').disabled).toBe(true)
    for (const button of root.querySelectorAll<HTMLButtonElement>('button')) expect(button.disabled).toBe(true)
    expect(editor.value?.commitJsonDrafts()).toBe(false)
    expect(policy.value.failover_rules.success_failover_patterns).toEqual([])
  })

  it('commits both JSON sections atomically when saving without returning to the form', async () => {
    const { root, policy, editor, pending } = mountEditor()
    control<HTMLButtonElement>(root, '切到成功转移规则 JSON').click()
    control<HTMLButtonElement>(root, '切到错误终止规则 JSON').click()
    await nextTick()
    const [successJson, errorJson] = root.querySelectorAll<HTMLTextAreaElement>('textarea')
    await input(successJson, '[{"pattern":"(?i)capacity"}]')
    await input(errorJson, '[{"status_codes":[400,413]}]')
    expect(editor.value?.commitJsonDrafts()).toBe(true)
    await nextTick()
    expect(policy.value.failover_rules.success_failover_patterns).toEqual([{ pattern: '(?i)capacity', status_codes: [] }])
    expect(policy.value.failover_rules.error_stop_patterns).toEqual([{ pattern: '', status_codes: [400, 413] }])
    expect(pending.value).toBe(false)
  })

  it('marks JSON-only edits pending and never partially applies invalid drafts', async () => {
    const { root, policy, editor, pending } = mountEditor()
    control<HTMLButtonElement>(root, '切到成功转移规则 JSON').click()
    control<HTMLButtonElement>(root, '切到错误终止规则 JSON').click()
    await nextTick()
    const [successJson, errorJson] = root.querySelectorAll<HTMLTextAreaElement>('textarea')
    await input(successJson, '[{"pattern":"capacity"}]')
    expect(pending.value).toBe(true)
    await input(errorJson, '{')
    expect(editor.value?.commitJsonDrafts()).toBe(false)
    expect(policy.value.failover_rules.success_failover_patterns).toEqual([])
    await input(errorJson, '[{"status_codes":[429]}]')
    expect(editor.value?.commitJsonDrafts()).toBe(true)
    await nextTick()
    expect(root.querySelector('[role="alert"]')).toBeNull()
  })

  it('preserves separators during status-code typing and rejects invalid local input at save time', async () => {
    const { root, policy, editor } = mountEditor()
    control<HTMLButtonElement>(root, '添加错误终止规则').click()
    await nextTick()
    const statuses = control<HTMLInputElement>(root, '终止规则 1 状态码')
    for (const value of ['4', '40', '400', '400,', '400, ', '400, 4', '400, 41', '400, 413']) {
      await input(statuses, value)
      expect(statuses.value).toBe(value)
    }
    expect(editor.value?.commitJsonDrafts()).toBe(true)
    await nextTick()
    expect(policy.value.failover_rules.error_stop_patterns[0].status_codes).toEqual([400, 413])
    await input(statuses, 'oops')
    expect(editor.value?.commitJsonDrafts()).toBe(false)
  })

  it('rejects non-finite limits instead of normalizing them to unlimited', async () => {
    const { policy, editor } = mountEditor()
    policy.value.max_transfer_count = Number.NaN
    await nextTick()
    expect(editor.value?.commitJsonDrafts()).toBe(false)
  })

  it('edits independent global budgets and documents sticky retry exclusion', async () => {
    const { root, policy } = mountEditor()
    expect(root.textContent).toContain('首次尝试和粘性同 Key 重试不计入')
    expect(root.textContent).toContain('不会中断已开始的调用')
    const count = control<HTMLInputElement>(root, '全局最大转移次数')
    count.value = '4'
    count.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(policy.value.max_transfer_count).toBe(4)
    expect(policy.value.max_transfer_timeout_seconds).toBe(0)
  })

  it('adds regex and status-only rules and reports invalid drafts', async () => {
    const { root, policy } = mountEditor()
    control<HTMLButtonElement>(root, '添加成功转移规则').click()
    await nextTick()
    expect(root.querySelector('[role="alert"]')?.textContent).toContain('正则表达式')
    const regex = control<HTMLInputElement>(root, '成功转移规则 1 正则')
    regex.value = '(?i)capacity.*exhausted'
    regex.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(policy.value.failover_rules.success_failover_patterns[0].pattern).toBe('(?i)capacity.*exhausted')
    control<HTMLButtonElement>(root, '添加错误终止规则').click()
    await nextTick()
    const statuses = control<HTMLInputElement>(root, '终止规则 1 状态码')
    statuses.value = '400, 413'
    statuses.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    expect(policy.value.failover_rules.error_stop_patterns[0].status_codes).toEqual([400, 413])
    expect(root.querySelector('[role="alert"]')).toBeNull()
    control<HTMLButtonElement>(root, '删除成功转移规则 1').click()
    await nextTick()
    expect(policy.value.failover_rules.success_failover_patterns).toHaveLength(0)
  })

  it('edits and applies both rule groups through JSON mode', async () => {
    const { root, policy } = mountEditor()
    control<HTMLButtonElement>(root, '切到成功转移规则 JSON').click()
    await nextTick()
    const successJson = root.querySelector<HTMLTextAreaElement>('textarea')
    if (!successJson) throw new Error('Missing success JSON editor')
    successJson.value = '[{"pattern":"capacity"}]'
    successJson.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    control<HTMLButtonElement>(root, '切回成功转移规则表单').click()
    await nextTick()
    expect(policy.value.failover_rules.success_failover_patterns).toEqual([{ pattern: 'capacity', status_codes: [] }])

    control<HTMLButtonElement>(root, '切到错误终止规则 JSON').click()
    await nextTick()
    const errorJson = root.querySelector<HTMLTextAreaElement>('textarea')
    if (!errorJson) throw new Error('Missing error JSON editor')
    errorJson.value = '[{"status_codes":[429,500],"pattern":"rate"}]'
    errorJson.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    control<HTMLButtonElement>(root, '切回错误终止规则表单').click()
    await nextTick()
    expect(policy.value.failover_rules.error_stop_patterns).toEqual([{ pattern: 'rate', status_codes: [429, 500] }])
  })

  it('keeps invalid JSON visible until it is corrected', async () => {
    const { root, policy } = mountEditor()
    control<HTMLButtonElement>(root, '切到错误终止规则 JSON').click()
    await nextTick()
    const editor = root.querySelector<HTMLTextAreaElement>('textarea')
    if (!editor) throw new Error('Missing JSON editor')
    editor.value = '{'
    editor.dispatchEvent(new Event('input', { bubbles: true }))
    await nextTick()
    control<HTMLButtonElement>(root, '切回错误终止规则表单').click()
    await nextTick()
    expect(root.querySelector('[role="alert"]')?.textContent).toContain('JSON')
    expect(policy.value.failover_rules.error_stop_patterns).toHaveLength(0)
  })
})
