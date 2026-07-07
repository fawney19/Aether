<template>
  <div class="px-4 sm:px-6 py-3.5 border-b border-border/60">
    <div class="flex flex-col gap-3 sm:hidden">
      <div class="flex items-center justify-between">
        <h3 class="text-base font-semibold">
          {{ legacyT('用户管理') }}
        </h3>
        <div class="flex items-center gap-2">
          <Button
            v-if="canOperateAdmin"
            variant="ghost"
            size="icon"
            class="h-8 w-8"
            :title="legacyT('分组管理')"
            @click="$emit('openGroups')"
          >
            <FolderKanban class="w-3.5 h-3.5" />
          </Button>
          <Button
            v-if="canOperateAdmin"
            variant="ghost"
            size="icon"
            class="h-8 w-8"
            :title="legacyT('新增用户')"
            @click="$emit('createUser')"
          >
            <Plus class="w-3.5 h-3.5" />
          </Button>
          <RefreshButton
            :loading="loading"
            @click="$emit('refresh')"
          />
        </div>
      </div>

      <UserFilterControls
        :search-query="searchQuery"
        :filter-role="filterRole"
        :filter-group="filterGroup"
        :filter-status="filterStatus"
        :sort-option="sortOption"
        :user-groups="userGroups"
        :role-options="roleOptions"
        :status-options="statusOptions"
        :sort-options="sortOptions"
        mobile
        @update:search-query="$emit('update:searchQuery', $event)"
        @update:filter-role="$emit('update:filterRole', $event)"
        @update:filter-group="$emit('update:filterGroup', $event)"
        @update:filter-status="$emit('update:filterStatus', $event)"
        @update:sort-option="$emit('update:sortOption', $event)"
      />
    </div>

    <div class="hidden sm:flex items-center justify-between gap-4">
      <h3 class="text-base font-semibold">
        {{ legacyT('用户管理') }}
      </h3>

      <div class="flex items-center gap-2">
        <UserFilterControls
          :search-query="searchQuery"
          :filter-role="filterRole"
          :filter-group="filterGroup"
          :filter-status="filterStatus"
          :sort-option="sortOption"
          :user-groups="userGroups"
          :role-options="roleOptions"
          :status-options="statusOptions"
          :sort-options="sortOptions"
          @update:search-query="$emit('update:searchQuery', $event)"
          @update:filter-role="$emit('update:filterRole', $event)"
          @update:filter-group="$emit('update:filterGroup', $event)"
          @update:filter-status="$emit('update:filterStatus', $event)"
          @update:sort-option="$emit('update:sortOption', $event)"
        />

        <div class="h-4 w-px bg-border" />

        <Button
          v-if="canOperateAdmin"
          variant="ghost"
          size="icon"
          class="h-8 w-8"
          :title="legacyT('分组管理')"
          @click="$emit('openGroups')"
        >
          <FolderKanban class="w-3.5 h-3.5" />
        </Button>
        <Button
          v-if="canOperateAdmin"
          variant="ghost"
          size="icon"
          class="h-8 w-8"
          :title="legacyT('新增用户')"
          @click="$emit('createUser')"
        >
          <Plus class="w-3.5 h-3.5" />
        </Button>
        <RefreshButton
          :loading="loading"
          @click="$emit('refresh')"
        />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { FolderKanban, Plus } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import RefreshButton from '@/components/ui/refresh-button.vue'
import type { UserGroup, UserRole } from '@/api/users'
import { useI18n } from '@/i18n'
import UserFilterControls from './UserFilterControls.vue'

type FilterRole = 'all' | UserRole
type FilterStatus = 'all' | 'active' | 'inactive'
type SortOption = 'default' | 'created_at_desc' | 'created_at_asc'

interface FilterOption<TValue extends string = string> {
  value: TValue
  label: string
}

defineProps<{
  searchQuery: string
  filterRole: FilterRole
  filterGroup: string
  filterStatus: FilterStatus
  sortOption: SortOption
  userGroups: UserGroup[]
  roleOptions: FilterOption<FilterRole>[]
  statusOptions: FilterOption<FilterStatus>[]
  sortOptions: FilterOption<SortOption>[]
  loading: boolean
  canOperateAdmin: boolean
}>()

defineEmits<{
  'update:searchQuery': [value: string]
  'update:filterRole': [value: FilterRole]
  'update:filterGroup': [value: string]
  'update:filterStatus': [value: FilterStatus]
  'update:sortOption': [value: SortOption]
  openGroups: []
  createUser: []
  refresh: []
}>()

const { legacyT } = useI18n()
</script>
