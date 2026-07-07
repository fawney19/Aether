<template>
  <TableRow class="border-b border-border/40 transition-colors hover:bg-muted/30">
    <TableCell class="w-[44px] px-4 py-4">
      <Checkbox
        :checked="selected"
        :disabled="selectionDisabled"
        @update:checked="(checked) => $emit('toggle-selected', checked === true)"
      />
    </TableCell>
    <TableCell class="py-4">
      <UserIdentityCell :row="row" />
    </TableCell>
    <TableCell class="py-4">
      <UserWalletSummary :row="row" />
    </TableCell>
    <TableCell class="py-4">
      <div class="space-y-1 text-xs">
        <div class="flex items-center text-muted-foreground">
          <span class="w-14">{{ legacyT('请求:') }}</span>
          <span class="font-medium text-foreground">{{ row.requestCountLabel }}</span>
        </div>
        <div class="flex items-center text-muted-foreground">
          <span class="w-14">Tokens:</span>
          <span class="font-medium text-foreground">{{ row.tokensLabel }}</span>
        </div>
        <div class="flex items-center text-muted-foreground">
          <span class="w-14">{{ legacyT('限速:') }}</span>
          <Badge
            v-if="row.rateLimitAsBadge"
            variant="secondary"
            class="h-5 px-1.5 py-0 text-[10px] font-medium"
          >
            {{ legacyT(row.rateLimitLabel) }}
          </Badge>
          <span
            v-else
            class="font-medium text-foreground"
          >
            {{ legacyT(row.rateLimitLabel) }}
          </span>
        </div>
      </div>
    </TableCell>
    <TableCell class="py-4 text-xs text-muted-foreground">
      {{ row.createdAtLabel }}
    </TableCell>
    <TableCell class="py-4">
      <UserStatusBadges :row="row" />
    </TableCell>
    <TableCell class="py-4">
      <UserActionButtons
        :can-operate-admin="canOperateAdmin"
        :is-active="row.user.is_active"
        @edit="$emit('edit')"
        @wallet="$emit('wallet')"
        @plans="$emit('plans')"
        @api-keys="$emit('api-keys')"
        @sessions="$emit('sessions')"
        @toggle-status="$emit('toggle-status')"
        @delete="$emit('delete')"
      />
    </TableCell>
  </TableRow>
</template>

<script setup lang="ts">
import Badge from '@/components/ui/badge.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import TableCell from '@/components/ui/table-cell.vue'
import TableRow from '@/components/ui/table-row.vue'
import { useI18n } from '@/i18n'
import UserActionButtons from './UserActionButtons.vue'
import UserIdentityCell from './UserIdentityCell.vue'
import UserStatusBadges from './UserStatusBadges.vue'
import UserWalletSummary from './UserWalletSummary.vue'
import type { UserManagementRow } from './user-management-types'

defineProps<{
  row: UserManagementRow
  selected: boolean
  selectionDisabled: boolean
  canOperateAdmin: boolean
}>()

defineEmits<{
  'toggle-selected': [checked: boolean]
  edit: []
  wallet: []
  plans: []
  'api-keys': []
  sessions: []
  'toggle-status': []
  delete: []
}>()

const { legacyT } = useI18n()
</script>
