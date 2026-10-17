import { computed, onScopeDispose, ref, shallowRef, watch } from 'vue'
import type { LocationQuery } from 'vue-router'
import { adminWalletApi, type AdminGlobalRefund, type AdminLedgerTransaction, type AdminWalletDetailResponse } from '@/api/admin-wallets'
import { adminPaymentsApi } from '@/api/admin-payments'
import type { PaymentOrder } from '@/api/wallet'
import { parseApiError } from '@/utils/errorParser'

export type WalletLocation =
  | { kind: 'wallets'; item: AdminWalletDetailResponse }
  | { kind: 'orders'; item: PaymentOrder }
  | { kind: 'refunds'; item: AdminGlobalRefund }
  | { kind: 'ledger'; item: AdminLedgerTransaction }

// 详情请求独立于列表分页，来自邀请记录的目标不会因不在第一页而丢失。
export function useWalletLocation(query: () => LocationQuery, reset: () => void, resolve: (location: WalletLocation) => void) {
  const target = shallowRef<WalletLocation | null>(null)
  const loading = ref(false)
  const error = ref('')
  const targetId = ref('')
  let sequence = 0
  let controller: AbortController | null = null
  const locationKey = computed(() => {
    const current = query()
    return JSON.stringify([current.tab, current.wallet_id, current.order_id, current.refund_id, current.transaction_id])
  })
  function value(key: string) {
    const raw = query()[key]
    return (Array.isArray(raw) ? raw[0] : raw)?.trim() || ''
  }
  async function locate() {
    const generation = ++sequence
    controller?.abort()
    controller = new AbortController()
    reset()
    target.value = null
    error.value = ''
    loading.value = false
    const tab = value('tab')
    const walletId = value('wallet_id')
    const id = tab === 'wallets' ? walletId : tab === 'orders' ? value('order_id') : tab === 'refunds' ? value('refund_id') : tab === 'ledger' ? value('transaction_id') : ''
    targetId.value = id
    if (!id) return
    if ((tab === 'refunds' || tab === 'ledger') && !walletId) {
      error.value = '定位链接缺少钱包 ID，请核对原记录。'
      return
    }
    loading.value = true
    try {
      let found: WalletLocation
      const signal = controller.signal
      if (tab === 'wallets') found = { kind: 'wallets', item: await adminWalletApi.getWalletDetail(id, signal) }
      else if (tab === 'orders') found = { kind: 'orders', item: (await adminPaymentsApi.getOrder(id)).order }
      else if (tab === 'refunds') found = { kind: 'refunds', item: (await adminWalletApi.getRefundDetail(walletId, id, signal)).refund }
      else found = { kind: 'ledger', item: (await adminWalletApi.getTransactionDetail(walletId, id, signal)).transaction }
      if (generation !== sequence) return
      if (found.item.id !== id || ((found.kind === 'refunds' || found.kind === 'ledger') && found.item.wallet_id !== walletId)) {
        throw new Error('返回记录与定位目标不一致')
      }
      target.value = found
      resolve(found)
    } catch (cause) {
      if (generation !== sequence) return
      error.value = parseApiError(cause, '无法读取目标，记录可能已删除或当前账号无权访问。')
    } finally {
      if (generation === sequence) loading.value = false
    }
  }
  watch(locationKey, () => { void locate() }, { immediate: true })
  onScopeDispose(() => { sequence++; controller?.abort() })
  return { target, loading, error, targetId, retry: locate }
}
