import { describe, expect, it } from 'vitest'

import { legacyUiEnglishMessages } from '@/i18n/legacy-ui-messages'
import { legacyAdminEnglishMessages } from '@/i18n/legacy-admin-messages'
import { legacyGuideEnglishMessages } from '@/i18n/legacy-guide-messages'
import { messages, translateLegacyText } from '@/i18n/messages'

const placeholders = (message: string) => [...message.matchAll(/\{(\w+)\}/g)]
  .map(match => match[1])
  .sort()

describe('translation coverage', () => {
  it('keeps locale keys and interpolation parameters aligned', () => {
    expect(Object.keys(messages['en-US']).sort()).toEqual(Object.keys(messages['zh-CN']).sort())

    for (const key of Object.keys(messages['zh-CN']) as Array<keyof typeof messages['zh-CN']>) {
      expect(placeholders(messages['en-US'][key]), key).toEqual(placeholders(messages['zh-CN'][key]))
    }
  })

  it('provides complete English sentences for the extended UI catalog', () => {
    const catalog = { ...legacyUiEnglishMessages, ...legacyAdminEnglishMessages, ...legacyGuideEnglishMessages }
    for (const [source, translation] of Object.entries(catalog)) {
      expect(translation, source).not.toMatch(/[\u4e00-\u9fff]/u)
      expect(translateLegacyText(source, 'en-US'), source).not.toMatch(/[\u4e00-\u9fff]/u)
      expect(translateLegacyText(source, 'zh-CN'), source).toBe(source)
    }
  })

  it.each([
    ['暂无登录设备记录', 'No sign-in devices recorded'],
    ['请妥善保管，此令牌只会显示一次', 'Store this token securely. It is shown only once.'],
    ['给密钥起一个有意义的名称方便识别', 'Use a descriptive name to identify this key'],
    ['套餐不足时继续扣钱包余额', 'Charge the wallet when plan credit runs out'],
    ['冲突处理模式', 'Conflict handling'],
    ['如果发现任何冲突，导入将在写入前预检并中止', 'Conflicts are checked before any data is written. The import stops if a conflict is found.'],
    ['有未保存的更改，确定要关闭吗？', 'You have unsaved changes. Close without saving?'],
    ['该独立 Key 的钱包尚未初始化，暂时无法进行资金操作', 'This key wallet has not been initialized. Balance operations are unavailable.'],
    ['1月', 'Jan'],
    ['12月', 'Dec'],
    ['用户已启用', 'User enabled'],
    ['用户已禁用', 'User disabled'],
    ['用户已删除', 'User deleted'],
    ['成功率 99.2%', 'Success rate 99.2%'],
    ['输入 1.8M / 输出 0.7M', 'Input 1.8M / Output 0.7M'],
    ['节省 $12.34 (21%)', 'Saved $12.34 (21%)'],
    ['总用户 156', 'Total users 156'],
    ['IP restrictions: 不限制', 'IP restrictions: No restriction'],
    ['2 维度', '2 dimensions'],
    ['1 维度', '1 dimension'],
    ['14天0时', '14d 0h'],
    ['5天 0:00:00', '5d 0:00:00'],
  ])('translates %s as a complete message', (source, expected) => {
    expect(translateLegacyText(source, 'en-US')).toBe(expected)
  })

  it('reuses keyed translations for static legacy UI', () => {
    expect(translateLegacyText('打开导航菜单', 'en-US')).toBe(messages['en-US']['common.openMenu'])
    expect(translateLegacyText('再次输入密码', 'en-US')).toBe(messages['en-US']['auth.register.confirmPasswordPlaceholder'])
  })

  it.each([
    '研发用户的专属密钥',
    '用户已保存的自定义内容',
    '华东数据分析项目',
    '上海服务提供商',
    '客户张月',
    '用户14天0时',
    '5天 50:90:00',
    '30月',
    '  Keep this 用户输入 exactly as written.\n',
    '{"name":"生产环境密钥","enabled":true}',
  ])('preserves unknown content: %s', (source) => {
    expect(translateLegacyText(source, 'en-US')).toBe(source)
  })

  it('preserves dynamic names, formatting, and outer whitespace', () => {
    expect(translateLegacyText('\n  该独立 Key 的钱包尚未初始化，暂时无法进行资金操作  \n', 'en-US'))
      .toBe('\n  This key wallet has not been initialized. Balance operations are unavailable.  \n')
    expect(translateLegacyText('选择目标客户端和模型 ID。点击导入后浏览器会请求打开 CC Switch，本页面不会展示或保存包含 API\n  Key 的链接。', 'en-US'))
      .toBe('Select a client and model ID. Import opens CC Switch through your browser. Links containing your API key are not displayed or stored on this page.')
    expect(translateLegacyText('  已删除映射 华东用户模型  ', 'en-US'))
      .toBe('  Deleted mapping 华东用户模型  ')
    expect(translateLegacyText('确认删除（生产账号）', 'en-US'))
      .toBe('Confirm delete (生产账号)')
    expect(translateLegacyText('12 分钟', 'en-US')).toBe('12 min')
    expect(translateLegacyText('发布于 2026-09-07', 'en-US')).toBe('Published at 2026-09-07')
  })
})
