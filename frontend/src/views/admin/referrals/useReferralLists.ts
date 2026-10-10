import { onScopeDispose, reactive, watch } from 'vue'
import type { ReferralListResponse } from '@/api/referrals'

// 仅供本页面的四个业务列表使用；每组保留独立分页、已提交条件和请求序号。
export function useReferralList<T>(fetchPage: (limit: number, offset: number) => Promise<ReferralListResponse<T>>) {
  const state = reactive({ items: [] as T[], total: 0, page: 1, pageSize: 20, loading: false, error: '', loaded: false })
  let sequence = 0
  async function load(): Promise<void> {
    const request = ++sequence
    const { page, pageSize } = state
    state.loading = true
    state.error = ''
    try {
      const result = await fetchPage(pageSize, (page - 1) * pageSize)
      if (request !== sequence) return
      state.total = result.total
      const lastPage = Math.max(1, Math.ceil(result.total / pageSize))
      if (page > lastPage) { state.page = lastPage; return }
      state.items = result.items as typeof state.items
      state.loaded = true
    } catch {
      if (request === sequence) state.error = '加载失败，请重试'
    } finally {
      if (request === sequence) state.loading = false
    }
  }
  function query() { if (state.page !== 1) state.page = 1; else void load() }
  watch(() => state.page, () => { void load() })
  watch(() => state.pageSize, query)
  onScopeDispose(() => { sequence++ })
  return { state, load, query }
}
