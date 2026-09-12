<template>
  <div class="space-y-6 pb-8">
    <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
      <div>
        <h1 class="text-2xl font-semibold text-foreground">
          邀请返利
        </h1>
        <p class="mt-1 text-sm text-muted-foreground">
          查看邀请关系、返利记录和失败返利处理状态
        </p>
      </div>
      <Button
        variant="outline"
        :disabled="loading"
        @click="loadAll"
      >
        <RefreshCw
          class="mr-2 h-4 w-4"
          :class="{ 'animate-spin': loading }"
        />
        刷新
      </Button>
    </div>

    <div class="grid grid-cols-1 gap-4 md:grid-cols-5">
      <Card
        v-for="item in statCards"
        :key="item.label"
        class="p-4"
      >
        <p class="text-xs text-muted-foreground">
          {{ item.label }}
        </p>
        <p class="mt-2 text-xl font-semibold">
          {{ item.value }}
        </p>
      </Card>
    </div>

    <Card class="overflow-hidden">
      <div class="border-b border-border px-5 py-4">
        <h2 class="text-base font-semibold">
          邀请关系
        </h2>
      </div>
      <div class="grid grid-cols-1 gap-3 border-b border-border/70 p-4 md:grid-cols-5">
        <Input
          v-model="relationshipFilters.inviter"
          placeholder="邀请人"
        />
        <Input
          v-model="relationshipFilters.invitee"
          placeholder="被邀请人"
        />
        <Input
          v-model="relationshipFilters.invite_code"
          placeholder="邀请码"
        />
        <Select v-model="firstPaidFilter">
          <SelectTrigger>
            <SelectValue placeholder="首付状态" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">
              全部
            </SelectItem>
            <SelectItem value="true">
              已首付
            </SelectItem>
            <SelectItem value="false">
              未首付
            </SelectItem>
          </SelectContent>
        </Select>
        <Button
          type="button"
          @click="loadRelationships"
        >
          查询
        </Button>
      </div>

      <div class="overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>邀请人</TableHead>
              <TableHead>被邀请人</TableHead>
              <TableHead>邀请码</TableHead>
              <TableHead>绑定时间</TableHead>
              <TableHead>首付状态</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="item in relationships"
              :key="item.id"
            >
              <TableCell>{{ item.inviter_username || item.inviter_user_id }}</TableCell>
              <TableCell>{{ item.invitee_username || item.invitee_user_id }}</TableCell>
              <TableCell class="font-mono text-xs">
                {{ item.invite_code_snapshot }}
              </TableCell>
              <TableCell>{{ formatUnix(item.created_at_unix_secs) }}</TableCell>
              <TableCell>
                <Badge :variant="item.first_paid_order_id ? 'success' : 'secondary'">
                  {{ item.first_paid_order_id ? '已首付' : '未首付' }}
                </Badge>
              </TableCell>
            </TableRow>
            <TableRow v-if="relationships.length === 0">
              <TableCell
                colspan="5"
                class="py-8 text-center text-sm text-muted-foreground"
              >
                暂无邀请关系
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </Card>

    <Card class="overflow-hidden">
      <div class="border-b border-border px-5 py-4">
        <h2 class="text-base font-semibold">
          返利记录
        </h2>
      </div>
      <div class="grid grid-cols-1 gap-3 border-b border-border/70 p-4 md:grid-cols-5">
        <Input
          v-model="rewardFilters.order_id"
          placeholder="订单号"
        />
        <Select v-model="rewardFilters.reward_type">
          <SelectTrigger>
            <SelectValue placeholder="返利类型" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">
              全部类型
            </SelectItem>
            <SelectItem value="percent">
              比例返利
            </SelectItem>
            <SelectItem value="headcount">
              人头返利
            </SelectItem>
          </SelectContent>
        </Select>
        <Select v-model="rewardFilters.status">
          <SelectTrigger>
            <SelectValue placeholder="状态" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">
              全部状态
            </SelectItem>
            <SelectItem value="pending">
              待发
            </SelectItem>
            <SelectItem value="failed">
              失败
            </SelectItem>
            <SelectItem value="applied">
              已发
            </SelectItem>
            <SelectItem value="voided">
              已作废
            </SelectItem>
            <SelectItem value="reversed">
              已冲回
            </SelectItem>
          </SelectContent>
        </Select>
        <Button
          type="button"
          class="md:col-start-5"
          @click="loadRewards"
        >
          查询
        </Button>
      </div>

      <div class="overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>类型</TableHead>
              <TableHead>来源订单</TableHead>
              <TableHead>金额</TableHead>
              <TableHead>状态</TableHead>
              <TableHead>冲回</TableHead>
              <TableHead>创建时间</TableHead>
              <TableHead class="text-right">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="item in rewards"
              :key="item.id"
            >
              <TableCell>{{ getRewardTypeLabel(item.reward_type) }}</TableCell>
              <TableCell class="font-mono text-xs">
                {{ item.source_order_id || '-' }}
              </TableCell>
              <TableCell>{{ formatUsd(item.amount_usd) }}</TableCell>
              <TableCell>
                <Badge :variant="getRewardStatusVariant(item.status)">
                  {{ getRewardStatusLabel(item.status) }}
                </Badge>
              </TableCell>
              <TableCell>
                {{ formatUsd(item.reversed_amount_usd) }}
                <span
                  v-if="item.pending_reversal_amount_usd > 0"
                  class="text-xs text-amber-600 dark:text-amber-400"
                >
                  / 待冲回 {{ formatUsd(item.pending_reversal_amount_usd) }}
                </span>
              </TableCell>
              <TableCell>{{ formatUnix(item.created_at_unix_secs) }}</TableCell>
              <TableCell class="text-right">
                <div class="flex justify-end gap-2">
                  <Button
                    v-if="item.status === 'failed'"
                    variant="outline"
                    size="sm"
                    :disabled="mutatingRewardId === item.id"
                    @click="retryReward(item)"
                  >
                    补发
                  </Button>
                  <Button
                    v-if="item.status === 'failed' || item.status === 'pending'"
                    variant="ghost"
                    size="sm"
                    :disabled="mutatingRewardId === item.id"
                    @click="voidReward(item)"
                  >
                    作废
                  </Button>
                </div>
              </TableCell>
            </TableRow>
            <TableRow v-if="rewards.length === 0">
              <TableCell
                colspan="7"
                class="py-8 text-center text-sm text-muted-foreground"
              >
                暂无返利记录
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { getI18nLocale } from '@/i18n'
import { computed, onMounted, ref } from 'vue'
import { RefreshCw } from 'lucide-vue-next'
import {
  referralApi,
  type ReferralRelationshipRecord,
  type ReferralRewardRecord,
  type ReferralSummary
} from '@/api/referrals'
import {
  Badge,
  Button,
  Card,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from '@/components/ui'
import { useToast } from '@/composables/useToast'

const relationships = ref<ReferralRelationshipRecord[]>([])
const rewards = ref<ReferralRewardRecord[]>([])
const stats = ref<ReferralSummary>({
  total_invites: 0,
  effective_invites: 0,
  paid_reward_usd: 0,
  pending_reward_usd: 0,
  reversed_reward_usd: 0
})
const loading = ref(false)
const mutatingRewardId = ref<string | null>(null)
const relationshipFilters = ref({
  inviter: '',
  invitee: '',
  invite_code: ''
})
const firstPaidFilter = ref('all')
const rewardFilters = ref({
  order_id: '',
  reward_type: 'all',
  status: 'all'
})
const { success, error: showError } = useToast()

const statCards = computed(() => [
  { label: '总邀请', value: stats.value.total_invites },
  { label: '有效邀请', value: stats.value.effective_invites },
  { label: '已发返利', value: formatUsd(stats.value.paid_reward_usd) },
  { label: '待发返利', value: formatUsd(stats.value.pending_reward_usd) },
  { label: '已冲回返利', value: formatUsd(stats.value.reversed_reward_usd) },
])

function formatUsd(value: number): string {
  return `$${Number(value || 0).toFixed(2)}`
}

function formatUnix(value?: number | null): string {
  if (!value) return '-'
  return new Date(value * 1000).toLocaleString(getI18nLocale())
}

function getRewardTypeLabel(value: string): string {
  if (value === 'percent') return '比例返利'
  if (value === 'headcount') return '人头返利'
  return value
}

function getRewardStatusLabel(value: string): string {
  switch (value) {
    case 'applied':
      return '已发'
    case 'pending':
      return '待发'
    case 'failed':
      return '失败'
    case 'voided':
      return '已作废'
    case 'reversed':
      return '已冲回'
    default:
      return value
  }
}

function getRewardStatusVariant(value: string): 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark' {
  switch (value) {
    case 'applied':
      return 'success'
    case 'failed':
      return 'destructive'
    case 'pending':
      return 'warning'
    case 'voided':
      return 'secondary'
    default:
      return 'outline'
  }
}

