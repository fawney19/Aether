<template>
  <div :class="mobile ? 'rounded-xl border border-border/60 bg-muted/40 p-3.5' : 'space-y-1.5'">
    <div :class="mobile ? 'flex items-start justify-between gap-3' : ''">
      <div class="space-y-1">
        <p :class="mobile ? 'text-[11px] text-muted-foreground' : 'flex items-center gap-1 text-[11px] text-muted-foreground'">
          <span>{{ legacyT('总可用：') }}</span>
          <Badge
            v-if="row.isUnlimited"
            variant="secondary"
            class="h-5 px-1.5 py-0 text-[10px] font-medium"
          >
            {{ legacyT('无限额度') }}
          </Badge>
          <span
            v-else
            class="font-semibold tabular-nums"
            :class="[
              mobile ? 'text-base leading-none' : 'text-sm',
              row.isNegativeBalance ? 'text-rose-600' : 'text-foreground',
            ]"
          >
            {{ row.totalBalanceLabel }}
          </span>
        </p>
        <p
          v-if="!row.isUnlimited && row.hasWallet"
          class="text-[11px] text-muted-foreground"
        >
          {{ legacyT('套餐') }} {{ row.packageBalanceLabel }}
          · {{ legacyT('钱包') }} {{ row.walletBalanceLabel }}
        </p>
      </div>
      <div :class="mobile ? 'text-right' : 'flex items-center gap-2 text-[11px] text-muted-foreground flex-wrap'">
        <p :class="mobile ? 'text-[11px] text-muted-foreground' : ''">
          {{ legacyT('已消费：') }}
        </p>
        <p :class="mobile ? 'text-sm font-medium tabular-nums text-foreground' : 'font-medium tabular-nums text-foreground'">
          {{ row.consumedLabel }}
        </p>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import Badge from '@/components/ui/badge.vue'
import { useI18n } from '@/i18n'
import type { UserManagementRow } from './user-management-types'

withDefaults(defineProps<{
  row: UserManagementRow
  mobile?: boolean
}>(), {
  mobile: false,
})

const { legacyT } = useI18n()
</script>
