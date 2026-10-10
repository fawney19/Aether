<template>
  <Dialog
    :open="!!operation"
    :title="operation?.action === 'void' ? '确认作废返利' : '确认重试发放'"
    :persistent="busy"
    :z-index="90"
    size="lg"
    @update:open="!$event && $emit('close')"
  >
    <div
      v-if="operation"
      class="space-y-3 py-3 text-sm"
    >
      <p class="break-all">
        {{ operation.reward.inviter_username || operation.reward.inviter_user_id }} ← {{ operation.reward.invitee_username || operation.reward.invitee_user_id }}
      </p>
      <p class="break-all font-mono text-xs text-muted-foreground">
        奖励 {{ operation.reward.id }}
      </p><p>奖励原额 {{ usd(operation.reward.amount_usd) }} · {{ trigger(operation.reward.trigger_point) }}</p>
      <p
        v-if="loading"
        role="status"
      >
        正在核对最新状态和历史规则…
      </p><p
        v-else-if="error"
        role="alert"
        class="text-destructive"
      >
        核查失败，请关闭后重试。
      </p>
      <template v-else-if="detail">
        <p>当前状态：{{ rewardStatus(detail.reward.status) }}</p><p v-if="detail.rule_snapshot">
          当时规则：{{ detail.rule_snapshot.percent_enabled ? `比例 ${detail.rule_snapshot.percent_rate}%` : '' }} {{ detail.rule_snapshot.headcount_enabled ? `人头 ${usd(detail.rule_snapshot.headcount_amount_usd)} / ${trigger(detail.rule_snapshot.headcount_trigger)}` : '' }}
        </p><p v-else>
          历史规则未记录，重试不会套用当前费率。
        </p><p
          v-if="detail.source_order && detail.source_order.refunded_amount_usd > 0"
          class="rounded-lg bg-amber-500/10 p-3"
        >
          来源订单已经退款，发放只入账扣除应冲回后的净赠款；全额冲回不代表新增可用余额。
        </p>
      </template>
      <p
        v-if="operation.action === 'void'"
        class="rounded-lg bg-muted/40 p-3"
      >
        停止该奖励后续发放，同一邀请关系的人头奖励不会因切换规则重新生成。已发奖励不能作废。
      </p>
      <label
        class="block"
        for="referral-operation-note"
      >{{ operation.action === 'void' ? '作废原因（必填）' : '操作备注（选填）' }}</label><Textarea
        id="referral-operation-note"
        v-model="note"
        :disabled="busy"
        :maxlength="1000"
        placeholder="填写操作原因，便于后续核对"
      />
    </div>
    <template #footer>
      <Button
        :variant="operation?.action === 'void' ? 'destructive' : 'default'"
        :disabled="!canConfirm"
        @click="$emit('confirm', note.trim())"
      >
        {{ busy ? '正在处理…' : operation?.action === 'void' ? '确认作废' : '确认重试' }}
      </Button><Button
        variant="outline"
        :disabled="busy"
        @click="$emit('close')"
      >
        取消
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { ReferralRewardDetail, ReferralRewardRecord } from '@/api/referrals'
import { Button, Dialog, Textarea } from '@/components/ui'
import { rewardStatus, trigger, usd } from './presentation'
export interface RewardOperation { action: 'retry' | 'void'; reward: ReferralRewardRecord }
const props = defineProps<{ operation: RewardOperation | null; detail: ReferralRewardDetail | null; loading: boolean; error: boolean; busy: boolean }>()
defineEmits<{ close: []; confirm: [note: string] }>()
const note = ref('')
watch(() => props.operation, () => { note.value = '' })
const canConfirm = computed(() => !!props.operation && !props.busy && !props.loading && !props.error && !!props.detail && props.detail.reward.id === props.operation.reward.id && (props.operation.action === 'retry' ? props.detail.reward.status === 'failed' : ['pending', 'failed'].includes(props.detail.reward.status)) && (props.operation.action !== 'void' || !!note.value.trim()))
</script>
