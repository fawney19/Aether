import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

const {
  getAllUsageRecordsMock,
  getUsageStatsMock,
  getUsageByModelMock,
  getUsageByProviderMock,
  getUsageByApiFormatMock,
  meGetUsageMock,
} = vi.hoisted(() => ({
  getAllUsageRecordsMock: vi.fn(),
  getUsageStatsMock: vi.fn(),
  getUsageByModelMock: vi.fn(),
  getUsageByProviderMock: vi.fn(),
  getUsageByApiFormatMock: vi.fn(),
  meGetUsageMock: vi.fn(),
}))

vi.mock('@/api/usage', () => ({
  usageApi: {
    getAllUsageRecords: getAllUsageRecordsMock,
    getUsageStats: getUsageStatsMock,
    getUsageByModel: getUsageByModelMock,
    getUsageByProvider: getUsageByProviderMock,
    getUsageByApiFormat: getUsageByApiFormatMock,
  },
}))

vi.mock('@/api/me', () => ({
  meApi: {
    getUsage: meGetUsageMock,
  },
}))

vi.mock('@/utils/logger', () => ({
  log: {
    debug: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    http: vi.fn(),
    performance: vi.fn(),
  },
}))

import { useUsageData } from '../useUsageData'
import type { UsageRecord } from '../../types'

function buildUsageRecord(overrides: Partial<UsageRecord> = {}): UsageRecord {
  return {
    id: 'usage-1',
    model: 'gpt-5',
    input_tokens: 10,
    output_tokens: 5,
    total_tokens: 15,
    cost: 0.01,
    is_stream: false,
    created_at: '2026-05-01T00:00:00Z',
    status: 'completed',
    ...overrides,
  }
}

