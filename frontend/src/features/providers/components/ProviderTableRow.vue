<template>
  <TableRow
    class="border-b border-border/30 hover:bg-muted/20 transition-colors cursor-pointer"
    @mousedown="$emit('mousedown', $event)"
    @click="$emit('rowClick', $event, provider.id)"
  >
    <TableCell class="py-3.5">
      <div class="space-y-0.5">
        <div class="flex items-center gap-1.5">
          <span class="text-sm font-medium text-foreground">{{ provider.name }}</span>
          <a
            v-if="provider.website"
            :href="provider.website"
            target="_blank"
            rel="noopener noreferrer"
            class="text-muted-foreground hover:text-primary transition-colors shrink-0"
            :title="provider.website"
            @click.stop
          >
            <ExternalLink class="w-3.5 h-3.5" />
          </a>
        </div>
        <!-- 内联编辑备注 -->
        <div
          v-if="editingDescriptionId === provider.id"
          data-desc-editor
          class="flex items-center gap-1 max-w-[220px]"
          @click.stop
        >
          <input
            v-model="localDescriptionValue"
            v-auto-focus
            class="flex-1 min-w-0 text-xs px-1.5 py-0.5 rounded border border-border bg-background text-foreground focus:outline-none focus:ring-1 focus:ring-primary/50"
            placeholder="输入备注..."
            @keydown="handleDescriptionKeydown"
          >
          <button
            class="shrink-0 p-0.5 rounded hover:bg-muted text-primary"
            title="保存"
            @click="handleSave"
          >
            <Check class="w-3.5 h-3.5" />
          </button>
          <button
            class="shrink-0 p-0.5 rounded hover:bg-muted text-muted-foreground"
            title="取消"
            @click="handleCancel"
          >
            <X class="w-3.5 h-3.5" />
          </button>
        </div>
        <span
          v-else-if="provider.description"
          class="text-xs text-muted-foreground truncate block max-w-[200px] group/desc cursor-pointer hover:text-foreground/70 transition-colors"
          :title="provider.description"
          @click="handleStartEdit"
        >{{ provider.description }} <Pencil class="w-3 h-3 inline-block opacity-0 group-hover/desc:opacity-50 transition-opacity" /></span>
        <span
          v-else
          class="text-xs text-muted-foreground cursor-pointer hover:text-foreground/70 transition-colors"
          @click="handleStartEdit"
        >添加备注</span>
      </div>
    </TableCell>
    <TableCell class="py-3.5">
      <ProviderBalanceCell
        :provider="provider"
        :is-balance-loading="isBalanceLoading"
        :get-provider-balance="getProviderBalance"
        :get-provider-balance-breakdown="getProviderBalanceBreakdown"
        :get-provider-balance-error="getProviderBalanceError"
        :get-provider-checkin="getProviderCheckin"
        :get-provider-cookie-expired="getProviderCookieExpired"
        :get-provider-balance-extra="getProviderBalanceExtra"
        :format-balance-display="formatBalanceDisplay"
        :format-reset-countdown="formatResetCountdown"
        :get-quota-used-color-class="getQuotaUsedColorClass"
      />
    </TableCell>
    <TableCell class="py-3.5 text-center">
      <div class="inline-grid grid-cols-[1.75rem_1.75rem_1.75rem] gap-x-0.5 gap-y-0.5 text-xs text-left">
        <span class="text-muted-foreground/70">端点:</span>
        <span class="font-medium text-foreground/90 tabular-nums text-right">{{ provider.active_endpoints }}</span>
        <span class="text-muted-foreground/50 tabular-nums">/{{ provider.total_endpoints }}</span>

        <span class="text-muted-foreground/70">{{ `${getCredentialLabel(provider)}:` }}</span>
        <span class="font-medium text-foreground/90 tabular-nums text-right">{{ provider.active_keys }}</span>
        <span class="text-muted-foreground/50 tabular-nums">/{{ provider.total_keys }}</span>

        <span class="text-muted-foreground/70">模型:</span>
        <span class="font-medium text-foreground/90 tabular-nums text-right">{{ provider.active_models }}</span>
        <span class="text-muted-foreground/50 tabular-nums">/{{ provider.total_models }}</span>
      </div>
    </TableCell>
    <TableCell class="py-3.5 align-middle">
      <div
        v-if="provider.endpoint_health_details && provider.endpoint_health_details.length > 0"
        class="grid grid-cols-3 gap-x-3 gap-y-2 max-w-[240px]"
      >
        <div
          v-for="endpoint in sortEndpoints(provider.endpoint_health_details)"
          :key="endpoint.api_format"
          class="flex flex-col gap-1.5"
          :title="getEndpointTooltip(endpoint)"
        >
          <!-- 上排：缩写 + 百分比 -->
          <div class="flex items-center justify-between text-[10px] leading-none">
            <span class="font-medium text-muted-foreground/80">
              {{ formatApiFormatShort(endpoint.api_format) }}
            </span>
            <span class="font-medium text-muted-foreground/80">
              {{ isEndpointAvailable(endpoint) ? `${(endpoint.health_score * 100).toFixed(0)}%` : '-' }}
            </span>
          </div>

          <!-- 下排：进度条 -->
          <div class="h-1.5 w-full bg-border dark:bg-border/80 rounded-full overflow-hidden">
            <div
              class="h-full rounded-full transition-all duration-300"
              :class="getEndpointDotColor(endpoint)"
              :style="{ width: isEndpointAvailable(endpoint) ? `${Math.max(endpoint.health_score * 100, 5)}%` : '100%' }"
            />
          </div>
        </div>
      </div>
      <span
        v-else
        class="text-xs text-muted-foreground/50"
      >暂无端点</span>
    </TableCell>
    <TableCell class="py-3.5 text-center">
      <Badge
        :variant="provider.is_active ? 'success' : 'secondary'"
        class="text-xs"
      >
        {{ provider.is_active ? '活跃' : '停用' }}
      </Badge>
    </TableCell>
    <TableCell
      class="py-3.5"
      @click.stop
    >
      <div class="flex items-center justify-center gap-0.5">
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 text-muted-foreground/70 hover:text-foreground"
          title="查看详情"
          @click="$emit('viewDetail', provider.id)"
        >
          <Eye class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 text-muted-foreground/70 hover:text-foreground"
          title="编辑提供商"
          @click="$emit('editProvider', provider)"
        >
          <Edit class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 text-muted-foreground/70 hover:text-foreground"
          title="扩展操作配置"
          @click="$emit('openOpsConfig', provider)"
        >
          <KeyRound class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 text-muted-foreground/70 hover:text-foreground"
          :title="provider.is_active ? '停用提供商' : '启用提供商'"
          @click="$emit('toggleStatus', provider)"
        >
          <Power class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7 text-muted-foreground/70 hover:text-destructive"
          title="删除提供商"
          @click="$emit('deleteProvider', provider)"
        >
          <Trash2 class="h-3.5 w-3.5" />
        </Button>
      </div>
    </TableCell>
  </TableRow>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import {
  Edit,
  Eye,
  Trash2,
  Power,
  KeyRound,
  ExternalLink,
  Pencil,
  Check,
  X,
} from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import TableRow from '@/components/ui/table-row.vue'
import TableCell from '@/components/ui/table-cell.vue'
import ProviderBalanceCell from './ProviderBalanceCell.vue'
import { type ProviderWithEndpointsSummary, formatApiFormatShort } from '@/api/endpoints'
import { sortEndpoints, isEndpointAvailable, getEndpointDotColor, getEndpointTooltip } from '@/features/providers/composables/useEndpointStatus'
import type { BalanceExtraItem } from '@/features/providers/auth-templates'

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
  editingDescriptionId: string | null
  // Balance functions
  isBalanceLoading: (providerId: string) => boolean
  getProviderBalance: (providerId: string) => { available: number | null; currency: string } | null
  getProviderBalanceBreakdown: (providerId: string) => { balance: number; points: number; currency: string } | null
  getProviderBalanceError: (providerId: string) => { status: string; message: string } | null
  getProviderCheckin: (providerId: string) => { success: boolean | null; message: string } | null
  getProviderCookieExpired: (providerId: string) => { expired: boolean; message: string } | null
  getProviderBalanceExtra: (providerId: string, architectureId?: string) => BalanceExtraItem[]
  formatBalanceDisplay: (balance: { available: number | null; currency: string } | null) => string
  formatResetCountdown: (resetsAt: number) => string
  getQuotaUsedColorClass: (provider: ProviderWithEndpointsSummary) => string
}>()

