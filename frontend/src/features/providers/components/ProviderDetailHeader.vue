<template>
  <div class="sticky top-0 z-10 bg-background border-b px-4 sm:px-6 pt-4 sm:pt-6 pb-3 sm:pb-3">
    <div class="flex items-center justify-between gap-x-3 sm:gap-x-4 flex-wrap">
      <div class="flex items-center gap-2 min-w-0">
        <h2 class="text-lg sm:text-xl font-bold truncate">
          {{ provider.name }}
        </h2>
        <Badge
          :variant="provider.is_active ? 'default' : 'secondary'"
          class="text-xs shrink-0"
        >
          {{ legacyT(provider.is_active ? '活跃' : '停用') }}
        </Badge>
      </div>
      <div class="flex items-center gap-1 shrink-0">
        <span :title="formatConversionTitle">
          <Button
            variant="ghost"
            size="icon"
            :class="(provider.enable_format_conversion || systemFormatConversionEnabled) ? 'text-primary' : ''"
            :disabled="systemFormatConversionEnabled"
            @click="$emit('toggleFormatConversion')"
          >
            <Shuffle class="w-4 h-4" />
          </Button>
        </span>
        <span :title="legacyT(hasFailoverRules ? '已配置故障转移规则（点击编辑）' : '配置故障转移规则')">
          <Button
            variant="ghost"
            size="icon"
            :class="hasFailoverRules ? 'text-orange-500 dark:text-orange-400' : ''"
            @click="$emit('openFailoverRules')"
          >
            <GitBranch class="w-4 h-4" />
          </Button>
        </span>
        <Popover
          :open="providerProxyPopoverOpen"
          @update:open="$emit('update:providerProxyPopoverOpen', $event)"
        >
          <PopoverTrigger as-child>
            <Button
              variant="ghost"
              size="icon"
              :class="provider.proxy?.node_id ? 'text-blue-500' : ''"
              :disabled="savingProviderProxy"
              :title="provider.proxy?.node_id ? `${legacyT('代理')}: ${providerProxyNodeName}` : legacyT('设置代理节点')"
            >
              <Globe class="w-4 h-4" />
            </Button>
          </PopoverTrigger>
          <PopoverContent
            class="w-72 p-3"
            side="bottom"
            align="end"
          >
            <div class="space-y-2">
              <div class="flex items-center justify-between">
                <span class="text-xs font-medium">{{ legacyT('代理节点') }}</span>
                <Button
                  v-if="provider.proxy?.node_id"
                  variant="ghost"
                  size="sm"
                  class="h-6 px-2 text-[10px] text-muted-foreground"
                  :disabled="savingProviderProxy"
                  @click="$emit('clearProviderProxy')"
                >
                  {{ legacyT('清除') }}
                </Button>
              </div>
              <ProxyNodeSelect
                :model-value="provider.proxy?.node_id || ''"
                trigger-class="h-8"
                @update:model-value="$emit('setProviderProxy', $event)"
              />
              <p class="text-[10px] text-muted-foreground">
                {{ legacyT(provider.proxy?.node_id ? '当前使用独立代理' : '未设置代理节点') }}
              </p>
            </div>
          </PopoverContent>
        </Popover>
        <Button
          variant="ghost"
          size="icon"
          :title="legacyT('编辑提供商')"
          @click="$emit('edit', provider)"
        >
          <Edit class="w-4 h-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          :title="legacyT(provider.is_active ? '点击停用' : '点击启用')"
          @click="$emit('toggleStatus', provider)"
        >
          <Power class="w-4 h-4" />
        </Button>
        <Button
          variant="ghost"
          size="icon"
          :title="legacyT('关闭')"
          @click="$emit('close')"
        >
          <X class="w-4 h-4" />
        </Button>
      </div>
    </div>

    <div
      v-if="provider.website"
      class="-mt-0.5"
    >
      <a
        :href="provider.website"
        target="_blank"
        rel="noopener noreferrer"
        class="text-xs text-muted-foreground hover:text-primary hover:underline transition-colors truncate block"
        :title="provider.website"
      >{{ provider.website }}</a>
    </div>

    <div class="flex items-center gap-1.5 flex-wrap mt-3">
      <template v-if="loadingProviderEndpoints && endpoints.length === 0">
        <span class="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
          <Loader2 class="w-3.5 h-3.5 animate-spin" />
          {{ legacyT('加载端点中') }}
        </span>
      </template>
      <template v-else>
        <template
          v-for="endpoint in endpoints"
          :key="endpoint.id"
        >
          <span
            class="text-xs px-2 py-0.5 rounded-md border border-border bg-background hover:bg-accent hover:border-accent-foreground/20 cursor-pointer transition-colors font-medium"
            :class="{ 'opacity-40': !endpoint.is_active }"
            :title="legacyT('编辑端点')"
            @click="$emit('editEndpoint', endpoint)"
          >{{ formatApiFormat(endpoint.api_format) }}</span>
        </template>
        <span
          v-if="endpoints.length > 0"
          class="text-xs px-2 py-0.5 rounded-md border border-dashed border-border hover:bg-accent hover:border-accent-foreground/20 cursor-pointer transition-colors text-muted-foreground"
          :title="legacyT('编辑端点')"
          @click="$emit('addEndpoint')"
        >{{ legacyT('编辑') }}</span>
        <Button
          v-else
          variant="outline"
          size="sm"
          class="h-7 text-xs"
          @click="$emit('addEndpoint')"
        >
          <Plus class="w-3 h-3 mr-1" />
          {{ legacyT('添加 API 端点') }}
        </Button>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Edit, GitBranch, Globe, Loader2, Plus, Power, Shuffle, X } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui'
import { useI18n } from '@/i18n'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import type { ProviderEndpoint, ProviderWithEndpointsSummary } from '@/api/endpoints'
import ProxyNodeSelect from './ProxyNodeSelect.vue'

const props = defineProps<{
  provider: ProviderWithEndpointsSummary
  endpoints: ProviderEndpoint[]
  loadingProviderEndpoints: boolean
  systemFormatConversionEnabled: boolean
  hasFailoverRules: boolean
  providerProxyPopoverOpen: boolean
  providerProxyNodeName: string
  savingProviderProxy: boolean
}>()

defineEmits<{
  (e: 'toggleFormatConversion'): void
  (e: 'openFailoverRules'): void
  (e: 'update:providerProxyPopoverOpen', value: boolean): void
  (e: 'setProviderProxy', value: string): void
  (e: 'clearProviderProxy'): void
  (e: 'edit', provider: ProviderWithEndpointsSummary): void
  (e: 'toggleStatus', provider: ProviderWithEndpointsSummary): void
  (e: 'close'): void
  (e: 'editEndpoint', endpoint: ProviderEndpoint): void
  (e: 'addEndpoint'): void
}>()

const { legacyT } = useI18n()

const formatConversionTitle = computed(() => {
  if (props.systemFormatConversionEnabled) return legacyT('系统级格式转换已启用')
  if (props.provider.enable_format_conversion) return legacyT('已启用格式转换（点击关闭）')
  return legacyT('启用格式转换')
})
</script>
