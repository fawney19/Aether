<template>
  <div :class="mobile ? 'flex flex-wrap items-center gap-2' : 'flex items-center gap-2'">
    <div :class="mobile ? 'relative min-w-40 flex-1' : 'relative'">
      <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
      <Input
        :id="mobile ? 'users-search-mobile' : 'users-search'"
        :model-value="searchQuery"
        type="text"
        :placeholder="searchPlaceholder"
        :class="mobile ? 'w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60' : 'w-48 pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60 focus:border-primary/40 transition-colors'"
        @update:model-value="$emit('update:searchQuery', $event)"
      />
    </div>

    <div class="hidden sm:block h-4 w-px bg-border" />

    <div class="xl:hidden">
      <Select
        :model-value="filterRole"
        @update:model-value="$emit('update:filterRole', $event)"
      >
        <SelectTrigger :class="mobile ? 'w-24 h-8 text-xs border-border/60' : 'w-32 h-8 text-xs border-border/60'">
          <SelectValue :placeholder="legacyT(mobile ? '角色' : '全部角色')" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="option in roleOptions"
            :key="option.value"
            :value="option.value"
          >
            {{ legacyT(option.label) }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <div class="xl:hidden">
      <Select
        :model-value="filterStatus"
        @update:model-value="$emit('update:filterStatus', $event)"
      >
        <SelectTrigger :class="mobile ? 'w-20 h-8 text-xs border-border/60' : 'w-28 h-8 text-xs border-border/60'">
          <SelectValue :placeholder="legacyT(mobile ? '状态' : '全部状态')" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="option in statusOptions"
            :key="option.value"
            :value="option.value"
          >
            {{ legacyT(option.label) }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <Select
      :model-value="filterGroup"
      @update:model-value="$emit('update:filterGroup', $event)"
    >
      <SelectTrigger :class="mobile ? 'w-24 h-8 text-xs border-border/60' : 'w-32 h-8 text-xs border-border/60'">
        <SelectValue :placeholder="legacyT(mobile ? '分组' : '全部分组')" />
      </SelectTrigger>
      <SelectContent>
        <SelectItem value="all">
          {{ legacyT(mobile ? '全部' : '全部分组') }}
        </SelectItem>
        <SelectItem
          v-for="group in userGroups"
          :key="group.id"
          :value="group.id"
        >
          {{ group.name }}
        </SelectItem>
      </SelectContent>
    </Select>

    <div class="xl:hidden">
      <Select
        :model-value="sortOption"
        @update:model-value="$emit('update:sortOption', $event)"
      >
        <SelectTrigger :class="mobile ? 'w-32 h-8 text-xs border-border/60' : 'w-40 h-8 text-xs border-border/60'">
          <SelectValue :placeholder="legacyT('排序')" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="option in sortOptions"
            :key="option.value"
            :value="option.value"
          >
            {{ legacyT(option.label) }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Search } from 'lucide-vue-next'
import Input from '@/components/ui/input.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import type { UserGroup, UserRole } from '@/api/users'
import { useI18n } from '@/i18n'

type FilterRole = 'all' | UserRole
type FilterStatus = 'all' | 'active' | 'inactive'
type SortOption = 'default' | 'created_at_desc' | 'created_at_asc'

interface FilterOption<TValue extends string = string> {
  value: TValue
  label: string
}

const props = withDefaults(defineProps<{
  searchQuery: string
  filterRole: FilterRole
  filterGroup: string
  filterStatus: FilterStatus
  sortOption: SortOption
  userGroups: UserGroup[]
  roleOptions: FilterOption<FilterRole>[]
  statusOptions: FilterOption<FilterStatus>[]
  sortOptions: FilterOption<SortOption>[]
  mobile?: boolean
}>(), {
  mobile: false,
})

defineEmits<{
  'update:searchQuery': [value: string]
  'update:filterRole': [value: FilterRole]
  'update:filterGroup': [value: string]
  'update:filterStatus': [value: FilterStatus]
  'update:sortOption': [value: SortOption]
}>()

const { legacyT } = useI18n()
const searchPlaceholder = computed(() => props.mobile ? legacyT('搜索...') : legacyT('搜索用户名或邮箱...'))
</script>
