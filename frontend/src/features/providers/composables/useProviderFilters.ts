import { ref, computed, watch } from 'vue'
import type { ProviderSummaryQuery } from '@/api/endpoints'
import { API_FORMAT_ORDER, formatApiFormat } from '@/api/endpoints/types/api-format'
import { useI18n } from '@/i18n'

export interface FilterOption {
  value: string
  label: string
}

export function useProviderFilters(
  globalModels: () => { id: string; name: string }[],
) {
  const { legacyT } = useI18n()
  // 搜索与筛选
  const searchQuery = ref('')
  const filterStatus = ref('all')
  const filterApiFormat = ref('all')
  const filterModel = ref('all')

  const statusFilters = computed<FilterOption[]>(() => [
    { value: 'all', label: legacyT('全部状态') },
    { value: 'active', label: legacyT('活跃') },
    { value: 'inactive', label: legacyT('停用') },
  ])

  const apiFormatFilters = computed<FilterOption[]>(() => [
    { value: 'all', label: legacyT('全部格式') },
    ...API_FORMAT_ORDER.map(value => ({ value, label: formatApiFormat(value) })),
  ])

  const modelFilters = computed<FilterOption[]>(() => {
    const items = globalModels()
      .map(m => ({ value: m.id, label: m.name }))
      .sort((a, b) => a.label.localeCompare(b.label))
    return [{ value: 'all', label: legacyT('全部模型') }, ...items]
  })

  const hasActiveFilters = computed(() => {
    return (
      searchQuery.value !== '' ||
      filterStatus.value !== 'all' ||
      filterApiFormat.value !== 'all' ||
      filterModel.value !== 'all'
    )
  })

  // 分页
  const currentPage = ref(1)
  const pageSize = ref(20)
  const total = ref(0)

  // 服务端分页查询参数
  const queryParams = computed<ProviderSummaryQuery>(() => ({
    page: currentPage.value,
    page_size: pageSize.value,
    search: searchQuery.value.trim() || undefined,
    status: filterStatus.value !== 'all' ? filterStatus.value : undefined,
    api_format: filterApiFormat.value !== 'all' ? filterApiFormat.value : undefined,
    model_id: filterModel.value !== 'all' ? filterModel.value : undefined,
  }))

  // 搜索/筛选变化时重置分页到第1页
  watch([searchQuery, filterStatus, filterApiFormat, filterModel], () => {
    currentPage.value = 1
  })

  function resetFilters() {
    searchQuery.value = ''
    filterStatus.value = 'all'
    filterApiFormat.value = 'all'
    filterModel.value = 'all'
  }

  return {
    searchQuery,
    filterStatus,
    filterApiFormat,
    filterModel,
    statusFilters,
    apiFormatFilters,
    modelFilters,
    hasActiveFilters,
    currentPage,
    pageSize,
    total,
    queryParams,
    resetFilters,
  }
}
