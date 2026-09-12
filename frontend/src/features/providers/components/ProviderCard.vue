<template>
  <Card
    variant="interactive"
    class="flex max-h-96 w-full min-w-0 flex-col cursor-pointer overflow-hidden"
    @mousedown="$emit('mousedown', $event)"
    @click="$emit('rowClick', $event, provider.id)"
  >
    <div class="flex shrink-0 items-start gap-2 p-4 pb-3">
      <slot name="drag-handle" />
      <div
        class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl text-base font-semibold"
        :class="provider.is_active ? 'bg-primary/10 text-primary' : 'bg-muted text-muted-foreground'"
        aria-hidden="true"
      >
        {{ provider.name.slice(0, 1).toUpperCase() }}
      </div>
      <div class="min-w-0 flex-1 space-y-1">
        <div class="flex items-center gap-1.5">
          <button
            type="button"
            class="min-w-0 truncate rounded text-left text-sm font-semibold text-foreground hover:text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            :title="provider.name"
            @click.stop="$emit('viewDetail', provider.id)"
          >
            {{ provider.name }}
          </button>
          <a
            v-if="safeProviderWebsite"
            :href="safeProviderWebsite"
            target="_blank"
            rel="noopener noreferrer"
            class="shrink-0 text-muted-foreground transition-colors hover:text-primary"
            :title="safeProviderWebsite"
            @click.stop
          >
            <ExternalLink class="h-3.5 w-3.5" />
          </a>
        </div>
        <div
          v-if="editingDescriptionId === provider.id"
          data-desc-editor
          class="flex items-center gap-1"
          @click.stop
        >
          <input
            v-model="localDescriptionValue"
            v-auto-focus
            class="min-w-0 flex-1 rounded border border-border bg-background px-1.5 py-0.5 text-xs text-foreground focus:outline-none focus:ring-1 focus:ring-primary/50"
            :placeholder="legacyT('输入备注...')"
            :aria-label="legacyT('输入备注...')"
            @keydown="handleDescriptionKeydown"
          >
          <button
            type="button"
            class="shrink-0 rounded p-0.5 text-primary hover:bg-muted"
            :title="legacyT('保存')"
            :aria-label="legacyT('保存')"
            @click="handleSave"
          >
            <Check class="h-3.5 w-3.5" />
          </button>
          <button
            type="button"
            class="shrink-0 rounded p-0.5 text-muted-foreground hover:bg-muted"
            :title="legacyT('取消')"
            :aria-label="legacyT('取消')"
            @click="handleCancel"
          >
            <X class="h-3.5 w-3.5" />
          </button>
        </div>
        <button
          v-else
          type="button"
          class="group/desc flex max-w-full items-center gap-1 rounded text-xs text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          :title="provider.description || legacyT('添加备注')"
          @click="handleStartEdit"
        >
          <span class="truncate">{{ provider.description || legacyT('添加备注') }}</span>
          <Pencil class="h-3 w-3 shrink-0 opacity-0 transition-opacity group-hover/desc:opacity-60 group-focus-visible/desc:opacity-60" />
        </button>
      </div>
      <Badge
        :variant="provider.is_active ? 'success' : 'secondary'"
        class="shrink-0 text-xs"
      >
        {{ legacyT(provider.is_active ? '活跃' : '停用') }}
      </Badge>
    </div>

    <div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto overscroll-contain break-words px-4 pb-4">
      <div class="space-y-2 rounded-xl border border-border/40 bg-muted/20 p-3">
        <div class="flex flex-wrap items-center justify-between gap-2">
          <span class="text-xs text-muted-foreground">{{ legacyT('余额监控') }}</span>
          <Badge
            variant="outline"
            class="border-border/50 text-[10px] font-normal"
          >
            {{ formatBillingType(provider.billing_type || 'pay_as_you_go') }}
          </Badge>
        </div>
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
      </div>

      <dl class="grid grid-cols-3 divide-x divide-border/50 text-center">
        <div class="min-w-0 space-y-1 px-1">
          <dt class="text-xs text-muted-foreground">
            {{ legacyT('端点') }}
          </dt>
          <dd class="text-sm font-semibold tabular-nums">
            {{ provider.active_endpoints }}<span class="ml-0.5 text-xs font-normal text-muted-foreground">/ {{ provider.total_endpoints }}</span>
          </dd>
        </div>
        <div class="min-w-0 space-y-1 px-1">
          <dt class="text-xs text-muted-foreground">
            {{ legacyT(isKeyManagedProviderType(provider.provider_type) ? '密钥' : '账号') }}
          </dt>
          <dd class="text-sm font-semibold tabular-nums">
            {{ provider.active_keys }}<span class="ml-0.5 text-xs font-normal text-muted-foreground">/ {{ provider.total_keys }}</span>
          </dd>
        </div>
        <div class="min-w-0 space-y-1 px-1">
          <dt class="text-xs text-muted-foreground">
            {{ legacyT('模型') }}
          </dt>
          <dd class="text-sm font-semibold tabular-nums">
            {{ provider.active_models }}<span class="ml-0.5 text-xs font-normal text-muted-foreground">/ {{ provider.total_models }}</span>
          </dd>
        </div>
      </dl>

      <div class="mt-auto space-y-2 border-t border-border/40 pt-3">
        <div class="text-xs text-muted-foreground">
          {{ legacyT('端点健康') }}
        </div>
        <div
          v-if="provider.endpoint_health_details?.length"
          class="grid grid-cols-3 gap-x-3 gap-y-2"
        >
          <div
            v-for="endpoint in sortEndpoints(provider.endpoint_health_details)"
            :key="endpoint.api_format"
            class="flex min-w-0 flex-col gap-1.5"
            :title="getEndpointTooltip(endpoint, locale)"
          >
            <div class="flex items-center justify-between gap-1 text-[10px] leading-none text-muted-foreground">
              <span class="font-medium">{{ formatApiFormatShort(endpoint.api_format) }}</span>
              <span class="tabular-nums">{{ getEndpointHealthLabel(endpoint) }}</span>
            </div>
            <div class="h-1.5 w-full overflow-hidden rounded-full bg-border dark:bg-border/80">
              <div
                class="h-full rounded-full transition-all duration-300"
                :class="getEndpointDotColor(endpoint)"
                :style="{ width: getEndpointHealthBarWidth(endpoint) }"
              />
            </div>
          </div>
        </div>
        <span
          v-else
          class="text-xs text-muted-foreground/60"
        >{{ legacyT('暂无端点') }}</span>
      </div>
    </div>

    <div
      class="flex shrink-0 items-center justify-between gap-2 border-t border-border/40 bg-muted/10 px-3 py-2"
      @click.stop
    >
      <Button
        variant="ghost"
        size="icon"
        class="h-8 w-8 text-muted-foreground hover:text-primary"
        :title="legacyT('查看详情')"
        :aria-label="legacyT('查看详情')"
        @click="$emit('viewDetail', provider.id)"
      >
        <Eye class="h-3.5 w-3.5" />
      </Button>
      <div class="flex shrink-0 items-center gap-0.5">
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8 text-muted-foreground hover:text-foreground"
          :title="legacyT('编辑提供商')"
          :aria-label="legacyT('编辑提供商')"
          @click="$emit('editProvider', provider)"
        >
          <Edit class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8 text-muted-foreground hover:text-foreground"
          :title="legacyT('扩展操作配置')"
          :aria-label="legacyT('扩展操作配置')"
          @click="$emit('openOpsConfig', provider)"
        >
          <KeyRound class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8 text-muted-foreground hover:text-foreground"
          :title="legacyT(provider.is_active ? '停用提供商' : '启用提供商')"
          :aria-label="legacyT(provider.is_active ? '停用提供商' : '启用提供商')"
          @click="$emit('toggleStatus', provider)"
        >
          <Power class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-8 w-8 text-muted-foreground hover:text-destructive"
          :title="legacyT('删除提供商')"
          :aria-label="legacyT('删除提供商')"
          @click="$emit('deleteProvider', provider)"
        >
          <Trash2 class="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  </Card>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { Check, Edit, ExternalLink, Eye, KeyRound, Pencil, Power, Trash2, X } from 'lucide-vue-next'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import Card from '@/components/ui/card.vue'
