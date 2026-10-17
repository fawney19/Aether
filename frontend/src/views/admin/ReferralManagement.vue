<template>
  <div class="space-y-6 pb-8">
    <div class="grid grid-cols-[1fr_auto] items-center gap-x-6 gap-y-3 xl:grid-cols-[minmax(8rem,1fr)_auto_minmax(12rem,1fr)]">
      <div>
        <h1 class="text-2xl font-semibold">
          邀请返利
        </h1>
      </div>
      <ReferralOverview
        summary
        class="col-span-2 row-start-2 xl:col-span-1 xl:row-start-auto"
        :overview="overview"
        :loading="overviewLoading"
        :error="overviewError"
      />
      <div class="col-start-2 row-start-1 flex flex-wrap justify-end gap-2 xl:col-start-3">
        <RouterLink
          to="/admin/system"
          class="inline-flex items-center gap-2 rounded-lg px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <Settings2 class="h-4 w-4" />规则设置
        </RouterLink><Button
          variant="outline"
          :disabled="refreshing"
          @click="refresh"
        >
          <RefreshCw
            class="mr-2 h-4 w-4"
            :class="{ 'animate-spin': refreshing }"
          />刷新
        </Button>
      </div>
    </div>
    <ReferralOverview
      :overview="overview"
      :loading="overviewLoading"
      :error="overviewError"
      @reload="loadOverview"
    />
    <nav
      class="flex gap-5 overflow-x-auto border-b border-border"
      aria-label="返利管理分类"
    >
      <button
        v-for="item in tabs"
        :key="item.value"
        type="button"
        class="relative inline-flex shrink-0 items-center gap-2 border-b-2 px-1 pb-3 text-sm font-medium transition-colors"
        :class="tab === item.value ? 'border-primary text-primary' : 'border-transparent text-muted-foreground hover:text-foreground'"
        :aria-pressed="tab === item.value"
        @click="tab = item.value"
      >
        <component
          :is="item.icon"
          class="h-4 w-4"
        />
        {{ item.label }}
      </button>
    </nav>
    <div
      v-if="tab === 'pending'"
      class="space-y-4"
    >
      <div
        class="grid gap-3 sm:grid-cols-2"
        aria-label="待处理队列"
      >
        <button
          v-for="queue in queues"
          :key="queue.value"
          type="button"
          :aria-label="`${queue.label}队列`"
          :aria-pressed="queueTab === queue.value"
          class="flex min-w-0 items-center gap-3 rounded-xl border p-4 text-left transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          :class="queueTab === queue.value ? 'border-primary/50 bg-primary/5' : 'border-border bg-card hover:bg-muted/30'"
          @click="selectQueue(queue.value)"
        >
          <span
            class="flex h-10 w-10 shrink-0 items-center justify-center rounded-lg"
            :class="queueTab === queue.value ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground'"
          >
            <component
              :is="queue.icon"
              class="h-5 w-5"
            />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block text-sm font-semibold">{{ queue.label }} <span
              class="ml-2 inline-flex rounded-md bg-background/70 px-1.5 py-0.5 text-xs font-medium tabular-nums"
              :class="queueTab === queue.value ? 'text-primary' : 'text-muted-foreground'"
            >{{ queue.count }}</span></span>
          </span>
          <ChevronRight
            class="h-4 w-4 shrink-0"
            :class="queueTab === queue.value ? 'text-primary' : 'text-muted-foreground'"
          />
        </button>
      </div>
      <ReferralRewardList
        v-show="queueTab === 'failed'"
        title="发放失败"
        kind="failed"
        v-bind="listProps(failed)"
        :mutating="mutating"
        @page="failed.state.page = $event"
        @size="failed.state.pageSize = $event"
        @reload="failed.load"
        @detail="openDetail"
        @operate="openOperation"
      >
        <template #filters>
          <ReferralRewardFilters
            v-model:filters="failedFilters"
            :loading="failed.state.loading"
            @query="queryFailed"
            @reset="resetFailed"
          />
        </template>
      </ReferralRewardList>
      <ReferralRewardList
        v-show="queueTab === 'debt'"
        title="待冲回款项"
        kind="debt"
        v-bind="listProps(debt)"
        :mutating="mutating"
        @page="debt.state.page = $event"
        @size="debt.state.pageSize = $event"
        @reload="debt.load"
        @detail="openDetail"
        @operate="openOperation"
      >
        <template #filters>
          <ReferralRewardFilters
            v-model:filters="debtFilters"
            :loading="debt.state.loading"
            @query="queryDebt"
            @reset="resetDebt"
          />
        </template>
      </ReferralRewardList>
    </div>
    <ReferralRewardList
      v-if="tab === 'rewards'"
      title="返利记录"
      kind="rewards"
      v-bind="listProps(rewards)"
      :mutating="mutating"
      @page="rewards.state.page = $event"
      @size="rewards.state.pageSize = $event"
      @reload="rewards.load"
      @detail="openDetail"
      @operate="openOperation"
    >
      <template #filters>
        <ReferralRewardFilters
          v-model:filters="rewardFilters"
          full
          :loading="rewards.state.loading"
          @query="queryRewards"
          @reset="resetRewards"
        />
      </template>
    </ReferralRewardList>
    <ReferralRelationshipList
      v-if="tab === 'relationships'"
      v-model:filters="relationshipFilters"
      v-bind="listProps(relationships)"
      @page="relationships.state.page = $event"
      @size="relationships.state.pageSize = $event"
      @reload="relationships.load"
      @query="queryRelationships"
      @reset="resetRelationships"
      @rewards="showRelationshipRewards"
      @copy="copy"
    />
    <ReferralDetailDrawer
      :id="detailId"
      :detail="detail"
      :loading="detailLoading"
      :error="detailError"
      :mutating="mutating"
      @close="closeDetail"
      @reload="loadDetail"
      @copy="copy"
      @operate="openOperation"
    />
    <ReferralOperationDialog
      :operation="operation"
      :detail="operationDetail"
      :loading="operationLoading"
      :error="operationError"
      :busy="!!mutating"
      @close="closeOperation"
      @confirm="confirmOperation"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onScopeDispose, ref, watch } from 'vue'
