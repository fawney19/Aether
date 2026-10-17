<template>
  <div class="max-w-6xl space-y-6 pb-8">
    <div>
      <h1 class="text-2xl font-semibold text-foreground">
        我的邀请
      </h1>
      <p class="mt-1 text-sm text-muted-foreground">
        分享邀请链接，邀请好友注册
      </p>
    </div>

    <Card
      v-if="loading"
      class="flex items-center gap-3 p-6 text-sm text-muted-foreground"
      role="status"
    >
      <Loader2
        class="h-4 w-4 animate-spin"
        aria-hidden="true"
      />
      正在加载...
    </Card>

    <template v-else-if="dashboard">
      <Card class="overflow-hidden">
        <div class="space-y-6 p-5 sm:p-6">
          <div class="flex items-start gap-3">
            <div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
              <Gift
                class="h-5 w-5"
                aria-hidden="true"
              />
            </div>
            <div>
              <h2 class="text-lg font-semibold">
                邀请好友
              </h2>
              <p class="mt-1 text-sm text-muted-foreground">
                将链接发送给好友，注册时会自动带上你的邀请码。
              </p>
            </div>
          </div>

          <div>
            <Label for="referral-invitation-link">
              邀请链接
            </Label>
            <div class="mt-2 flex min-w-0 flex-col gap-3 sm:flex-row">
              <Input
                id="referral-invitation-link"
                :model-value="invitationLink"
                readonly
                class="min-w-0 font-mono text-sm sm:flex-1"
                @focus="selectInvitationLink"
              />
              <Button
                type="button"
                class="shrink-0 gap-2"
                @click="copyToClipboard(invitationLink)"
              >
                <Copy
                  class="h-4 w-4"
                  aria-hidden="true"
                />
                复制邀请链接
              </Button>
            </div>
          </div>
        </div>

        <div class="flex flex-wrap items-center gap-x-4 gap-y-2 border-t border-border/60 bg-muted/30 px-5 py-3 sm:px-6">
          <span class="text-sm text-muted-foreground">邀请码</span>
          <code class="break-all font-mono text-sm font-semibold tracking-wide">{{ dashboard.invite_code }}</code>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            class="gap-2"
            @click="copyToClipboard(dashboard.invite_code)"
          >
            <Copy
              class="h-3.5 w-3.5"
              aria-hidden="true"
            />
            复制邀请码
          </Button>
          <span class="text-xs text-muted-foreground">邀请码已包含在上方链接中</span>
        </div>
      </Card>

      <Card class="p-5 sm:p-6">
        <div class="mb-5">
          <h2 class="text-base font-semibold">
            邀请与返利
          </h2>
          <p class="mt-1 text-sm text-muted-foreground">
            符合规则的返利会进入赠款余额，可在钱包中心查看。
          </p>
        </div>

        <dl class="grid grid-cols-2 gap-x-6 gap-y-5 sm:grid-cols-3 lg:grid-cols-5">
          <div>
            <dt class="text-xs text-muted-foreground">
              总邀请
            </dt>
            <dd class="mt-2 text-2xl font-semibold tabular-nums">
              {{ dashboard.summary.total_invites }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">
              有效邀请
            </dt>
            <dd class="mt-2 text-2xl font-semibold tabular-nums">
              {{ dashboard.summary.effective_invites }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">
              已发返利
            </dt>
            <dd class="mt-2 break-all text-2xl font-semibold tabular-nums text-primary">
              {{ formatUsd(dashboard.summary.paid_reward_usd) }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">
              待发返利
            </dt>
            <dd class="mt-2 break-all text-2xl font-semibold tabular-nums">
              {{ formatUsd(dashboard.summary.pending_reward_usd) }}
            </dd>
          </div>
          <div>
            <dt class="text-xs text-muted-foreground">
              已冲回返利
            </dt>
            <dd class="mt-2 break-all text-2xl font-semibold tabular-nums">
              {{ formatUsd(dashboard.summary.reversed_reward_usd) }}
            </dd>
          </div>
        </dl>

        <div
          v-if="dashboard.summary.total_invites === 0"
          class="mt-5 flex items-start gap-3 rounded-xl bg-muted/40 p-4"
        >
          <Users
            class="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground"
            aria-hidden="true"
          />
          <div>
            <p class="text-sm font-medium">
              还没有邀请记录
            </p>
            <p class="mt-1 text-sm text-muted-foreground">
              复制上方链接，开始邀请第一位好友。
            </p>
          </div>
        </div>

        <div class="mt-5 space-y-1 border-t border-border/60 pt-4 text-xs leading-relaxed text-muted-foreground">
          <p>有效邀请：已完成首次付款的受邀用户。</p>
          <p>待发返利尚未入账；返利发放与冲回明细以钱包流水为准。</p>
        </div>
      </Card>
    </template>

    <Card
      v-else
      class="space-y-3 p-6"
    >
      <p class="text-sm text-muted-foreground">
        邀请数据暂不可用
      </p>
      <Button
        type="button"
        variant="outline"
        size="sm"
        class="gap-2"
        @click="loadReferralDashboard"
      >
        <RefreshCw
          class="h-4 w-4"
          aria-hidden="true"
        />
        重新加载
      </Button>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { Copy, Gift, Loader2, RefreshCw, Users } from 'lucide-vue-next'
import { referralApi, type ReferralDashboardResponse } from '@/api/referrals'
import { Button, Card, Input, Label } from '@/components/ui'
import { useClipboard } from '@/composables/useClipboard'
import { useToast } from '@/composables/useToast'

const dashboard = ref<ReferralDashboardResponse | null>(null)
const loading = ref(false)
const { copyToClipboard } = useClipboard()
const { error: showError } = useToast()

// 后端默认返回相对路径，分享时需补全当前站点的协议、域名和端口。
const invitationLink = computed(() => dashboard.value
  ? new URL(dashboard.value.invitation_link, window.location.origin).href
  : '')

function selectInvitationLink(event: FocusEvent) {
  (event.target as HTMLInputElement).select()
}

function formatUsd(value: number): string {
  return `$${Number(value || 0).toFixed(2)}`
}

async function loadReferralDashboard() {
  loading.value = true
  try {
    dashboard.value = await referralApi.getMyReferral()
  } catch {
    dashboard.value = null
    showError('加载邀请数据失败')
  } finally {
    loading.value = false
  }
}

onMounted(() => {
  void loadReferralDashboard()
})
</script>
