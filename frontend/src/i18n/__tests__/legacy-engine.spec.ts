import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as Vue from 'vue'
import { compile, nextTick, ref } from 'vue'
import { compileScript, compileTemplate, parse } from 'vue/compiler-sfc'

import { installLegacyDomTranslator } from '@/i18n/dom-translator'
import { transformLegacyTemplateI18n, transformVueSource } from '@/i18n/legacy-template-transform'
import { translateLegacyText, type Locale } from '@/i18n/messages'

describe('legacy template compiler', () => {
  it('translates content following nested slot templates and accepts reordered setup attributes', () => {
    const source = `<template><Panel><template #default>保存</template></Panel><footer>取消</footer></template>
      <script lang="ts" setup>const name = 'value'</script>`
    const result = transformVueSource(source)
    const { descriptor, errors } = parse(result.code)

    expect(errors).toEqual([])
    expect(descriptor.template?.content).toContain('{{ __aetherLegacyT("取消") }}')
    expect(result.code.match(/<script/g)).toHaveLength(1)
    expect(() => compileScript(descriptor, { id: 'nested-template' })).not.toThrow()
    expect(transformVueSource(result.code).changed).toBe(false)
  })

  it('preserves comparisons and user values while translating displayed branches', () => {
    const source = `<span>{{ count < limit ? '保存' : user.name }}</span>
      <span>{{ status === '保存' }}</span><span>{{ names['保存'] }}</span>
      <span>{{ format('保存') }}</span><span>{{ legacyT('保存') }}</span>
      <span :title="count > limit ? '关闭' : user.name">{{ user.name }}</span>`
    const result = transformLegacyTemplateI18n(source)

    expect(result.code).toContain(`count < limit ? __aetherLegacyT("保存") : user.name`)
    expect(result.code).toContain(`{{ status === '保存' }}`)
    expect(result.code).toContain(`{{ names['保存'] }}`)
    expect(result.code).toContain(`{{ format('保存') }}`)
    expect(result.code).toContain(`{{ legacyT('保存') }}`)
    expect(result.code).toContain(`{{ user.name }}`)
    expect(compileTemplate({ source: result.code, id: 'comparisons', filename: 'comparisons.vue' }).errors).toEqual([])
  })

  it('matches an existing plain script language when adding the setup helper', () => {
    const result = transformVueSource('<template><span>保存</span></template><script>export default { name: "Legacy" }</script>')
    const { descriptor } = parse(result.code)
    expect(descriptor.scriptSetup?.lang).toBe(descriptor.script?.lang)
    expect(() => compileScript(descriptor, { id: 'plain-script' })).not.toThrow()
  })

  it('keeps quotes and HTML entities intact in static and bound attributes', () => {
    const result = transformLegacyTemplateI18n(`<input title="保存 &quot;O&apos;Reilly&quot; &amp; &lt;x&gt;" placeholder=保存 :aria-label="open ? '关闭 &amp; 保存' : '保存'">`)
    const render = compile(result.code) as (context: Record<string, unknown>, cache: unknown[]) => Vue.VNode
    const vnode = render({ open: true, __aetherLegacyT: (value: string) => value }, [])

    expect(vnode.props?.title).toBe(`保存 "O'Reilly" & <x>`)
    expect(vnode.props?.placeholder).toBe('保存')
    expect(vnode.props?.['aria-label']).toBe('关闭 & 保存')
  })

  it('translates template literal text without translating interpolated values', () => {
    const result = transformLegacyTemplateI18n('<span>{{ `保存 ${user.name}` }}</span>')
    expect(result.code).toContain('`${__aetherLegacyT("保存 ")}${user.name}`')
    const render = compile(result.code) as (context: Record<string, unknown>, cache: unknown[]) => Vue.VNode
    const vnode = render({ user: { name: '取消' }, __aetherLegacyT: (value: string) => value === '保存 ' ? 'Save ' : 'unexpected' }, [])
    expect(vnode.children).toBe('Save 取消')
  })

  it.each(['HelpHint', 'help-hint'])('translates static and displayed literal text props on %s', tag => {
    const context = {
      expanded: true,
      record: { text: '保存' },
      __aetherLegacyT: (value: string) => translateLegacyText(value, 'en-US'),
    }
    const staticResult = transformLegacyTemplateI18n(`<${tag} text="保存" />`)
    const renderStatic = compile(staticResult.code, { isCustomElement: name => name === tag }) as (context: Record<string, unknown>, cache: unknown[]) => Vue.VNode
    expect(renderStatic(context, []).props?.text).toBe('Save')

    const dynamicResult = transformLegacyTemplateI18n(`<${tag} :text="expanded ? '关闭' : record.text" />`)
    const renderDynamic = compile(dynamicResult.code, { isCustomElement: name => name === tag }) as (context: Record<string, unknown>, cache: unknown[]) => Vue.VNode
    expect(renderDynamic(context, []).props?.text).toBe('Close')
    expect(renderDynamic({ ...context, expanded: false }, []).props?.text).toBe('保存')
  })

  it('keeps ordinary text props and opted-out HelpHint text as application data', () => {
    const source = `<Message text="保存" /><Message :text="active ? '关闭' : record.text" />
      <div text="保存" /><HelpHint translate="no" text="保存" />
      <HelpHint data-i18n-skip :text="active ? '关闭' : record.text" />`
    expect(transformLegacyTemplateI18n(source).code).toBe(source)
  })

  it('respects skipped subtrees including v-pre and contenteditable', () => {
    const source = `<section translate="no"><span title="关闭">保存</span></section>
      <section data-i18n-skip><span>保存</span></section>
      <section v-pre title="保存 > 标题"><span>{{ '保存' }}</span></section>
      <section contenteditable="true">保存</section><pre>保存</pre><code>保存</code>`
    const result = transformLegacyTemplateI18n(source)
    expect(result.code).toBe(source.replace('<section v-pre', '<section data-i18n-skip v-pre'))
    expect(result.needsHelper).toBe(false)
    expect(transformLegacyTemplateI18n(result.code).changed).toBe(false)
    expect(transformLegacyTemplateI18n('<span title="v-pre">保存</span>').changed).toBe(true)
  })
})