async function loadRelationships() {
  const firstPaid =
    firstPaidFilter.value === 'true' ? true : firstPaidFilter.value === 'false' ? false : null
  const response = await referralApi.getAdminReferrals({
    ...relationshipFilters.value,
    first_paid: firstPaid,
    limit: 100,
    offset: 0
  })
  relationships.value = response.items
  stats.value = response.stats
}

async function loadRewards() {
  const response = await referralApi.getAdminReferralRewards({
    order_id: rewardFilters.value.order_id,
    reward_type: rewardFilters.value.reward_type === 'all' ? undefined : rewardFilters.value.reward_type,
    status: rewardFilters.value.status === 'all' ? undefined : rewardFilters.value.status,
    limit: 100,
    offset: 0
  })
  rewards.value = response.items
}

async function loadAll() {
  loading.value = true
  try {
    await Promise.all([loadRelationships(), loadRewards()])
  } catch {
    showError('加载邀请返利数据失败')
  } finally {
    loading.value = false
  }
}

async function retryReward(item: ReferralRewardRecord) {
  mutatingRewardId.value = item.id
  try {
    const response = await referralApi.retryReferralReward(item.id, '管理员后台补发')
    replaceReward(response.reward)
    success('返利已补发')
  } catch {
    showError('补发失败')
  } finally {
    mutatingRewardId.value = null
  }
}

async function voidReward(item: ReferralRewardRecord) {
  mutatingRewardId.value = item.id
  try {
    const response = await referralApi.voidReferralReward(item.id, '管理员后台作废')
    replaceReward(response.reward)
    success('返利已作废')
  } catch {
    showError('作废失败')
  } finally {
    mutatingRewardId.value = null
  }
}

function replaceReward(updated: ReferralRewardRecord) {
  rewards.value = rewards.value.map(item => item.id === updated.id ? updated : item)
}

onMounted(() => {
  void loadAll()
})
</script>
