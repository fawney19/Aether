import type { CodexAdapterSchedulingMode } from '@/api/modules'

export const codexAdapterSchedulingModeOptions: ReadonlyArray<{
  readonly value: CodexAdapterSchedulingMode
  readonly label: string
}> = [
  { value: 'priority', label: '优先级' },
  { value: 'sticky', label: '粘性' },
  { value: 'load_balance', label: '负载均衡' },
]

export const codexAdapterSchedulingModeHelp: Readonly<Record<CodexAdapterSchedulingMode, string>> = {
  priority: '按优先级和列表顺序依次尝试，当前模型全部失败后再进入下一个候选模型。',
  sticky: '同一会话优先命中首选模型，失败后仍按优先级和列表顺序继续兜底。',
  load_balance: '首选模型按权重分摊，失败后仍按优先级和列表顺序继续兜底。',
}
