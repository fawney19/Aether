import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import { createMemoryHistory, createRouter } from 'vue-router'
import type { ReferralRewardDetail, ReferralRewardRecord } from '@/api/referrals'
const mocks = vi.hoisted(() => ({ overview: vi.fn(), detail: vi.fn(), relationships: vi.fn(), rewards: vi.fn(), retry: vi.fn(), void: vi.fn(), success: vi.fn(), info: vi.fn(), error: vi.fn(), users: vi.fn(), user: vi.fn() }))
vi.mock('@/api/users', () => ({ usersApi: { getAllUsers: mocks.users, getUser: mocks.user } }))
vi.mock('@/api/referrals', () => ({ referralApi: { getAdminOverview: mocks.overview, getReferralRewardDetail: mocks.detail, getAdminReferrals: mocks.relationships, getAdminReferralRewards: mocks.rewards, retryReferralReward: mocks.retry, voidReferralReward: mocks.void } }))
vi.mock('@/composables/useToast', () => ({ useToast: () => ({ success: mocks.success, info: mocks.info, error: mocks.error }) }))
import ReferralManagement from '../ReferralManagement.vue'
const mounted: Array<{ app: App; root: HTMLElement }> = []
const stats = { total_invites: 121, effective_invites: 1, paid_reward_usd: 10, cumulative_reward_usd: 17, pending_reward_usd: 2, reversed_reward_usd: 4, failed_reward_count: 1, pending_reversal_reward_usd: 2, pending_reversal_reward_count: 1 }
const rules = { available: true, enabled: true, reward_mode: 'both', recharge_percent: 5, headcount_amount_usd: 2, headcount_trigger: 'registration' }
const reward: ReferralRewardRecord = { id: 'reward-1', referral_id: 'relation-1', inviter_user_id: 'alice-id', inviter_username: 'Alice', invitee_user_id: 'bob-id', invitee_username: 'Bob', status: 'failed', amount_usd: 10, reward_type: 'headcount', trigger_point: 'registration', reversed_amount_usd: 0, pending_reversal_amount_usd: 0, created_at_unix_secs: 1, updated_at_unix_secs: 2 }
const detail = (record = reward): ReferralRewardDetail => ({ reward: record, relationship: null, rule_snapshot: null, source_order: null, ledger_entries: [], refunds: [] })
const page = (items: unknown[], total = 121) => ({ items, stats, total, limit: 20, offset: 0 })
async function settle() { for (let i = 0; i < 12; i++) { await Promise.resolve(); await nextTick() } }
async function mount() { const root = document.createElement('div'); document.body.appendChild(root); const router = createRouter({ history: createMemoryHistory(), routes: [{ path: '/:pathMatch(.*)*', component: { template: '<div />' } }] }); await router.push('/admin/referrals'); const app = createApp(ReferralManagement); app.use(router); app.mount(root); mounted.push({ app, root }); await settle(); return root }
function button(root: ParentNode, text: string) { const found = [...root.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent?.trim() === text); if (!found) throw new Error(`Missing ${text}`); return found }
function list(root: Element, kind: string) { return root.querySelector(`[data-list="${kind}"]`)! }
function selectQueue(root: Element, kind: 'failed' | 'debt') { root.querySelector<HTMLButtonElement>(`[aria-label="${kind === 'failed' ? '发放失败' : '待冲回款项'}队列"]`)!.click() }
function input(root: ParentNode, placeholder: string, value: string) { const element = root.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[placeholder="${placeholder}"]`)!; element.value = value; element.dispatchEvent(new Event('input')); return element }
async function selectPerson(root: ParentNode, label: string, name: string) {
  const trigger = root.querySelector<HTMLButtonElement>(`[aria-label="${label}"]`)!
  trigger.click(); await settle()
  const option = [...trigger.parentElement!.querySelectorAll<HTMLButtonElement>('button')].find(item => item.querySelector('span.block.truncate')?.textContent?.trim() === name)!
  option.click(); await settle()
}
async function confirmVoid() { await settle(); input(document, '填写操作原因，便于后续核对', '误发资格'); await settle(); button(document, '确认作废').click(); await settle() }
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done }); return { promise, resolve } }
beforeEach(() => {
  vi.resetAllMocks()
  const people = [{ id: 'alice-id', username: 'Alice', email: 'alice@example.com' }, { id: 'bob-id', username: 'Bob', email: 'bob@example.com' }, { id: 'filtered-id', username: 'Filtered', email: '' }]
  mocks.users.mockResolvedValue(people)
  mocks.user.mockImplementation(async id => people.find(user => user.id === id))
  mocks.overview.mockResolvedValue({ stats, rules })
  mocks.detail.mockImplementation(async (id: string) => detail({ ...reward, id }))
  mocks.relationships.mockResolvedValue(page([{ id: 'relation-1', inviter_user_id: 'Alice', invitee_user_id: 'Bob', invite_code_snapshot: 'CODE', created_at_unix_secs: 1 }]))
  mocks.rewards.mockImplementation(async query => page(query.pending_reversal ? [{ ...reward, id: 'debt-1', status: 'applied', reversed_amount_usd: 4, pending_reversal_amount_usd: 2 }] : [reward]))
})
afterEach(() => { for (const { app, root } of mounted.splice(0)) { app.unmount(); root.remove() } })
describe('ReferralManagement', () => {
  it('queries selected inviter and invitee IDs independently and clears both on reset', async () => {
    const root = await mount(); const card = list(root, 'failed')
    expect(mocks.users).not.toHaveBeenCalled()
    await selectPerson(card, '邀请人', 'Alice')
    await selectPerson(card, '被邀请人', 'Bob')
    button(card, '查询').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ inviter: 'alice-id', invitee: 'bob-id', status: 'failed', offset: 0 }))
    button(card, '重置').click(); await settle()
    expect(mocks.rewards.mock.lastCall?.[0].inviter).toBeUndefined()
    expect(mocks.rewards.mock.lastCall?.[0].invitee).toBeUndefined()
    expect(card.querySelector('[aria-label="邀请人"]')?.textContent?.trim()).toBe('邀请人')
    expect(card.querySelector('[aria-label="被邀请人"]')?.textContent?.trim()).toBe('被邀请人')
  })
  it('loads two independent queues while presenting only the selected queue', async () => {
    const root = await mount()
    expect(root.textContent).toContain('待处理')
    expect(mocks.rewards).toHaveBeenCalledWith(expect.objectContaining({ status: 'failed', offset: 0, limit: 20 }))
    expect(mocks.rewards).toHaveBeenCalledWith(expect.objectContaining({ pending_reversal: true, offset: 0, limit: 20 }))
    expect(mocks.relationships).not.toHaveBeenCalled()
    expect(root.textContent).toContain('累计已发$17.00')
    expect((list(root, 'failed') as HTMLElement).style.display).not.toBe('none')
    expect((list(root, 'debt') as HTMLElement).style.display).toBe('none')
    selectQueue(root, 'debt'); await settle()
    expect((list(root, 'failed') as HTMLElement).style.display).toBe('none')
    expect((list(root, 'debt') as HTMLElement).style.display).not.toBe('none')
    expect(root.querySelector('[aria-label="待冲回款项队列"]')?.getAttribute('aria-pressed')).toBe('true')
    expect(list(root, 'debt').textContent).toContain('原额 $10.00')
    expect(list(root, 'debt').textContent).toContain('已冲回 $4.00')
    expect(list(root, 'debt').textContent).toContain('待冲回 $2.00')
  })
  it('restores the selected user name when the records tab remounts', async () => {
    const root = await mount(); button(root, '返利记录').click(); await settle()
    await selectPerson(list(root, 'rewards'), '邀请人', 'Alice')
    button(root, '邀请关系').click(); await settle()
    button(root, '返利记录').click(); await settle()
    expect(list(root, 'rewards').querySelector('[aria-label="邀请人"]')?.textContent?.trim()).toBe('Alice')
    button(list(root, 'rewards'), '查询').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ inviter: 'alice-id' }))
  })
  it('preserves each queue page and submitted filter through switches without mixing financial meanings', async () => {
    const root = await mount()
    await selectPerson(list(root, 'failed'), '邀请人', 'Alice'); button(list(root, 'failed'), '查询').click(); await settle()
    button(list(root, 'failed'), '7').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'failed', inviter: 'alice-id', offset: 120 }))
    selectQueue(root, 'debt'); await settle()
    input(list(root, 'debt'), '订单号', 'ORDER-2'); button(list(root, 'debt'), '查询').click(); await settle()
    button(list(root, 'debt'), '2').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ pending_reversal: true, order_no: 'ORDER-2', offset: 20 }))
    expect(mocks.rewards.mock.lastCall?.[0].status).toBeUndefined()
    const calls = mocks.rewards.mock.calls.length
    selectQueue(root, 'failed'); await settle()
    expect(mocks.rewards).toHaveBeenCalledTimes(calls)
    expect(list(root, 'failed').querySelector('[aria-current="page"]')?.textContent?.trim()).toBe('7')
    expect(list(root, 'failed').querySelector('[aria-label="邀请人"]')?.textContent?.trim()).toBe('Alice')
    selectQueue(root, 'debt'); await settle()
    expect(list(root, 'debt').querySelector('[aria-current="page"]')?.textContent?.trim()).toBe('2')
    const queues = root.querySelector('[aria-label="待处理队列"]')!
    expect(queues.textContent).toContain('发放失败 1')
    expect(queues.textContent).toContain('待冲回款项 1')
    expect(queues.textContent).not.toContain('$2.00')
    expect(root.querySelector('[aria-label="发放失败队列"]')?.textContent).toContain('1')
    expect(root.querySelector('[aria-label="发放失败队列"]')?.textContent).not.toContain('121')
  })
  it('starts with existing debt when the initial failed queue is empty', async () => {
    mocks.rewards.mockImplementation(async query => page(query.status === 'failed' ? [] : [{ ...reward, pending_reversal_amount_usd: 2 }], query.status === 'failed' ? 0 : 1))
    const root = await mount()
    expect(root.querySelector('[aria-label="待冲回款项队列"]')?.getAttribute('aria-pressed')).toBe('true')
    selectQueue(root, 'failed'); await settle(); button(root, '刷新').click(); await settle()
    expect(root.querySelector('[aria-label="发放失败队列"]')?.getAttribute('aria-pressed')).toBe('true')
  })
  it('does not replace a queue choice made while the initial requests are still loading', async () => {
    const debtResult = deferred<ReturnType<typeof page>>()
    mocks.rewards.mockImplementation(async query => query.status === 'failed' ? page([], 0) : debtResult.promise)
    const root = await mount()
    selectQueue(root, 'debt'); await settle(); selectQueue(root, 'failed'); await settle()
    debtResult.resolve(page([{ ...reward, pending_reversal_amount_usd: 2 }], 1)); await settle()
    expect(root.querySelector('[aria-label="发放失败队列"]')?.getAttribute('aria-pressed')).toBe('true')
  })
  it('retains advanced filter values when their panel closes and submits every supported condition', async () => {
    const root = await mount(); button(root, '返利记录').click(); await settle()
    const card = list(root, 'rewards')
    expect(card.querySelector('[aria-label="关系 ID"]')).toBeNull()
    button(card, '更多条件').click(); await settle()
    input(card, '关系 ID', 'relation-23')
    for (const [label, value] of [['返利类型', 'percent'], ['触发条件', 'paid_order'], ['待冲回筛选', 'false'], ['发放状态', 'applied']]) {
      const select = card.querySelector<HTMLSelectElement>(`[aria-label="${label}"]`)!
      select.value = value; select.dispatchEvent(new Event('change'))
    }
    await settle()
    card.querySelector<HTMLButtonElement>('[aria-label="更多条件"]')!.click(); await settle()
    expect(card.querySelector('[aria-label="关系 ID"]')).toBeNull()
    button(card, '查询').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ referral_id: 'relation-23', reward_type: 'percent', trigger_point: 'paid_order', pending_reversal: false, status: 'applied', offset: 0 }))
    card.querySelector<HTMLButtonElement>('[aria-label="更多条件"]')!.click(); await settle()
    expect(card.querySelector<HTMLInputElement>('[aria-label="关系 ID"]')?.value).toBe('relation-23')
  })
  it('pages both business lists beyond 100 and resets actual order-number and relationship queries to page one', async () => {
    const root = await mount(); button(root, '邀请关系').click(); await settle()
    const relationships = list(root, 'relationships'); button(relationships, '7').click(); await settle()
    expect(mocks.relationships).toHaveBeenLastCalledWith(expect.objectContaining({ limit: 20, offset: 120 }))
    await selectPerson(relationships, '邀请人', 'Alice'); button(relationships, '查询').click(); await settle()
    expect(mocks.relationships).toHaveBeenLastCalledWith(expect.objectContaining({ inviter: 'alice-id', offset: 0 }))
    button(root, '返利记录').click(); await settle()
    const rewards = list(root, 'rewards'); button(rewards, '7').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 120 }))
    input(rewards, '订单号', 'order-no-9'); button(rewards, '查询').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ order_no: 'order-no-9', offset: 0 }))
    expect(mocks.rewards.mock.lastCall?.[0]).not.toHaveProperty('order_id')
  })
  it('does not announce void success when the returned record was already applied', async () => {
    mocks.void.mockResolvedValue({ reward: { ...reward, status: 'applied' } })
    const root = await mount(); button(list(root, 'failed'), '作废').click(); await confirmVoid()
    expect(mocks.void).toHaveBeenCalledWith('reward-1', '误发资格')
    expect(mocks.success).not.toHaveBeenCalled()
    expect(mocks.error).toHaveBeenCalledWith('返利状态已变化，未作废，请核对最新记录')
  })
  it('requires a reason and refreshes after a concurrent void conflict', async () => {
    mocks.void.mockRejectedValue(new Error('conflict'))
    const root = await mount(); button(list(root, 'failed'), '作废').click(); await settle()
    expect(button(document, '确认作废').disabled).toBe(true)
    mocks.rewards.mockImplementation(async query => page(query.status === 'failed' ? [] : [{ ...reward, status: 'applied' }], 0))
    await confirmVoid()
    expect(mocks.success).not.toHaveBeenCalled()
    expect(list(root, 'failed').textContent).toContain('没有发放失败')
    expect(mocks.overview).toHaveBeenCalledTimes(2)
  })
  it.each(['relationships', 'rewards'] as const)('ignores stale page and filter responses for %s', async kind => {
    const root = await mount(); button(root, kind === 'relationships' ? '邀请关系' : '返利记录').click(); await settle()
    const card = list(root, kind); const api = kind === 'relationships' ? mocks.relationships : mocks.rewards
    const result = (name: string, total: number) => page(kind === 'relationships' ? [{ id: name, inviter_user_id: name, invitee_user_id: 'Bob', invite_code_snapshot: 'CODE', created_at_unix_secs: 1 }] : [{ ...reward, inviter_username: name }], total)
    const first = deferred<unknown>(); const second = deferred<unknown>(); api.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise)
    button(card, '2').click(); await settle(); button(card, '3').click(); await settle()
    second.resolve(result('latest-page', 47)); await settle(); first.resolve(result('obsolete-page', 121)); await settle()
    expect(card.textContent).toContain('latest-page'); expect(card.textContent).not.toContain('obsolete-page')
    expect(card.querySelector('[aria-live="polite"]')?.textContent).toContain('47')
    const stale = deferred<unknown>(); const filtered = deferred<unknown>(); api.mockReturnValueOnce(stale.promise).mockReturnValueOnce(filtered.promise)
    button(card, '2').click(); await settle()
    if (kind === 'relationships') await selectPerson(card, '邀请人', 'Filtered')
    else input(card, '订单号', 'filtered')
    button(card, '查询').click(); await settle()
    expect(api).toHaveBeenLastCalledWith(expect.objectContaining({ offset: 0 }))
    stale.resolve(result('obsolete-filter', 121)); await settle(); expect(button(card, '查询').getAttribute('aria-busy')).toBe('true')
    filtered.resolve(result('filtered-page', 1)); await settle(); expect(card.textContent).toContain('filtered-page'); expect(card.textContent).not.toContain('obsolete-filter')
  })
  it('keeps independent queue pages, clamps a deleted last page, and never computes global totals from rows', async () => {
    const root = await mount(); const failed = list(root, 'failed'); button(failed, '7').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'failed', offset: 120 }))
    mocks.rewards.mockImplementation(async query => page([reward], query.status === 'failed' ? 21 : 121))
    button(failed, '刷新列表').click(); await settle()
    expect(mocks.rewards).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'failed', offset: 20 }))
    expect(list(root, 'debt').querySelector('[aria-current="page"]')?.textContent?.trim()).toBe('1')
    expect(root.textContent).toContain('累计已发$17.00')
  })
  it('shows unavailable overview and an independent list error rather than presenting false zero values', async () => {
    mocks.overview.mockRejectedValue(new Error('offline'))
    mocks.rewards.mockImplementation(async query => { if (query.status === 'failed') throw new Error('offline'); return page([], 0) })
    const root = await mount(); expect(root.textContent).toContain('全局统计和规则加载失败')
    expect(root.textContent).not.toContain('累计已发$0.00'); expect(list(root, 'failed').textContent).toContain('加载失败')
    expect(list(root, 'debt').textContent).toContain('没有待冲回款项')
  })
  it('shows eight decimal nonzero amounts and applying/reversed as separate grant and reversal states', async () => {
    mocks.rewards.mockResolvedValue(page([{ ...reward, amount_usd: 0.00000001, status: 'applying' }, { ...reward, id: 'reversed-1', status: 'reversed', reversed_amount_usd: 10 }]))
    const root = await mount(); expect(root.textContent).toContain('$0.00000001'); expect(root.textContent).toContain('处理中'); expect(root.textContent).toContain('全部冲回')
    const processing = list(root, 'failed').querySelector('tbody tr')!; expect(processing.textContent).not.toContain('作废'); expect(processing.textContent).not.toContain('重试发放')
  })
  it('does not resurrect a closed detail from a late response or apply current rules to missing history', async () => {
    const pending = deferred<ReferralRewardDetail>(); mocks.detail.mockReturnValueOnce(pending.promise)
    const root = await mount(); button(list(root, 'failed'), '详情').click(); await settle(); button(document, '关闭详情').click(); await settle()
    pending.resolve(detail()); await settle(); expect(document.querySelector('[aria-label="返利记录详情"]')).toBeNull()
    button(list(root, 'failed'), '详情').click(); await settle(); expect(document.body.textContent).toContain('历史规则未记录'); expect(document.body.textContent).toContain('未记录失败原因')
  })
  it('locks the mutation target while a different detail opens and prevents duplicate submits', async () => {
    const pending = deferred<{ reward: ReferralRewardRecord }>(); mocks.retry.mockReturnValueOnce(pending.promise)
    const root = await mount(); button(list(root, 'failed'), '重试发放').click(); await settle(); button(document, '确认重试').click(); await settle()
    selectQueue(root, 'debt'); await settle(); button(list(root, 'debt'), '详情').click(); await settle(); button(document, '正在处理…').click(); await settle()
    expect(mocks.retry).toHaveBeenCalledExactlyOnceWith('reward-1', undefined)
    pending.resolve({ reward: { ...reward, status: 'failed' } }); await settle(); expect(mocks.success).not.toHaveBeenCalled(); expect(mocks.error).toHaveBeenCalledWith('重试后仍发放失败，请核对记录')
  })
  it('reports full reversal truthfully and explains refunded delayed grants before retry', async () => {
    mocks.detail.mockResolvedValue({ ...detail(), source_order: { id: 'order-1', wallet_id: 'wallet-1', order_no: 'ORDER1', order_kind: 'recharge', amount_usd: 10, refunded_amount_usd: 10, status: 'refunded' } })
    mocks.retry.mockResolvedValue({ reward: { ...reward, status: 'reversed', reversed_amount_usd: 10 } })
    const root = await mount(); button(list(root, 'failed'), '重试发放').click(); await settle(); expect(document.body.textContent).toContain('只入账扣除应冲回后的净赠款')
    button(document, '确认重试').click(); await settle(); expect(mocks.success).not.toHaveBeenCalled(); expect(mocks.info).toHaveBeenCalledWith('返利已全部冲回，没有新增可用赠款')
  })
  it('does not revive a cancelled operation with a late review and ignores mismatched detail identifiers', async () => {
    const late = deferred<ReferralRewardDetail>(); mocks.detail.mockReturnValueOnce(late.promise)
    const root = await mount(); button(list(root, 'failed'), '重试发放').click(); await settle(); button(document, '取消').click(); await settle()
    late.resolve(detail()); await settle(); expect(document.querySelector('[aria-label="确认重试发放"]')).toBeNull()
    mocks.detail.mockResolvedValue(detail({ ...reward, id: 'another-reward' })); button(list(root, 'failed'), '重试发放').click(); await settle()
    expect(button(document, '确认重试').disabled).toBe(true); button(document, '确认重试').click(); await settle(); expect(mocks.retry).not.toHaveBeenCalled()
  })
  it('shows a historical snapshot and exact wallet/order/refund links without substituting current rules', async () => {
    mocks.detail.mockResolvedValue({ ...detail({ ...reward, inviter_wallet_id: 'inviter-wallet' }), rule_snapshot: { percent_enabled: true, percent_rate: 2, headcount_enabled: false, headcount_amount_usd: 0, headcount_trigger: 'registration' }, source_order: { id: 'source-order', order_no: 'ORDER-X', wallet_id: 'invitee-wallet', order_kind: 'recharge', amount_usd: 100, refunded_amount_usd: 60, status: 'credited' }, refunds: [{ id: 'refund-x', refund_no: 'REFUND-X', status: 'succeeded', refund_mode: 'original_channel', wallet_id: 'invitee-wallet', refund_amount_usd: 60, created_at_unix_secs: 1 }] })
    const root = await mount(); button(list(root, 'failed'), '详情').click(); await settle()
    const drawer = document.querySelector('[aria-label="返利记录详情"]')!
    expect(drawer.textContent).toContain('比例返利 2%'); expect(drawer.textContent).not.toContain('比例返利 5%')
    expect(drawer.textContent).toContain('已入账'); expect(drawer.textContent).toContain('已成功')
    const links = [...drawer.querySelectorAll('a')].map(item => item.getAttribute('href'))
    expect(links).toContain('/admin/wallets?tab=orders&order_id=source-order')
    expect(links).toContain('/admin/wallets?tab=wallets&wallet_id=inviter-wallet')
    expect(links).toContain('/admin/wallets?tab=refunds&wallet_id=invitee-wallet&refund_id=refund-x')
  })
})
