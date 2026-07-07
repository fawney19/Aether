import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

const {
  getAllUsageRecordsMock,
  getAllUsageRecordTotalMock,
  getUsageStatsMock,
  getUsageByModelMock,
  getUsageByProviderMock,
  getUsageByApiFormatMock,
  meGetUsageMock,
} = vi.hoisted(() => ({
  getAllUsageRecordsMock: vi.fn(),
  getAllUsageRecordTotalMock: vi.fn(),
  getUsageStatsMock: vi.fn(),
  getUsageByModelMock: vi.fn(),
  getUsageByProviderMock: vi.fn(),
  getUsageByApiFormatMock: vi.fn(),
  meGetUsageMock: vi.fn(),
}))

vi.mock('@/api/usage', () => ({
  usageApi: {
    getAllUsageRecords: getAllUsageRecordsMock,
    getAllUsageRecordTotal: getAllUsageRecordTotalMock,
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

function createDeferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

async function flushMicrotasks() {
  await Promise.resolve()
  await Promise.resolve()
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
    getAllUsageRecordTotalMock.mockResolvedValue(1)
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

  it('keeps locally resolved failure fields when a stale active record refreshes', async () => {
    const isAdminPage = ref(true)
    const { loadRecords, currentRecords } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'today', tz_offset_minutes: 0 }

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord({
        status: 'failed',
        status_code: 524,
        error_message: 'error code: 524',
        response_time_ms: 125_000,
      })],
      total: 1,
      limit: 20,
      offset: 0,
    })

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord({
        status: 'pending',
        status_code: undefined,
        error_message: undefined,
        response_time_ms: null,
      })],
      total: 1,
      limit: 20,
      offset: 0,
    })

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    expect(currentRecords.value[0]).toMatchObject({
      status: 'failed',
      status_code: 524,
      error_message: 'error code: 524',
      response_time_ms: 125_000,
    })
  })

  it('preserves detail-filled usage metrics when a later list refresh is still empty', async () => {
    const isAdminPage = ref(true)
    const { loadRecords, currentRecords } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'today', tz_offset_minutes: 0 }

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord({
        status: 'completed',
        input_tokens: 0,
        effective_input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_creation_ephemeral_5m_input_tokens: 0,
        cache_creation_ephemeral_1h_input_tokens: 0,
        cache_read_input_tokens: 0,
        cost: 0,
        actual_cost: 0,
        response_time_ms: null,
        first_byte_time_ms: null,
        is_stream: false,
        upstream_is_stream: false,
        client_requested_stream: false,
        client_is_stream: false,
      })],
      total: 1,
      limit: 20,
      offset: 0,
    })

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    Object.assign(currentRecords.value[0], {
      input_tokens: 1138,
      effective_input_tokens: 1138,
      output_tokens: 244,
      total_tokens: 81126,
      cache_creation_input_tokens: 17,
      cache_creation_ephemeral_5m_input_tokens: 5,
      cache_creation_ephemeral_1h_input_tokens: 12,
      cache_read_input_tokens: 79744,
      cost: 0.052882,
      actual_cost: 0.052882,
      response_time_ms: 5570,
      first_byte_time_ms: 1600,
      is_stream: true,
      upstream_is_stream: true,
      client_requested_stream: true,
      client_is_stream: true,
      api_format: 'openai:responses',
      endpoint_api_format: 'openai:responses',
      has_format_conversion: false,
      has_retry: true,
      target_model: 'gpt-5.5',
      reasoning_effort: 'xhigh',
      service_tier: 'auto',
    })

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord({
        status: 'completed',
        input_tokens: 0,
        effective_input_tokens: 0,
        output_tokens: 0,
        total_tokens: 0,
        cache_creation_input_tokens: 0,
        cache_creation_ephemeral_5m_input_tokens: 0,
        cache_creation_ephemeral_1h_input_tokens: 0,
        cache_read_input_tokens: 0,
        cost: 0,
        actual_cost: 0,
        response_time_ms: null,
        first_byte_time_ms: null,
        is_stream: false,
        upstream_is_stream: false,
        client_requested_stream: false,
        client_is_stream: false,
        api_format: undefined,
        endpoint_api_format: undefined,
        has_format_conversion: undefined,
        has_retry: false,
        target_model: null,
        reasoning_effort: null,
        service_tier: null,
      })],
      total: 1,
      limit: 20,
      offset: 0,
    })

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    expect(currentRecords.value[0]).toMatchObject({
      status: 'completed',
      input_tokens: 1138,
      effective_input_tokens: 1138,
      output_tokens: 244,
      total_tokens: 81126,
      cache_creation_input_tokens: 17,
      cache_creation_ephemeral_5m_input_tokens: 5,
      cache_creation_ephemeral_1h_input_tokens: 12,
      cache_read_input_tokens: 79744,
      cost: 0.052882,
      actual_cost: 0.052882,
      response_time_ms: 5570,
      first_byte_time_ms: 1600,
      is_stream: true,
      upstream_is_stream: true,
      client_requested_stream: true,
      client_is_stream: true,
      api_format: 'openai:responses',
      endpoint_api_format: 'openai:responses',
      has_format_conversion: false,
      has_retry: true,
      target_model: 'gpt-5.5',
      reasoning_effort: 'xhigh',
      service_tier: 'auto',
    })
  })

  it('allows finalized list metrics to replace larger detail estimates', async () => {
    const isAdminPage = ref(true)
    const { loadRecords, currentRecords } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'today', tz_offset_minutes: 0 }

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord({
        status: 'completed',
        input_tokens: 1200,
        output_tokens: 300,
        total_tokens: 1500,
        cost: 0.09,
        actual_cost: 0.09,
      })],
      total: 1,
      limit: 20,
      offset: 0,
    })

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord({
        status: 'completed',
        input_tokens: 1100,
        output_tokens: 250,
        total_tokens: 1350,
        cost: 0.07,
        actual_cost: 0.07,
      })],
      total: 1,
      limit: 20,
      offset: 0,
    })

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    expect(currentRecords.value[0]).toMatchObject({
      input_tokens: 1100,
      output_tokens: 250,
      total_tokens: 1350,
      cost: 0.07,
      actual_cost: 0.07,
    })
  })

  it('refreshes exact admin record totals after an estimated first page', async () => {
    const isAdminPage = ref(true)
    const { loadRecords, totalRecords } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'last7days', tz_offset_minutes: 0 }

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord()],
      total: 21,
      total_is_estimated: true,
      limit: 20,
      offset: 0,
    })
    getAllUsageRecordTotalMock.mockResolvedValueOnce(122101)

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)

    expect(getAllUsageRecordsMock).toHaveBeenCalledWith(expect.objectContaining({
      include_total: false,
    }))
    await Promise.resolve()
    await Promise.resolve()

    expect(getAllUsageRecordTotalMock).toHaveBeenCalledWith(expect.objectContaining({
      preset: 'last7days',
      tz_offset_minutes: 0,
    }))
    expect(totalRecords.value).toBe(122101)
  })

  it('keeps the exact admin record total while a later page returns an estimate', async () => {
    const isAdminPage = ref(true)
    const { loadRecords, totalRecords } = useUsageData({ isAdminPage })
    const dateRange = { preset: 'last7days', tz_offset_minutes: 0 }

    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord()],
      total: 21,
      total_is_estimated: true,
      limit: 20,
      offset: 0,
    })
    getAllUsageRecordTotalMock.mockResolvedValueOnce(8650)

    await loadRecords({ page: 1, pageSize: 20 }, undefined, dateRange)
    await flushMicrotasks()

    expect(totalRecords.value).toBe(8650)

    const exactTotal = createDeferred<number>()
    getAllUsageRecordsMock.mockResolvedValueOnce({
      records: [buildUsageRecord({ id: 'usage-2' })],
      total: 41,
      total_is_estimated: true,
      limit: 20,
      offset: 20,
    })
    getAllUsageRecordTotalMock.mockReturnValueOnce(exactTotal.promise)

    await loadRecords({ page: 2, pageSize: 20 }, undefined, dateRange)

    expect(totalRecords.value).toBe(8650)

    exactTotal.resolve(8651)
    await flushMicrotasks()

    expect(totalRecords.value).toBe(8651)
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
