<template>
  <CardSection
    title="充值配置"
    description="配置前台充值入口、支付通道、限额、过期时间与到账比例"
  >
    <template #actions>
      <Button
        size="sm"
        :disabled="loading || !hasChanges"
        @click="handleSave"
      >
        {{ loading ? '保存中...' : '保存' }}
      </Button>
    </template>

    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="wallet-recharge-enabled"
            :checked="form.walletRechargeEnabled"
            @update:checked="form.walletRechargeEnabled = Boolean($event)"
          />
          <div>
            <Label
              for="wallet-recharge-enabled"
              class="cursor-pointer"
            >
              开启前台充值
            </Label>
            <p class="text-xs text-muted-foreground">
              关闭后用户钱包页不显示“发起充值”卡片
            </p>
          </div>
        </div>
      </div>

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="wallet-recharge-alipay-enabled"
            :checked="form.walletRechargeAlipayEnabled"
            @update:checked="form.walletRechargeAlipayEnabled = Boolean($event)"
          />
          <div>
            <Label
              for="wallet-recharge-alipay-enabled"
              class="cursor-pointer"
            >
              开启支付宝
            </Label>
            <p class="text-xs text-muted-foreground">
              关闭后支付方式里不显示支付宝
            </p>
          </div>
        </div>
      </div>

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="wallet-recharge-wechat-enabled"
            :checked="form.walletRechargeWechatEnabled"
            @update:checked="form.walletRechargeWechatEnabled = Boolean($event)"
          />
          <div>
            <Label
              for="wallet-recharge-wechat-enabled"
              class="cursor-pointer"
            >
              开启微信支付
            </Label>
            <p class="text-xs text-muted-foreground">
              关闭后支付方式里不显示微信支付
            </p>
          </div>
        </div>
      </div>

      <div class="rounded-xl border border-border/60 bg-muted/20 p-3 text-xs text-muted-foreground">
        当全局充值关闭，或所有充值通道都关闭时，用户侧不会显示发起充值入口。
      </div>

      <div>
        <Label
          for="wallet-recharge-min-amount"
          class="block text-sm font-medium"
        >
          单笔最小充值金额 (CNY)
        </Label>
        <Input
          id="wallet-recharge-min-amount"
          :model-value="form.walletRechargeMinAmount"
          type="number"
          min="0.01"
          step="0.01"
          class="mt-1"
          @update:model-value="form.walletRechargeMinAmount = Number($event)"
        />
      </div>

      <div>
        <Label
          for="wallet-recharge-max-amount"
          class="block text-sm font-medium"
        >
          单笔最大充值金额 (CNY)
        </Label>
        <Input
          id="wallet-recharge-max-amount"
          :model-value="form.walletRechargeMaxAmount"
          type="number"
          min="0.01"
          step="0.01"
          class="mt-1"
          @update:model-value="form.walletRechargeMaxAmount = Number($event)"
        />
      </div>

      <div>
        <Label
          for="wallet-recharge-expire-minutes"
          class="block text-sm font-medium"
        >
          订单过期时间 (分钟)
        </Label>
        <Input
          id="wallet-recharge-expire-minutes"
          :model-value="form.walletRechargeExpireMinutes"
          type="number"
          min="1"
          step="1"
          class="mt-1"
          @update:model-value="form.walletRechargeExpireMinutes = Number($event)"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          默认建议 15 分钟
        </p>
      </div>

      <div>
        <Label
          for="wallet-recharge-credit-ratio"
          class="block text-sm font-medium"
        >
          充值到账比例
        </Label>
        <Input
          id="wallet-recharge-credit-ratio"
          :model-value="form.walletRechargeCreditRatio"
          type="number"
          min="0.01"
          step="0.01"
          class="mt-1"
          @update:model-value="form.walletRechargeCreditRatio = Number($event)"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          例如填写 2，则用户支付 1 CNY 到账 2 $余额
        </p>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import { computed, reactive, watch } from 'vue'
import { CardSection } from '@/components/layout'
import Button from '@/components/ui/button.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'

const props = defineProps<{
  walletRechargeEnabled: boolean
  walletRechargeAlipayEnabled: boolean
  walletRechargeWechatEnabled: boolean
  walletRechargeMinAmount: number
  walletRechargeMaxAmount: number
  walletRechargeExpireMinutes: number
  walletRechargeCreditRatio: number
  loading: boolean
}>()

const emit = defineEmits<{
  save: [payload: {
    walletRechargeEnabled: boolean
    walletRechargeAlipayEnabled: boolean
    walletRechargeWechatEnabled: boolean
    walletRechargeMinAmount: number
    walletRechargeMaxAmount: number
    walletRechargeExpireMinutes: number
    walletRechargeCreditRatio: number
  }]
}>()

const form = reactive({
  walletRechargeEnabled: props.walletRechargeEnabled,
  walletRechargeAlipayEnabled: props.walletRechargeAlipayEnabled,
  walletRechargeWechatEnabled: props.walletRechargeWechatEnabled,
  walletRechargeMinAmount: props.walletRechargeMinAmount,
  walletRechargeMaxAmount: props.walletRechargeMaxAmount,
  walletRechargeExpireMinutes: props.walletRechargeExpireMinutes,
  walletRechargeCreditRatio: props.walletRechargeCreditRatio,
})

function snapshotFromProps() {
  return {
    walletRechargeEnabled: Boolean(props.walletRechargeEnabled),
    walletRechargeAlipayEnabled: Boolean(props.walletRechargeAlipayEnabled),
    walletRechargeWechatEnabled: Boolean(props.walletRechargeWechatEnabled),
    walletRechargeMinAmount: Number(props.walletRechargeMinAmount),
    walletRechargeMaxAmount: Number(props.walletRechargeMaxAmount),
    walletRechargeExpireMinutes: Number(props.walletRechargeExpireMinutes),
    walletRechargeCreditRatio: Number(props.walletRechargeCreditRatio),
  }
}

function syncFormFromProps() {
  const snapshot = snapshotFromProps()
  form.walletRechargeEnabled = snapshot.walletRechargeEnabled
  form.walletRechargeAlipayEnabled = snapshot.walletRechargeAlipayEnabled
  form.walletRechargeWechatEnabled = snapshot.walletRechargeWechatEnabled
  form.walletRechargeMinAmount = snapshot.walletRechargeMinAmount
  form.walletRechargeMaxAmount = snapshot.walletRechargeMaxAmount
  form.walletRechargeExpireMinutes = snapshot.walletRechargeExpireMinutes
  form.walletRechargeCreditRatio = snapshot.walletRechargeCreditRatio
}

watch(
  () => snapshotFromProps(),
  () => {
    syncFormFromProps()
  },
  { immediate: true }
)

const hasChanges = computed(() => {
  return JSON.stringify(form) !== JSON.stringify(snapshotFromProps())
})

function handleSave() {
  emit('save', {
    walletRechargeEnabled: Boolean(form.walletRechargeEnabled),
    walletRechargeAlipayEnabled: Boolean(form.walletRechargeAlipayEnabled),
    walletRechargeWechatEnabled: Boolean(form.walletRechargeWechatEnabled),
    walletRechargeMinAmount: Number(form.walletRechargeMinAmount),
    walletRechargeMaxAmount: Number(form.walletRechargeMaxAmount),
    walletRechargeExpireMinutes: Number(form.walletRechargeExpireMinutes),
    walletRechargeCreditRatio: Number(form.walletRechargeCreditRatio),
  })
}
</script>
