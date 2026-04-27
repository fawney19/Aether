<template>
  <div
    class="p-4 space-y-3 hover:bg-muted/20 transition-colors cursor-pointer"
    @click="$emit('viewDetail', provider.id)"
  >
    <!-- 第一行：名称 + 状态 + 操作 -->
    <div class="flex items-start justify-between gap-3">
      <div class="flex-1 min-w-0 space-y-0.5">
        <div class="flex items-center gap-1.5">
          <span class="font-medium text-foreground truncate">{{ provider.name }}</span>
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
          <Badge
            :variant="provider.is_active ? 'success' : 'secondary'"
            class="text-xs shrink-0"
          >
            {{ provider.is_active ? '活跃' : '停用' }}
          </Badge>
        </div>
        <!-- 内联编辑备注 (移动端) -->
        <div
          v-if="editingDescriptionId === provider.id"
          data-desc-editor
          class="flex items-center gap-1 max-w-[180px]"
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
          class="text-xs text-muted-foreground truncate block max-w-[120px] group/desc cursor-pointer hover:text-foreground/70 transition-colors"
          :title="provider.description"
          @click="handleStartEdit"
        >{{ provider.description }} <Pencil class="w-3 h-3 inline-block opacity-0 group-hover/desc:opacity-50 transition-opacity" /></span>
        <span
          v-else
          class="text-xs text-muted-foreground cursor-pointer hover:text-foreground/70 transition-colors"
          @click="handleStartEdit"
        >添加备注</span>
      </div>
      <div
        class="flex items-center gap-0.5 shrink-0"
        @click.stop
      >
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          title="查看详情"
          @click="$emit('viewDetail', provider.id)"
        >
          <Eye class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          title="编辑"
          @click="$emit('editProvider', provider)"
        >
          <Edit class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          title="扩展操作配置"
          @click="$emit('openOpsConfig', provider)"
        >
          <KeyRound class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          @click="$emit('toggleStatus', provider)"
        >
          <Power class="h-3.5 w-3.5" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          class="h-7 w-7"
          @click="$emit('deleteProvider', provider)"
        >
          <Trash2 class="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>

    <!-- 第二行：计费类型 + 余额/配额 + 资源统计 -->
    <div class="flex flex-wrap items-center gap-3 text-xs">
      <Badge
        variant="outline"
        class="text-xs font-normal border-border/50"
      >
        {{ formatBillingType(provider.billing_type || 'pay_as_you_go') }}
      </Badge>
      <!-- 余额加载中 -->
      <span
        v-if="provider.ops_configured && isBalanceLoading(provider.id)"
        class="text-muted-foreground flex items-center gap-1"
      >
        <Loader2 class="h-3 w-3 animate-spin" />
        加载中...
      </span>
      <!-- 余额（从上游 API 查询） -->
      <span
        v-else-if="provider.ops_configured && getProviderBalance(provider.id)"
        class="text-muted-foreground"
      >
        余额 <span class="font-semibold text-foreground/90">{{ formatBalanceDisplay(getProviderBalance(provider.id)) }}</span>
        <!-- Cookie 失效警告 -->
        <span
          v-if="getProviderCookieExpired(provider.id)"
          class="ml-1 text-amber-600 dark:text-amber-500"
          :title="getProviderCookieExpired(provider.id)?.message"
        >签到 Cookie 已失效</span>
        <!-- 签到状态显示 -->
        <span
          v-else-if="getProviderCheckin(provider.id) && getProviderCheckin(provider.id)?.success !== false"
          class="ml-1 text-muted-foreground"
          :title="getProviderCheckin(provider.id)?.message"
        >已签到</span>
        <span
          v-else-if="getProviderCheckin(provider.id)?.success === false"
          class="ml-1 text-destructive/70"
          :title="getProviderCheckin(provider.id)?.message"
        >签到失败</span>
      </span>
      <!-- 余额查询失败时显示错误 -->
      <span
        v-else-if="provider.ops_configured && getProviderBalanceError(provider.id)"
        class="text-destructive/80"
        :title="getProviderBalanceError(provider.id)?.message"
      >
        {{ getProviderBalanceError(provider.id)?.message }}
      </span>
      <!-- 本地配额 -->
      <span
        v-else-if="provider.billing_type === 'monthly_quota'"
        class="text-muted-foreground"
      >
        配额 <span
          class="font-semibold"
          :class="getQuotaUsedColorClass(provider)"
        >${{ (provider.monthly_used_usd ?? 0).toFixed(2) }}</span>/<span class="font-medium">${{ (provider.monthly_quota_usd ?? 0).toFixed(2) }}</span>
      </span>
      <span class="text-muted-foreground">
        端点 {{ provider.active_endpoints }}/{{ provider.total_endpoints }}
      </span>
      <span class="text-muted-foreground">
        {{ getCredentialLabel(provider) }} {{ provider.active_keys }}/{{ provider.total_keys }}
      </span>
      <span class="text-muted-foreground">
        模型 {{ provider.active_models }}/{{ provider.total_models }}
      </span>
    </div>

    <!-- 第三行：端点健康 -->
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
  </div>
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
  Loader2,
} from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import { type ProviderWithEndpointsSummary, formatApiFormatShort } from '@/api/endpoints'
import { formatBillingType } from '@/utils/format'
import { sortEndpoints, isEndpointAvailable, getEndpointDotColor, getEndpointTooltip } from '@/features/providers/composables/useEndpointStatus'
import { isKeyManagedProviderType } from '../utils/providerTypeUtils'

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
  editingDescriptionId: string | null
  // Balance functions
  isBalanceLoading: (providerId: string) => boolean
  getProviderBalance: (providerId: string) => { available: number | null; currency: string } | null
  getProviderBalanceError: (providerId: string) => { status: string; message: string } | null
  getProviderCheckin: (providerId: string) => { success: boolean | null; message: string } | null
  getProviderCookieExpired: (providerId: string) => { expired: boolean; message: string } | null
  formatBalanceDisplay: (balance: { available: number | null; currency: string } | null) => string
  getQuotaUsedColorClass: (provider: ProviderWithEndpointsSummary) => string
}>()

const emit = defineEmits<{
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
  return isKeyManagedProviderType(provider.provider_type) ? '密钥' : '账号'
}
</script>
