import { afterEach, describe, expect, it } from 'vitest'
import { setI18nLocale } from '@/i18n'
import { useToast } from '../useToast'
import { useConfirm } from '../useConfirm'

afterEach(() => {
  useToast().clearAll()
  useConfirm().handleCancel()
})

describe('localized feedback', () => {
  it('retranslates an existing toast in both directions without changing its identity', () => {
    const { showToast, toasts, removeToast } = useToast()
    setI18nLocale('en-US')
    const id = showToast({ title: '保存', description: '保存成功', duration: 0 })
    expect(toasts.value[0]).toMatchObject({ id, title: 'Save' })
    setI18nLocale('zh-CN')
    expect(toasts.value[0]).toMatchObject({ id, title: '保存', message: '保存成功' })
    setI18nLocale('en-US')
    expect(toasts.value[0].title).toBe('Save')
    removeToast(id)
    expect(toasts.value).toEqual([])
  })

  it('keeps the pending confirmation while its labels change language', async () => {
    const { confirm, state, handleConfirm } = useConfirm()
    setI18nLocale('en-US')
    const result = confirm({ message: '保存', confirmText: '保存' })
    expect(state.value.confirmText).toBe('Save')
    setI18nLocale('zh-CN')
    expect(state.value).toMatchObject({ isOpen: true, message: '保存', confirmText: '保存' })
    setI18nLocale('en-US')
    expect(state.value.message).toBe('Save')
    handleConfirm()
    expect(await result).toBe(true)
    expect(state.value.isOpen).toBe(false)
  })
})