describe('useUsageData', () => {
  beforeEach(() => {
    vi.clearAllMocks()

    getAllUsageRecordsMock.mockResolvedValue({
      records: [buildUsageRecord()],
      total: 1,
      limit: 20,
      offset: 0,
    })
    getUsageStatsMock.mockRejectedValue({
      response: { status: 500 },
      message: 'stats failed',
    })
    getUsageByModelMock.mockResolvedValue([])
    getUsageByProviderMock.mockResolvedValue([])
    getUsageByApiFormatMock.mockResolvedValue([])
    meGetUsageMock.mockResolvedValue({})
  })

  it('keeps admin records when stats refresh fails', async () => {
    const isAdminPage = ref(true)
    const { loadRecords, loadStats, currentRecords, totalRecords } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'last7days', tz_offset_minutes: 0 }

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    expect(currentRecords.value).toHaveLength(1)
    expect(totalRecords.value).toBe(1)

    await loadStats(dateRange)

    expect(currentRecords.value).toHaveLength(1)
    expect(currentRecords.value[0]).toMatchObject({
      id: 'usage-1',
      model: 'gpt-5',
    })
    expect(totalRecords.value).toBe(1)
  })

  it('continues loading admin breakdowns when the summary request fails', async () => {
    const isAdminPage = ref(true)
    const {
      loadStats,
      stats,
      modelStats,
      providerStats,
      apiFormatStats,
      availableModels,
      availableProviders,
    } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'last7days', tz_offset_minutes: 0 }

    getUsageStatsMock.mockRejectedValueOnce({
      response: { status: 500 },
      message: 'summary failed',
    })
    getUsageByModelMock.mockResolvedValueOnce([
      {
        model: 'gpt-5',
        request_count: 3,
        total_tokens: 300,
        total_cost: 1.23,
      },
    ])
    getUsageByProviderMock.mockResolvedValueOnce([
      {
        provider_id: 'provider-openai',
        provider_key: 'provider-openai',
        provider_identity_source: 'provider_id',
        provider: 'OpenAI',
        request_count: 3,
        total_tokens: 300,
        total_cost: 1.23,
        actual_cost: 1.5,
        avg_response_time_ms: 1250,
        success_rate: 1,
      },
    ])
    getUsageByApiFormatMock.mockResolvedValueOnce([
      {
        api_format: 'openai:chat',
        request_count: 3,
        total_tokens: 300,
        total_cost: 1.23,
        actual_cost: 1.5,
        avg_response_time_ms: 1250,
      },
    ])

    const hadFailure = await loadStats(dateRange)

    expect(hadFailure).toBe(true)
    expect(stats.value).toMatchObject({
      total_requests: 0,
      total_tokens: 0,
      total_cost: 0,
    })
    expect(modelStats.value).toHaveLength(1)
    expect(providerStats.value).toHaveLength(1)
    expect(providerStats.value[0]).toMatchObject({
      providerId: 'provider-openai',
      providerKey: 'provider-openai',
      providerIdentitySource: 'provider_id',
    })
    expect(apiFormatStats.value).toHaveLength(1)
    expect(availableModels.value).toEqual(['gpt-5'])
    expect(availableProviders.value).toEqual(['OpenAI'])
  })

  it('filters placeholder providers from admin provider stats', async () => {
    const isAdminPage = ref(true)
    const { loadStats, providerStats, availableProviders } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'last7days', tz_offset_minutes: 0 }

    getUsageStatsMock.mockResolvedValueOnce({
      total_requests: 4,
      total_tokens: 400,
      total_cost: 1,
      avg_response_time: 0,
    })
    getUsageByProviderMock.mockResolvedValueOnce([
      {
        provider: 'OpenAI',
        request_count: 3,
        total_tokens: 300,
        total_cost: 1.23,
        actual_cost: 1.5,
        avg_response_time_ms: 1250,
        success_rate: 100,
      },
      {
        provider: 'Unknown',
        request_count: 1,
        total_tokens: 100,
        total_cost: 0,
        actual_cost: 0,
        avg_response_time_ms: 0,
        success_rate: 100,
      },
      {
        provider: 'unknow',
        request_count: 1,
        total_tokens: 100,
        total_cost: 0,
        actual_cost: 0,
        avg_response_time_ms: 0,
        success_rate: 100,
      },
      {
        provider: 'pending',
        request_count: 1,
        total_tokens: 100,
        total_cost: 0,
        actual_cost: 0,
        avg_response_time_ms: 0,
        success_rate: 100,
      },
    ])

    await loadStats(dateRange)

    expect(providerStats.value.map(item => item.provider)).toEqual(['OpenAI'])
    expect(availableProviders.value).toEqual(['OpenAI'])
  })

  it('keeps previous admin provider stats when a background refresh fails', async () => {
    const isAdminPage = ref(true)
    const { loadStats, providerStats, availableProviders } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'last7days', tz_offset_minutes: 0 }

    getUsageStatsMock.mockResolvedValueOnce({
      total_requests: 3,
      total_tokens: 300,
      total_cost: 1,
      avg_response_time: 0,
    })
    getUsageByProviderMock.mockResolvedValueOnce([
      {
        provider: 'OpenAI',
        request_count: 3,
        total_tokens: 300,
        total_cost: 1.23,
        actual_cost: 1.5,
        avg_response_time_ms: 1250,
        success_rate: 100,
      },
    ])

    await loadStats(dateRange)

    getUsageStatsMock.mockResolvedValueOnce({
      total_requests: 4,
      total_tokens: 400,
      total_cost: 2,
      avg_response_time: 0,
    })
    getUsageByProviderMock.mockRejectedValueOnce({
      response: { status: 500 },
      message: 'provider aggregation failed',
    })

    const hadFailure = await loadStats(dateRange, { preserveOnFailure: true })

    expect(hadFailure).toBe(true)
    expect(providerStats.value).toHaveLength(1)
    expect(providerStats.value[0]).toMatchObject({
      provider: 'OpenAI',
      requests: 3,
    })
    expect(availableProviders.value).toEqual(['OpenAI'])
  })
})
