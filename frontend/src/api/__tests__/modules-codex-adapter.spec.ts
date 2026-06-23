import { describe, expect, it } from 'vitest'
import {
  normalizeCodexAdapterConfig,
  serializeCodexAdapterConfig,
  validateCodexAdapterConfig,
} from '@/api/modules'
import {
  reorderCodexAdapterCandidates,
  validateCodexAdapterCompatibleRoutes,
} from '@/views/admin/modules/codex-adapter-support'

function createCandidate(globalModel: string, priority: number) {
  return {
    global_model: globalModel,
    enabled: true,
    priority,
    weight: 100,
  }
}

describe('codex adapter module config helpers', () => {
  it('normalizes routes and trims persisted model names', () => {
    const normalized = normalizeCodexAdapterConfig([
      {
        codex_model: ' gpt-5.5 ',
        enabled: true,
        scheduling_mode: 'sticky',
        candidates: [
          { global_model: ' glm-4.6 ', enabled: true, priority: 0, weight: 70 },
          { global_model: 'deepseek-v3.2', enabled: true, priority: 1, weight: 30 },
        ],
      },
      {
        codex_model: 'gpt-5.5',
        enabled: true,
        scheduling_mode: 'priority',
        candidates: [],
      },
    ])

    expect(normalized).toEqual([
      {
        codex_model: 'gpt-5.5',
        enabled: true,
        scheduling_mode: 'sticky',
        candidates: [
          { global_model: 'glm-4.6', enabled: true, priority: 0, weight: 70 },
          { global_model: 'deepseek-v3.2', enabled: true, priority: 1, weight: 30 },
        ],
      },
    ])
  })

  it('rejects enabled routes without enabled candidates before save', () => {
    const message = validateCodexAdapterConfig([
      {
        codex_model: 'gpt-5.5',
        enabled: true,
        scheduling_mode: 'priority',
        candidates: [
          { global_model: 'glm-4.6', enabled: false, priority: 0, weight: 100 },
        ],
      },
    ])

    expect(message).toContain('至少需要一个启用中的候选模型')
  })

  it('serializes draft values with trimmed strings and integer fallback', () => {
    const serialized = serializeCodexAdapterConfig([
      {
        codex_model: ' gpt-5.5 ',
        enabled: true,
        scheduling_mode: 'load_balance',
        candidates: [
          { global_model: ' glm-4.6 ', enabled: true, priority: 2.4, weight: 9.8 },
        ],
      },
    ])

    expect(serialized).toEqual([
      {
        codex_model: 'gpt-5.5',
        enabled: true,
        scheduling_mode: 'load_balance',
        candidates: [
          { global_model: 'glm-4.6', enabled: true, priority: 2, weight: 9 },
        ],
      },
    ])
  })

  it('rejects enabled candidates that cannot serve responses requests', () => {
    const message = validateCodexAdapterCompatibleRoutes(
      [
        {
          codex_model: 'gpt-5.5',
          enabled: true,
          scheduling_mode: 'priority',
          candidates: [
            { global_model: 'glm-4.6', enabled: true, priority: 0, weight: 100 },
          ],
        },
      ],
      {
        'glm-4.6': {
          global_model: 'glm-4.6',
          compatible: false,
          reasons: ['format_conversion_disabled'],
          summary: '未开启格式转换',
        },
      },
    )

    expect(message).toContain('glm-4.6')
    expect(message).toContain('未开启格式转换')
  })

  it('recomputes compact priorities when a candidate moves across priority groups', () => {
    const reordered = reorderCodexAdapterCandidates(
      [
        createCandidate('model-a', 0),
        createCandidate('model-b', 1),
        createCandidate('model-c', 1),
        createCandidate('model-d', 2),
      ],
      3,
      1,
    )

    expect(reordered).toEqual([
      createCandidate('model-a', 0),
      createCandidate('model-d', 1),
      createCandidate('model-b', 2),
      createCandidate('model-c', 2),
    ])
  })

  it('keeps priorities unchanged when reordering inside the same priority group', () => {
    const reordered = reorderCodexAdapterCandidates(
      [
        createCandidate('model-a', 0),
        createCandidate('model-b', 1),
        createCandidate('model-c', 1),
      ],
      2,
      1,
    )

    expect(reordered).toEqual([
      createCandidate('model-a', 0),
      createCandidate('model-c', 1),
      createCandidate('model-b', 1),
    ])
  })
})
