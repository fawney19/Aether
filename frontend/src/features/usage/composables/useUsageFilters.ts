import { ref, computed, type Ref } from 'vue'
import type { UsageRecord, FilterStatusValue } from '../types'
import {
  hasUsageFallback,
  hasUsageRetry,
  isUsageRecordFailed,
  isUsageUpstreamStream,
  resolveDisplayRequestStatus,
} from '../utils/status'

export interface UseUsageFiltersOptions {
  /** 所有记录的响应式引用 */
  allRecords: Ref<UsageRecord[]>
  /** 当筛选变化时的回调 */
  onFilterChange?: () => void
}

export function useUsageFilters(options: UseUsageFiltersOptions) {
  const { allRecords, onFilterChange } = options

  // 筛选状态
  const filterModel = ref('__all__')
  const filterProvider = ref('__all__')
  const filterStatus = ref<FilterStatusValue>('__all__')

  // Select 打开状态
  const filterModelSelectOpen = ref(false)
  const filterProviderSelectOpen = ref(false)
  const filterStatusSelectOpen = ref(false)

  // 可用模型和提供商选项
  const availableModels = computed(() => {
    const models = new Set<string>()
    allRecords.value.forEach(record => {
      if (record.model) models.add(record.model)
    })
    return Array.from(models).sort()
  })

  const availableProviders = computed(() => {
    const providers = new Set<string>()
    allRecords.value.forEach(record => {
      if (record.provider) providers.add(record.provider)
    })
    return Array.from(providers).sort()
  })

  // 是否有活跃的筛选条件
  const hasActiveFilters = computed(() => {
    return filterModel.value !== '__all__' ||
           filterProvider.value !== '__all__' ||
           filterStatus.value !== '__all__'
  })

  // 筛选后的记录
  const filteredRecords = computed(() => {
    if (!hasActiveFilters.value) {
      return allRecords.value
    }

    let records = [...allRecords.value]

    if (filterModel.value !== '__all__') {
      records = records.filter(record => record.model === filterModel.value)
    }

    if (filterProvider.value !== '__all__') {
      records = records.filter(record => record.provider === filterProvider.value)
    }

    if (filterStatus.value !== '__all__') {
      if (filterStatus.value === 'stream') {
        records = records.filter(record =>
          isUsageUpstreamStream(record) && !isUsageRecordFailed(record)
        )
      } else if (filterStatus.value === 'standard') {
        records = records.filter(record =>
          !isUsageUpstreamStream(record) && !isUsageRecordFailed(record)
        )
      } else if (filterStatus.value === 'active') {
        records = records.filter(record =>
          resolveDisplayRequestStatus(record) === 'pending' ||
          resolveDisplayRequestStatus(record) === 'streaming'
        )
      } else if (filterStatus.value === 'failed') {
        records = records.filter(record => isUsageRecordFailed(record))
      } else if (filterStatus.value === 'cancelled') {
        records = records.filter(record => record.status === 'cancelled')
      } else if (filterStatus.value === 'has_fallback') {
        records = records.filter(record => hasUsageFallback(record))
      } else if (filterStatus.value === 'has_retry') {
        records = records.filter(record => hasUsageRetry(record))
      }
    }

    return records
  })

  // 筛选后的总记录数
  const filteredTotalRecords = computed(() => filteredRecords.value.length)

  // 处理筛选变化
  function handleFilterModelChange(value: string) {
    filterModel.value = value
    onFilterChange?.()
  }

  function handleFilterProviderChange(value: string) {
    filterProvider.value = value
    onFilterChange?.()
  }

  function handleFilterStatusChange(value: string) {
    filterStatus.value = value as FilterStatusValue
    onFilterChange?.()
  }

  // 重置所有筛选
  function resetFilters() {
    filterModel.value = '__all__'
    filterProvider.value = '__all__'
    filterStatus.value = '__all__'
    onFilterChange?.()
  }

  return {
    // 筛选状态
    filterModel,
    filterProvider,
    filterStatus,

    // Select 打开状态
    filterModelSelectOpen,
    filterProviderSelectOpen,
    filterStatusSelectOpen,

    // 可用选项
    availableModels,
    availableProviders,

    // 计算属性
    hasActiveFilters,
    filteredRecords,
    filteredTotalRecords,

    // 方法
    handleFilterModelChange,
    handleFilterProviderChange,
    handleFilterStatusChange,
    resetFilters
  }
}
