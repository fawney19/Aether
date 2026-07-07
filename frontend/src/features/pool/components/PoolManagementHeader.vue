<template>
  <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
    <div class="flex flex-col gap-3 xl:hidden">
      <div class="min-w-0">
        <h3 class="text-base font-semibold">
          {{ legacyT('号池管理') }}
        </h3>
        <p
          v-if="metaText"
          class="mt-1 text-xs text-muted-foreground"
        >
          {{ metaText }}
        </p>
      </div>

      <div class="grid grid-cols-3 items-center gap-2">
        <Select
          v-model="providerModel"
          :disabled="providerSelectDisabled"
        >
          <SelectTrigger
            class="h-9 text-xs border-border/60"
            :disabled="providerSelectDisabled"
          >
            <SelectValue :placeholder="legacyT('选择 Provider')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="item in providers"
              :key="item.provider_id"
              :value="item.provider_id"
            >
              {{ item.provider_name }}
              <span class="text-muted-foreground ml-1">({{ item.total_keys }})</span>
              <span
                v-if="!item.pool_enabled"
                class="ml-1 text-[10px] text-amber-600"
              >{{ legacyT('未启用') }}</span>
            </SelectItem>
          </SelectContent>
        </Select>

        <Select v-model="statusModel">
          <SelectTrigger class="h-9 w-full text-xs border-border/60">
            <SelectValue :placeholder="legacyT('状态')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="item in statusOptions"
              :key="`mobile-${item.value}`"
              :value="item.value"
            >
              {{ legacyT(item.label) }}
            </SelectItem>
          </SelectContent>
        </Select>

        <div class="relative min-w-0">
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
          <Input
            v-model="searchModel"
            type="text"
            :placeholder="legacyT('搜索账号...')"
            class="w-full pl-8 pr-3 h-9 text-sm bg-background/50 border-border/60"
          />
        </div>
      </div>

      <div
        v-if="hasSelectedProvider"
        class="flex items-center gap-1"
      >
        <div
          v-for="action in mobileActions"
          :key="action.key"
          class="min-w-0 flex-1 flex justify-center"
        >
          <Button
            v-if="action.key !== 'providerProxy' && action.key !== 'refresh'"
            variant="ghost"
            size="icon"
            class="h-8 w-8 shrink-0"
            :class="action.key === 'toggleProvider' ? providerToggleButtonClass : ''"
            :disabled="action.key === 'toggleProvider' ? togglingProviderStatus : false"
            :title="action.title"
            @click="emit(action.event)"
          >
            <component
              :is="action.icon"
              class="w-3.5 h-3.5"
            />
          </Button>

          <ProviderProxyPopover
            v-else-if="action.key === 'providerProxy'"
            :open="providerProxyMobileOpen"
            :node-id="providerProxyNodeId"
            :saving="savingProviderProxy"
            :title="providerProxyButtonTitle"
            @update:open="emit('update:providerProxyMobileOpen', $event)"
            @select="emit('selectProviderProxy', $event)"
            @clear="emit('clearProviderProxy')"
          />

          <RefreshButton
            v-else
            :loading="refreshLoading"
            :title="refreshTitle"
            @click="emit('refresh')"
          />
        </div>
      </div>
    </div>

    <div class="hidden xl:flex items-center justify-between gap-4">
      <div class="flex items-center gap-2">
        <h3 class="text-base font-semibold">
          {{ legacyT('号池管理') }}
          <span
            v-if="metaText"
            class="ml-2 text-xs font-normal text-muted-foreground"
          >
            | {{ metaText }}
          </span>
        </h3>
      </div>

      <div
        class="flex items-center gap-2"
        data-testid="pool-header-actions"
      >
        <Select
          v-model="providerModel"
          :disabled="providerSelectDisabled"
        >
          <SelectTrigger
            class="w-36 h-8 text-xs border-border/60"
            :disabled="providerSelectDisabled"
          >
            <SelectValue :placeholder="legacyT('选择 Provider')" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem
              v-for="item in providers"
              :key="item.provider_id"
              :value="item.provider_id"
            >
              {{ item.provider_name }}
              <span class="text-muted-foreground ml-1">({{ item.total_keys }})</span>
              <span
                v-if="!item.pool_enabled"
                class="ml-1 text-[10px] text-amber-600"
              >{{ legacyT('未启用') }}</span>
            </SelectItem>
          </SelectContent>
        </Select>

        <div class="h-4 w-px bg-border" />

        <div
          v-if="hasSelectedProvider"
          class="relative"
        >
          <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
          <Input
            v-model="searchModel"
            type="text"
            :placeholder="legacyT('搜索账号...')"
            class="w-40 pl-8 pr-2 h-8 text-xs bg-background/50 border-border/60"
          />
        </div>

        <div
          v-if="hasSelectedProvider"
          class="h-4 w-px bg-border"
        />

        <button
          v-if="hasSelectedProvider"
          class="group inline-flex items-center gap-1.5 px-2.5 h-8 rounded-md border border-border/50 bg-muted/20 hover:bg-muted/40 hover:border-primary/40 transition-all duration-200 text-xs"
          type="button"
          :title="legacyT('点击调整号池调度')"
          @click="emit('scheduling')"
        >
          <span class="text-muted-foreground/80 hidden lg:inline">{{ legacyT('调度:') }}</span>
          <span class="font-medium text-foreground/90">{{ poolSchedulingLabel }}</span>
          <ChevronDown class="w-3 h-3 text-muted-foreground/70 group-hover:text-foreground transition-colors" />
        </button>

        <div
          v-if="hasSelectedProvider"
          class="h-4 w-px bg-border"
        />

        <Button
          v-if="hasSelectedProvider"
          variant="ghost"
          size="icon"
          class="h-8 w-8"
          :title="legacyT('添加账号')"
          @click="emit('import')"
        >
          <Upload class="w-3.5 h-3.5" />
        </Button>

        <ProviderProxyPopover
          v-if="hasSelectedProvider"
          :open="providerProxyDesktopOpen"
          :node-id="providerProxyNodeId"
          :saving="savingProviderProxy"
          :title="providerProxyButtonTitle"
          @update:open="emit('update:providerProxyDesktopOpen', $event)"
          @select="emit('selectProviderProxy', $event)"
          @clear="emit('clearProviderProxy')"
        />

        <Button
          v-for="action in desktopPostProxyActions"
          v-show="hasSelectedProvider"
          :key="action.key"
          variant="ghost"
          size="icon"
          class="h-8 w-8"
          :class="action.key === 'toggleProvider' ? providerToggleButtonClass : ''"
          :disabled="action.key === 'toggleProvider' ? togglingProviderStatus : false"
          :data-testid="action.key === 'demandMetrics' ? 'pool-demand-metrics-button' : undefined"
          :title="action.title"
          @click="emit(action.event)"
        >
          <component
            :is="action.icon"
            class="w-3.5 h-3.5"
          />
        </Button>

        <RefreshButton
          :loading="refreshLoading"
          :title="refreshTitle"
          @click="emit('refresh')"
        />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import {
  Activity,
  ChevronDown,
  Edit,
  Plug,
  Power,
  Search,
  Settings2,
  SlidersHorizontal,
  Upload,
  Users,
} from 'lucide-vue-next'
import {
  Button,
  Input,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import RefreshButton from '@/components/ui/refresh-button.vue'
import ProviderProxyPopover from '@/features/pool/components/ProviderProxyPopover.vue'
import { useI18n } from '@/i18n'
import type { PoolOverviewItem } from '@/api/endpoints/pool'

type HeaderActionEvent =
  | 'import'
  | 'scheduling'
  | 'accountBatch'
  | 'editProvider'
  | 'editEndpoint'
  | 'demandMetrics'
  | 'advanced'
  | 'toggleProvider'

type HeaderActionKey =
  | 'import'
  | 'providerProxy'
  | 'scheduling'
  | 'accountBatch'
  | 'editProvider'
  | 'editEndpoint'
  | 'demandMetrics'
  | 'advanced'
  | 'toggleProvider'
  | 'refresh'

interface HeaderAction {
  key: HeaderActionKey
  title: string
  event: HeaderActionEvent
  icon: unknown
}

const props = withDefaults(defineProps<{
  providers: PoolOverviewItem[]
  providerId: string
  providerSelectDisabled: boolean
  status: string
  statusOptions: Array<{ value: string, label: string }>
  search: string
  metaText?: string
  providerProxyNodeId?: string | null
  providerProxyMobileOpen: boolean
  providerProxyDesktopOpen: boolean
  providerProxyButtonTitle: string
  savingProviderProxy: boolean
  poolSchedulingLabel: string
  showAdaptiveHotPoolMetricsButton: boolean
  providerToggleButtonTitle: string
  providerToggleButtonClass?: string
  togglingProviderStatus: boolean
  refreshLoading: boolean
  refreshTitle: string
}>(), {
  metaText: '',
  providerProxyNodeId: null,
  providerToggleButtonClass: '',
})

const emit = defineEmits<{
  'update:providerId': [value: string]
  'update:status': [value: string]
  'update:search': [value: string]
  'update:providerProxyMobileOpen': [value: boolean]
  'update:providerProxyDesktopOpen': [value: boolean]
  import: []
  scheduling: []
  accountBatch: []
  editProvider: []
  editEndpoint: []
  demandMetrics: []
  advanced: []
  toggleProvider: []
  refresh: []
  selectProviderProxy: [nodeId: string]
  clearProviderProxy: []
}>()

const { legacyT } = useI18n()

const providerModel = computed({
  get: () => props.providerId,
  set: value => emit('update:providerId', value),
})

const statusModel = computed({
  get: () => props.status,
  set: value => emit('update:status', value),
})

const searchModel = computed({
  get: () => props.search,
  set: value => emit('update:search', value),
})

const hasSelectedProvider = computed(() => Boolean(props.providerId))

const mobileActions = computed(() => {
  const actions: Array<HeaderAction | { key: 'providerProxy' | 'refresh' }> = [
    { key: 'import', title: legacyT('添加账号'), event: 'import', icon: Upload },
    { key: 'providerProxy' },
    { key: 'scheduling', title: legacyT('号池调度'), event: 'scheduling', icon: SlidersHorizontal },
    { key: 'accountBatch', title: legacyT('账号批量操作'), event: 'accountBatch', icon: Users },
    { key: 'editProvider', title: legacyT('编辑提供商'), event: 'editProvider', icon: Edit },
    { key: 'editEndpoint', title: legacyT('编辑端点'), event: 'editEndpoint', icon: Plug },
  ]
  if (props.showAdaptiveHotPoolMetricsButton) {
    actions.push({ key: 'demandMetrics', title: legacyT('查看自适应热池指标'), event: 'demandMetrics', icon: Activity })
  }
  actions.push(
    { key: 'advanced', title: legacyT('高级设置'), event: 'advanced', icon: Settings2 },
    { key: 'toggleProvider', title: props.providerToggleButtonTitle, event: 'toggleProvider', icon: Power },
    { key: 'refresh' },
  )
  return actions
})

const desktopPostProxyActions = computed<HeaderAction[]>(() => {
  const actions: HeaderAction[] = [
    { key: 'editProvider', title: legacyT('编辑提供商'), event: 'editProvider', icon: Edit },
    { key: 'editEndpoint', title: legacyT('编辑端点'), event: 'editEndpoint', icon: Plug },
  ]
  if (props.showAdaptiveHotPoolMetricsButton) {
    actions.push({ key: 'demandMetrics', title: legacyT('查看自适应热池指标'), event: 'demandMetrics', icon: Activity })
  }
  actions.push(
    { key: 'advanced', title: legacyT('高级设置'), event: 'advanced', icon: Settings2 },
    { key: 'accountBatch', title: legacyT('账号'), event: 'accountBatch', icon: Users },
    { key: 'toggleProvider', title: props.providerToggleButtonTitle, event: 'toggleProvider', icon: Power },
  )
  return actions
})
</script>