describe('legacy DOM translation updates', () => {
  let stop: () => void
  let locale: Vue.Ref<Locale>
  let root: HTMLDivElement

  async function flushTranslation(): Promise<void> {
    await nextTick()
    await Promise.resolve()
    await vi.advanceTimersByTimeAsync(40)
  }

  beforeEach(() => {
    vi.useFakeTimers()
    root = document.createElement('div')
    document.body.append(root)
    locale = ref<Locale>('en-US')
    stop = installLegacyDomTranslator(locale)
  })

  afterEach(() => {
    stop()
    root.remove()
    vi.useRealTimers()
  })

  it('translates new state on a reused text node and restores the latest source', async () => {
    const text = document.createTextNode('保存')
    root.append(text)
    await flushTranslation()
    expect(text.nodeValue).toBe('Save')

    text.nodeValue = '保存中...'
    await flushTranslation()
    expect(text.nodeValue).toBe('Saving...')

    locale.value = 'zh-CN'
    await flushTranslation()
    expect(text.nodeValue).toBe('保存中...')

    text.nodeValue = '取消'
    locale.value = 'en-US'
    await flushTranslation()
    expect(text.nodeValue).toBe('Cancel')
    locale.value = 'zh-CN'
    await flushTranslation()
    expect(text.nodeValue).toBe('取消')
  })

  it('updates attributes across changes, removal, and language round trips', async () => {
    root.title = '关闭'
    await flushTranslation()
    expect(root.title).toBe('Close')
    root.title = '保存'
    await flushTranslation()
    expect(root.title).toBe('Save')
    locale.value = 'zh-CN'
    await flushTranslation()
    expect(root.title).toBe('保存')

    root.removeAttribute('title')
    await flushTranslation()
    root.title = '取消'
    locale.value = 'en-US'
    await flushTranslation()
    expect(root.title).toBe('Cancel')
    root.title = 'User supplied English'
    locale.value = 'zh-CN'
    await flushTranslation()
    expect(root.title).toBe('User supplied English')
  })

  it('preserves code, editable values, and explicit untranslated subtrees', async () => {
    root.innerHTML = `<code title="关闭">保存</code><pre>保存</pre><textarea>保存</textarea>
      <span contenteditable="true">保存</span><span v-pre>保存</span>
      <section translate="no"><span title="关闭">保存</span></section>
      <section data-i18n-skip><span title="关闭">保存</span></section>`
    const original = root.innerHTML
    await flushTranslation()
    expect(root.innerHTML).toBe(original)
    locale.value = 'zh-CN'
    await flushTranslation()
    expect(root.innerHTML).toBe(original)
  })

  it('restores original text when a translated region becomes opted out', async () => {
    root.textContent = '保存'
    root.title = '关闭'
    await flushTranslation()
    expect(root.textContent).toBe('Save')
    root.setAttribute('translate', 'no')
    await flushTranslation()
    expect(root.textContent).toBe('保存')
    expect(root.title).toBe('关闭')
  })
})