const emit = defineEmits<{
  'mousedown': [event: MouseEvent]
  'rowClick': [event: MouseEvent, providerId: string]
  'viewDetail': [providerId: string]
  'editProvider': [provider: ProviderWithEndpointsSummary]
  'openOpsConfig': [provider: ProviderWithEndpointsSummary]
  'toggleStatus': [provider: ProviderWithEndpointsSummary]
  'deleteProvider': [provider: ProviderWithEndpointsSummary]
  'startEditDescription': [event: Event, provider: ProviderWithEndpointsSummary]
  'saveDescription': [event: Event, provider: ProviderWithEndpointsSummary, value: string]
  'cancelEditDescription': [event?: Event]
}>()

const vAutoFocus = {
  mounted: (el: HTMLElement) => el.focus(),
}

const localDescriptionValue = ref('')

// 当进入编辑模式时，同步 props 的 description
watch(
  () => props.editingDescriptionId,
  (newId) => {
    if (newId === props.provider.id) {
      localDescriptionValue.value = props.provider.description || ''
    }
  },
)

function handleStartEdit(event: Event) {
  event.stopPropagation()
  emit('startEditDescription', event, props.provider)
}

function handleSave(event: Event) {
  event.stopPropagation()
  emit('saveDescription', event, props.provider, localDescriptionValue.value)
}

function handleCancel(event?: Event) {
  event?.stopPropagation()
  emit('cancelEditDescription', event)
}

function handleDescriptionKeydown(event: KeyboardEvent) {
  if (event.key === 'Enter') {
    event.preventDefault()
    handleSave(event)
  } else if (event.key === 'Escape') {
    handleCancel(event)
  }
}

function getCredentialLabel(provider: ProviderWithEndpointsSummary): '账号' | '密钥' {
  const providerType = String(provider.provider_type || '').trim().toLowerCase()
  return providerType && providerType !== 'custom' ? '账号' : '密钥'
}
</script>
