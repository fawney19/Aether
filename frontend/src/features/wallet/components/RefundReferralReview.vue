<template>
  <Dialog
    :open="open"
    title="退款返利核查"
    size="lg"
    @update:open="onOpenChange"
  >
    <div
      v-if="preview"
      class="space-y-4"
    >
      <p class="text-sm text-muted-foreground">
        {{ preview.stage === 'process' ? '批准退款前，请核对邀请返利冲回情况。' : '完成退款前，已重新核对邀请返利冲回情况。' }}
      </p>
      <div
        v-for="(item, index) in preview.rewards"
        :key="`${item.inviter_user_id}-${index}`"
        class="rounded-lg border p-3 text-sm"
      >
        <p class="font-medium">
          邀请人：{{ item.inviter_username || item.inviter_user_id }}
        </p>
        <dl class="mt-2 grid grid-cols-2 gap-2">
          <dt>截至本次退款待扣回返利</dt><dd>{{ usd(item.expected_reversal_usd) }}</dd>
          <dt>可用赠款</dt><dd>{{ usd(item.available_gift_usd) }}</dd>
          <dt>预计可扣回</dt><dd>{{ usd(item.deductible_usd) }}</dd>
          <dt>预计待冲回</dt><dd>{{ usd(item.shortfall_usd) }}</dd>
        </dl>
      </div>
      <p
        v-if="preview.rewards.length === 0"
        class="text-sm"
      >
        此退款没有需要冲回的邀请返利。
      </p>
      <div
        v-if="hasShortfall"
        class="rounded-lg border border-amber-500/40 bg-amber-500/10 p-3 text-sm"
        role="alert"
      >
        <p>邀请人赠款不足，预计 {{ usd(preview.total_shortfall_usd) }} 无法立即扣回。由管理员决定是否继续退款。</p>
        <p class="mt-1 text-muted-foreground">
          待扣回金额包含此前尚未冲回的返利。批准后，差额保留为待冲回，不扣邀请人的充值本金。
        </p>
        <label class="mt-3 flex items-start gap-2">
          <Checkbox
            :checked="acknowledged"
            @update:checked="acknowledged = !!$event"
          />
          <span>已知晓余额不足，仍同意继续退款</span>
        </label>
      </div>
    </div>
    <template #footer>
      <Button
        :disabled="!canApprove"
        @click="resolveReview('approve')"
      >
        {{ preview?.stage === 'process' ? '批准并处理退款' : '同意完成退款' }}
      </Button>
      <Button
        v-if="allowReject"
        variant="destructive"
        @click="resolveReview('reject')"
      >
        转为驳回退款
      </Button>
      <Button
        variant="outline"
        @click="resolveReview('cancel')"
      >
        取消
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from 'vue'
import { Button, Checkbox, Dialog } from '@/components/ui'
import { adminWalletApi, type ReferralRefundPreview } from '@/api/admin-wallets'

const props = withDefaults(defineProps<{ active?: boolean; contextKey?: string }>(), { active: true, contextKey: '' })

type Decision = { decision: 'approve' | 'reject' | 'cancel'; token?: string }
const open = ref(false)
const preview = ref<ReferralRefundPreview | null>(null)
const acknowledged = ref(false)
const allowReject = ref(true)
let generation = 0
let controller: AbortController | null = null
let pending: ((value: Decision) => void) | null = null
// 与服务端金额容差一致，浮点尾差不会产生无法取得确认凭据的审批。
const hasShortfall = computed(() => !!preview.value && preview.value.total_shortfall_usd > 0.00000001)
const canApprove = computed(() => !!preview.value && (!hasShortfall.value || (acknowledged.value && !!preview.value.confirmation_token)))
const usd = (value: number) => `$${value.toFixed(value !== 0 && Math.abs(value) < 0.01 ? 8 : 2)}`

async function review(walletId: string, refundId: string, stage: 'process' | 'complete', canReject = true): Promise<Decision> {
  cancelReview()
  if (!props.active) return { decision: 'cancel' }
  const currentGeneration = generation
  const requestController = new AbortController()
  controller = requestController
  acknowledged.value = false
  allowReject.value = canReject
  preview.value = null
  try {
    const latest = await adminWalletApi.getReferralRefundPreview(walletId, refundId, stage, requestController.signal)
    if (generation !== currentGeneration || !props.active) return { decision: 'cancel' }
    preview.value = latest
    open.value = true
    return new Promise(resolve => { pending = resolve })
  } catch (error) {
    if (generation !== currentGeneration || requestController.signal.aborted) return { decision: 'cancel' }
    throw error
  } finally {
    if (controller === requestController) controller = null
  }
}
function cancelReview() {
  generation += 1
  controller?.abort()
  controller = null
  resolveReview('cancel')
}
function resolveReview(decision: Decision['decision']) {
  if (decision === 'approve' && !canApprove.value) return
  const resolve = pending
  pending = null
  open.value = false
  resolve?.({ decision, token: decision === 'approve' ? preview.value?.confirmation_token || undefined : undefined })
}
function onOpenChange(value: boolean) { if (!value) cancelReview() }
watch(() => [props.active, props.contextKey], () => cancelReview())
onUnmounted(cancelReview)
defineExpose({ review })
</script>