import { RouterLink } from 'vue-router'
import { RefreshCw, Settings2, CircleAlert, History, Users, RotateCcw, ChevronRight } from 'lucide-vue-next'
import { Button } from '@/components/ui'
import { referralApi, type AdminReferralOverview, type ReferralRewardDetail, type ReferralRewardQuery, type ReferralRewardRecord } from '@/api/referrals'
import { useToast } from '@/composables/useToast'
import ReferralOverview from './referrals/ReferralOverview.vue'
import ReferralRewardList from './referrals/ReferralRewardList.vue'
import ReferralRewardFilters, { type RewardFilters } from './referrals/ReferralRewardFilters.vue'
import ReferralRelationshipList from './referrals/ReferralRelationshipList.vue'
import ReferralDetailDrawer from './referrals/ReferralDetailDrawer.vue'
import ReferralOperationDialog, { type RewardOperation } from './referrals/ReferralOperationDialog.vue'
import { useReferralList } from './referrals/useReferralLists'
import { usd } from './referrals/presentation'
const toast = useToast()
const tab = ref('pending')
const queueTab = ref<'failed' | 'debt'>('failed')
let queueSelected = false
let initialQueuesLoaded = false
function selectQueue(value: 'failed' | 'debt') { queueSelected = true; queueTab.value = value }
const overview = ref<AdminReferralOverview | null>(null)
const overviewLoading = ref(false)
const overviewError = ref(false)
let overviewSequence = 0
const tabs = [{ value: 'pending', label: '待处理', icon: CircleAlert }, { value: 'rewards', label: '返利记录', icon: History }, { value: 'relationships', label: '邀请关系', icon: Users }]
const queues = computed(() => [
  { value: 'failed' as const, label: '发放失败', count: overview.value && !overviewError.value ? overview.value.stats.failed_reward_count : '—', icon: CircleAlert },
  { value: 'debt' as const, label: '待冲回款项', count: overview.value && !overviewError.value ? overview.value.stats.pending_reversal_reward_count : '—', icon: RotateCcw },
])
const blankFilters = (): RewardFilters => ({ inviter: '', invitee: '', order_no: '', referral_id: '', reward_type: 'all', status: 'all', trigger_point: 'all', pending_reversal: 'all' })
const rewardFilters = ref(blankFilters())
const failedFilters = ref(blankFilters())
const debtFilters = ref(blankFilters())
let rewardQuery: ReferralRewardQuery = {}
let failedQuery: ReferralRewardQuery = {}
let debtQuery: ReferralRewardQuery = {}
const rewards = useReferralList((limit, offset) => referralApi.getAdminReferralRewards({ ...rewardQuery, limit, offset }))
const failed = useReferralList((limit, offset) => referralApi.getAdminReferralRewards({ ...failedQuery, status: 'failed', limit, offset }))
const debt = useReferralList((limit, offset) => referralApi.getAdminReferralRewards({ ...debtQuery, pending_reversal: true, limit, offset }))
const blankRelationships = () => ({ inviter: '', invitee: '', invite_code: '', first_paid: 'all' })
const relationshipFilters = ref(blankRelationships())
let relationshipQuery = {}
const relationships = useReferralList((limit, offset) => referralApi.getAdminReferrals({ ...relationshipQuery, limit, offset }))
function listProps<T>(list: ReturnType<typeof useReferralList<T>>) { return { items: list.state.items, total: list.state.total, page: list.state.page, pageSize: list.state.pageSize, loading: list.state.loading, error: list.state.error } }
function filterQuery(filters: RewardFilters): ReferralRewardQuery {
  const clean = (value: string) => value === 'all' || !value.trim() ? undefined : value.trim()
  return { inviter: clean(filters.inviter), invitee: clean(filters.invitee), order_no: clean(filters.order_no), referral_id: clean(filters.referral_id), reward_type: clean(filters.reward_type), status: clean(filters.status), trigger_point: clean(filters.trigger_point), pending_reversal: filters.pending_reversal === 'all' ? undefined : filters.pending_reversal === 'true' }
}
function queryRewards() { rewardQuery = filterQuery(rewardFilters.value); rewards.query() }
function queryFailed() { failedQuery = filterQuery(failedFilters.value); failed.query() }
function queryDebt() { debtQuery = filterQuery(debtFilters.value); debt.query() }
function resetRewards() { rewardFilters.value = blankFilters(); queryRewards() }
function resetFailed() { failedFilters.value = blankFilters(); queryFailed() }
function resetDebt() { debtFilters.value = blankFilters(); queryDebt() }
function queryRelationships() { const f = relationshipFilters.value; relationshipQuery = { inviter: f.inviter.trim(), invitee: f.invitee.trim(), invite_code: f.invite_code.trim(), first_paid: f.first_paid === 'all' ? undefined : f.first_paid === 'true' }; relationships.query() }
function resetRelationships() { relationshipFilters.value = blankRelationships(); queryRelationships() }
function showRelationshipRewards(id: string) { rewardFilters.value = { ...blankFilters(), referral_id: id }; rewardQuery = filterQuery(rewardFilters.value); tab.value = 'rewards'; if (rewards.state.page !== 1) rewards.state.page = 1; else void rewards.load() }
async function loadOverview() {
  const request = ++overviewSequence
  overviewLoading.value = true; overviewError.value = false
  try { const result = await referralApi.getAdminOverview(); if (request === overviewSequence) overview.value = result }
  catch { if (request === overviewSequence) overviewError.value = true }
  finally { if (request === overviewSequence) overviewLoading.value = false }
}
const refreshing = ref(false)
async function refresh() {
  refreshing.value = true
  await Promise.all([loadOverview(), failed.load(), debt.load(), ...(rewards.state.loaded || tab.value === 'rewards' ? [rewards.load()] : []), ...(relationships.state.loaded || tab.value === 'relationships' ? [relationships.load()] : [])])
  if (!initialQueuesLoaded) {
    initialQueuesLoaded = true
    // 首次无失败记录时呈现有款项的队列；用户已选择后不再跳转。
    if (!queueSelected && failed.state.loaded && !failed.state.error && failed.state.total === 0 && debt.state.loaded && !debt.state.error && debt.state.total > 0) queueTab.value = 'debt'
  }
  refreshing.value = false
}
watch(tab, value => { if (value === 'rewards' && !rewards.state.loaded && !rewards.state.loading) void rewards.load(); if (value === 'relationships' && !relationships.state.loaded && !relationships.state.loading) void relationships.load() })
const detailId = ref<string | null>(null)
const detail = ref<ReferralRewardDetail | null>(null)
const detailLoading = ref(false)
const detailError = ref(false)
let detailSequence = 0
function openDetail(id: string) { detailId.value = id; detail.value = null; void loadDetail() }
function closeDetail() { detailSequence++; detailId.value = null; detail.value = null; detailLoading.value = false }
async function loadDetail() {
  const id = detailId.value
  if (!id) return
  const request = ++detailSequence
  detailLoading.value = true; detailError.value = false
  try { const result = await referralApi.getReferralRewardDetail(id); if (request === detailSequence && detailId.value === id) detail.value = result }
  catch { if (request === detailSequence) detailError.value = true }
  finally { if (request === detailSequence) detailLoading.value = false }
}
const operation = ref<RewardOperation | null>(null)
const operationDetail = ref<ReferralRewardDetail | null>(null)
const operationLoading = ref(false)
const operationError = ref(false)
const mutating = ref<string | null>(null)
let operationSequence = 0
async function openOperation(action: 'retry' | 'void', reward: ReferralRewardRecord) {
  if (mutating.value) return
  const request = ++operationSequence
  operation.value = { action, reward: { ...reward } }; operationDetail.value = null; operationLoading.value = true; operationError.value = false
  try { const result = await referralApi.getReferralRewardDetail(reward.id); if (request === operationSequence) operationDetail.value = result }
  catch { if (request === operationSequence) operationError.value = true }
  finally { if (request === operationSequence) operationLoading.value = false }
}
function closeOperation() { if (mutating.value) return; operationSequence++; operation.value = null; operationDetail.value = null }
async function confirmOperation(note: string) {
  const target = operation.value
  if (!target || mutating.value || operationLoading.value || operationError.value || !operationDetail.value || operationDetail.value.reward.id !== target.reward.id || (target.action === 'void' && !note.trim())) return
  const { action, reward: { id } } = target
  mutating.value = id
  try {
    const { reward } = action === 'retry' ? await referralApi.retryReferralReward(id, note || undefined) : await referralApi.voidReferralReward(id, note)
    if (action === 'void') { if (reward.status === 'voided') toast.success('返利已作废'); else toast.error('返利状态已变化，未作废，请核对最新记录') }
    else if (reward.status === 'applied') toast.success(`返利已发放${reward.reversed_amount_usd > 0 ? `，累计冲回 ${usd(reward.reversed_amount_usd)}` : ''}`)
    else if (reward.status === 'reversed') toast.info('返利已全部冲回，没有新增可用赠款')
    else if (reward.status === 'failed') toast.error('重试后仍发放失败，请核对记录')
    else toast.info(`重试已受理，当前${reward.status === 'applying' ? '处理中' : reward.status === 'pending' ? '待发' : reward.status}`)
  } catch { toast.error(action === 'void' ? '作废失败，记录可能已发放，请核对最新状态' : '重试失败，请核对最新状态') }
  finally { mutating.value = null; closeOperation(); await refresh(); if (detailId.value === id) await loadDetail() }
}
async function copy(value: string) { try { await navigator.clipboard.writeText(value); toast.success('已复制') } catch { toast.error('复制失败，请手动复制') } }
onMounted(() => { void refresh() })
onScopeDispose(() => { overviewSequence++; detailSequence++; operationSequence++ })
</script>
