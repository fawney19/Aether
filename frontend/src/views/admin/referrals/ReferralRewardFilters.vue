<template>
  <form
    class="min-w-0"
    @submit.prevent="$emit('query')"
  >
    <div class="flex flex-wrap items-center gap-2">
      <ReferralUserSelect
        v-model="filters.inviter"
        class="w-full sm:w-36"
        label="邀请人"
      />
      <ReferralUserSelect
        v-model="filters.invitee"
        class="w-full sm:w-36"
        label="被邀请人"
      />
      <Input
        v-model="filters.order_no"
        class="w-full sm:w-40"
        placeholder="订单号"
        aria-label="订单号"
      />
      <select
        v-if="full"
        v-model="filters.status"
        class="h-9 w-full rounded-md border border-border bg-background px-3 text-sm sm:w-auto"
        aria-label="发放状态"
      >
        <option value="all">
          全部发放状态
        </option>
        <option value="pending">
          待发
        </option>
        <option value="applying">
          处理中
        </option>
        <option value="failed">
          发放失败
        </option>
        <option value="applied">
          已发
        </option>
        <option value="reversed">
          已发且全部冲回
        </option>
        <option value="voided">
          已作废
        </option>
      </select>
      <div class="flex flex-wrap items-center gap-1 sm:ml-auto">
        <Button
          type="submit"
          size="sm"
          :aria-busy="loading"
        >
          <Search class="mr-1.5 h-3.5 w-3.5" />查询
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          @click="$emit('reset')"
        >
          重置
        </Button>
        <Button
          v-if="full"
          type="button"
          variant="ghost"
          size="sm"
          aria-label="更多条件"
          :aria-expanded="expanded"
          @click="expanded = !expanded"
        >
          <SlidersHorizontal class="mr-1.5 h-3.5 w-3.5" />更多条件<span
            v-if="advancedCount"
            class="ml-1 text-primary"
          >{{ advancedCount }}</span>
        </Button>
      </div>
    </div>
    <div
      v-if="full && expanded"
      class="mt-3 grid gap-2 border-t border-border pt-3 sm:grid-cols-2 lg:grid-cols-4"
    >
      <label class="space-y-1 text-xs text-muted-foreground">
        <span>邀请关系</span><Input
          v-model="filters.referral_id"
          placeholder="关系 ID"
          aria-label="关系 ID"
        />
      </label>
      <label class="space-y-1 text-xs text-muted-foreground">
        <span>返利类型</span>
        <select
          v-model="filters.reward_type"
          class="h-9 w-full rounded-md border border-border bg-background px-3 text-sm text-foreground"
          aria-label="返利类型"
        >
          <option value="all">全部类型</option><option value="percent">比例返利</option><option value="headcount">人头返利</option>
        </select>
      </label>
      <label class="space-y-1 text-xs text-muted-foreground">
        <span>触发条件</span>
        <select
          v-model="filters.trigger_point"
          class="h-9 w-full rounded-md border border-border bg-background px-3 text-sm text-foreground"
          aria-label="触发条件"
        >
          <option value="all">全部触发条件</option><option value="registration">注册</option><option value="email_verified">邮箱验证</option><option value="first_paid_order">首次实际付款</option><option value="paid_order">实际付款</option>
        </select>
      </label>
      <label class="space-y-1 text-xs text-muted-foreground">
        <span>冲回情况</span>
        <select
          v-model="filters.pending_reversal"
          class="h-9 w-full rounded-md border border-border bg-background px-3 text-sm text-foreground"
          aria-label="待冲回筛选"
        >
          <option value="all">全部冲回情况</option><option value="true">有待冲回</option><option value="false">无待冲回</option>
        </select>
      </label>
    </div>
  </form>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { Search, SlidersHorizontal } from 'lucide-vue-next'
import { Button, Input } from '@/components/ui'
import ReferralUserSelect from './ReferralUserSelect.vue'
export interface RewardFilters { inviter: string; invitee: string; order_no: string; referral_id: string; reward_type: string; status: string; trigger_point: string; pending_reversal: string }
defineProps<{ loading: boolean; full?: boolean }>()
defineEmits<{ query: []; reset: [] }>()
const filters = defineModel<RewardFilters>('filters', { required: true })
const expanded = ref(false)
const advancedCount = computed(() => [filters.value.referral_id.trim(), ...[filters.value.reward_type, filters.value.trigger_point, filters.value.pending_reversal].map(value => value === 'all' ? '' : value)].filter(Boolean).length)
</script>
