<template>
  <div class="space-y-0">
    <div class="hidden xl:block overflow-x-auto">
      <Table>
        <TableHeader>
          <TableRow class="border-b border-border/60 hover:bg-transparent">
            <TableHead class="w-[44px] h-12 px-4">
              <Checkbox
                :checked="isCurrentPageFullySelected || isAllFilteredSelected"
                :indeterminate="isPartiallyFilteredSelected && !isCurrentPageFullySelected"
                :disabled="isHeaderCheckboxDisabled"
                @update:checked="handleToggleCurrentPage"
              />
            </TableHead>
            <TableHead class="w-[260px] h-12 font-semibold">
              {{ legacyT('用户信息') }}
            </TableHead>
            <TableHead class="w-[240px] h-12 font-semibold">
              {{ legacyT('钱包') }}
            </TableHead>
            <TableHead class="w-[170px] h-12 font-semibold">
              {{ legacyT('统计/限速') }}
            </TableHead>
            <SortableTableHead
              class="w-[110px] h-12 font-semibold"
              column-key="created_at"
              :active-key="sortBy"
              :direction="sortOrder"
              default-direction="desc"
              :title="legacyT('按创建时间排序')"
              @sort="handleSort"
            >
              {{ legacyT('创建时间') }}
            </SortableTableHead>
            <TableHead class="w-[180px] h-12 font-semibold">
              {{ legacyT('状态') }}
            </TableHead>
            <TableHead class="w-[260px] h-12 font-semibold text-center">
              {{ legacyT('操作') }}
            </TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          <UserTableRow
            v-for="row in rows"
            :key="row.user.id"
            :row="row"
            :selected="selectAllFiltered || selectedIdSet.has(row.user.id)"
            :selection-disabled="selectionDisabled"
            :can-operate-admin="canOperateAdmin"
            @toggle-selected="(checked) => emit('toggle-selected', row.user.id, checked)"
            @edit="emit('edit', row.user)"
            @wallet="emit('wallet', row.user)"
            @plans="emit('plans', row.user)"
            @api-keys="emit('api-keys', row.user)"
            @sessions="emit('sessions', row.user)"
            @toggle-status="emit('toggle-status', row.user)"
            @delete="emit('delete', row.user)"
          />
        </TableBody>
      </Table>
    </div>

    <div class="xl:hidden bg-muted/[0.14] p-3 sm:p-4">
      <UserMobileEmptyState
        v-if="rows.length === 0"
        :has-filters="hasFilters"
      />

      <div
        v-else
        class="space-y-3.5"
      >
        <UserMobileCard
          v-for="row in rows"
          :key="row.user.id"
          :row="row"
          :selected="selectAllFiltered || selectedIdSet.has(row.user.id)"
          :selection-disabled="selectionDisabled"
          :can-operate-admin="canOperateAdmin"
          @toggle-selected="(checked) => emit('toggle-selected', row.user.id, checked)"
          @edit="emit('edit', row.user)"
          @wallet="emit('wallet', row.user)"
          @plans="emit('plans', row.user)"
          @api-keys="emit('api-keys', row.user)"
          @sessions="emit('sessions', row.user)"
          @toggle-status="emit('toggle-status', row.user)"
          @delete="emit('delete', row.user)"
        />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Table from '@/components/ui/table.vue'
import TableHeader from '@/components/ui/table-header.vue'
import TableBody from '@/components/ui/table-body.vue'
import TableHead from '@/components/ui/table-head.vue'
import TableRow from '@/components/ui/table-row.vue'
import SortableTableHead from '@/components/ui/sortable-table-head.vue'
import { useI18n } from '@/i18n'
import type { AdminUserSortBy, AdminUserSortOrder } from '@/api/users'
import UserMobileCard from './UserMobileCard.vue'
import UserMobileEmptyState from './UserMobileEmptyState.vue'
import UserTableRow from './UserTableRow.vue'
import type { UserManagementRow } from './user-management-types'

const props = defineProps<{
  rows: UserManagementRow[]
  selectedIdSet: Set<string>
  selectAllFiltered: boolean
  isAllFilteredSelected: boolean
  isPartiallyFilteredSelected: boolean
  isCurrentPageFullySelected: boolean
  selectionDisabled: boolean
  loading: boolean
  canOperateAdmin: boolean
  hasFilters: boolean
  sortBy: AdminUserSortBy | null
  sortOrder: AdminUserSortOrder
}>()

const emit = defineEmits<{
  'toggle-selected': [userId: string, checked: boolean]
  'toggle-select-current-page': []
  edit: [user: UserManagementRow['user']]
  wallet: [user: UserManagementRow['user']]
  plans: [user: UserManagementRow['user']]
  'api-keys': [user: UserManagementRow['user']]
  sessions: [user: UserManagementRow['user']]
  'toggle-status': [user: UserManagementRow['user']]
  delete: [user: UserManagementRow['user']]
  sort: [payload: { key: string; direction: AdminUserSortOrder }]
}>()

const { legacyT } = useI18n()

const isHeaderCheckboxDisabled = computed(() => props.rows.length === 0 || props.selectAllFiltered || props.loading)

function handleToggleCurrentPage(): void {
  emit('toggle-select-current-page')
}

function handleSort(payload: { key: string; direction: AdminUserSortOrder }): void {
  emit('sort', payload)
}
</script>
