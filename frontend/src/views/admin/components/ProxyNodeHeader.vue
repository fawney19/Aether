<template>
  <div class="px-4 sm:px-6 py-3.5 border-b border-border/60">
    <div class="flex flex-col gap-3 sm:hidden">
      <div class="flex items-center justify-between">
        <h3 class="text-base font-semibold">
          {{ legacyT('代理节点') }}
        </h3>
        <div class="flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            class="h-7 text-xs"
            @click="$emit('open-distribution')"
          >
            {{ legacyT('均分') }}
          </Button>
          <Button
            size="sm"
            variant="outline"
            class="h-7 text-xs"
            @click="$emit('open-batch-upgrade')"
          >
            {{ legacyT('升级') }}
          </Button>
          <Button
            size="sm"
            class="h-7 text-xs"
            @click="$emit('open-add')"
          >
            <Plus class="w-3 h-3 mr-1" />
            {{ legacyT('添加') }}
          </Button>
          <RefreshButton
            :loading="loading"
            @click="$emit('refresh')"
          />
        </div>
      </div>
      <div class="flex flex-wrap items-center gap-2">
        <div class="relative min-w-0 basis-full">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
          <Input
            v-model="searchValue"
            type="text"
            :placeholder="legacyT('搜索...')"
            class="w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
          />
        </div>
        <div class="min-w-0 flex-1">
          <Select v-model="statusValue">
            <SelectTrigger class="w-full h-8 text-xs border-border/60">
              <SelectValue :placeholder="legacyT('状态')" />
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
      </div>
    </div>

    <div class="hidden sm:flex items-center justify-between gap-4">
      <h3 class="text-base font-semibold">
        {{ legacyT('代理节点') }}
      </h3>
      <div class="flex items-center gap-2">
        <div class="relative">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
          <Input
            v-model="searchValue"
            type="text"
            :placeholder="legacyT('搜索...')"
            class="w-48 pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
          />
        </div>
        <div class="h-4 w-px bg-border" />
        <div class="xl:hidden">
          <Select v-model="statusValue">
            <SelectTrigger class="w-28 h-8 text-xs border-border/60">
              <SelectValue :placeholder="legacyT('全部状态')" />
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
        <div class="h-4 w-px bg-border" />
        <Button
          variant="outline"
          size="sm"
          class="h-8 text-xs"
          @click="$emit('open-distribution')"
        >
          <Shuffle class="w-3.5 h-3.5 mr-1.5" />
          {{ legacyT('号池均分') }}
        </Button>
        <Button
          variant="outline"
          size="sm"
          class="h-8 text-xs"
          @click="$emit('open-batch-upgrade')"
        >
          {{ legacyT('批量升级') }}
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8"
          :title="legacyT('手动添加')"
          @click="$emit('open-add')"
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
import { computed } from 'vue'
import { Plus, Search, Shuffle } from 'lucide-vue-next'
import { Button, Input, RefreshButton, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui'
import { useI18n } from '@/i18n'
import type { ProxyNodeStatusFilterOption } from './proxy-node-types'

const props = defineProps<{
  loading: boolean
  searchQuery: string
  filterStatus: string
  statusOptions: ProxyNodeStatusFilterOption[]
}>()

const emit = defineEmits<{
  'update:searchQuery': [value: string]
  'update:filterStatus': [value: string]
  'open-distribution': []
  'open-batch-upgrade': []
  'open-add': []
  refresh: []
}>()

const { legacyT } = useI18n()

const searchValue = computed({
  get: () => props.searchQuery,
  set: (value: string) => emit('update:searchQuery', value),
})

const statusValue = computed({
  get: () => props.filterStatus,
  set: (value: string) => emit('update:filterStatus', value),
})
</script>