import ProviderBalanceCell from './ProviderBalanceCell.vue'
import { formatApiFormatShort, type ProviderWithEndpointsSummary } from '@/api/endpoints'
import type { BalanceExtraItem } from '@/features/providers/auth-templates'
import {
  sortEndpoints,
  getEndpointHealthLabel,
  getEndpointHealthBarWidth,
  getEndpointDotColor,
  getEndpointTooltip,
} from '@/features/providers/composables/useEndpointStatus'
import { isKeyManagedProviderType } from '../utils/providerTypeUtils'
import { formatBillingType } from '@/utils/format'
import { safeExternalWebUrl } from '@/utils/navigationSecurity'
import { useI18n } from '@/i18n'

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
  editingDescriptionId: string | null
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

const { legacyT, locale } = useI18n()
const safeProviderWebsite = computed(() => safeExternalWebUrl(props.provider.website))
const localDescriptionValue = ref('')
const vAutoFocus = {
  mounted: (element: HTMLElement) => element.focus(),
}

watch(() => props.editingDescriptionId, (providerId) => {
  if (providerId === props.provider.id) {
    localDescriptionValue.value = props.provider.description || ''
  }
}, { immediate: true })

function handleStartEdit(event: Event) {
  event.stopPropagation()
  emit('startEditDescription', event, props.provider)
}

function handleSave(event: Event) {
  event.stopPropagation()
  emit('saveDescription', event, props.provider, localDescriptionValue.value)
}

function handleCancel(event: Event) {
  event.stopPropagation()
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
</script>
