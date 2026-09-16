<template>
  <div
    class="space-y-3"
    :class="disabled ? 'opacity-60' : ''"
  >
    <div class="flex flex-wrap items-start justify-between gap-2">
      <div class="min-w-0 space-y-1">
        <Label class="text-sm font-medium">{{ legacyT('允许的 Key') }}</Label>
        <p class="text-xs leading-5 text-muted-foreground">
          {{ disabled
            ? legacyT('提供商不限制时，Key 也不限制')
            : legacyT('未单独配置的已选 Provider，允许其全部现有及未来新增的 Key。') }}
        </p>
      </div>

      <Popover
        :open="addPopoverOpen"
        @update:open="setAddPopoverOpen"
      >
        <PopoverTrigger as-child>
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="shrink-0 gap-1.5"
            :disabled="disabled || availableProviders.length === 0"
          >
            <Plus class="h-4 w-4" />
            {{ legacyT('按提供商指定') }}
          </Button>
        </PopoverTrigger>
        <PopoverContent
          class="z-[130] w-[min(22rem,calc(100vw-2rem))] p-2"
          side="bottom"
          align="end"
        >
          <div class="space-y-2">
            <div class="relative">
              <Search
                class="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
              />
              <Input
                v-model="providerSearch"
                class="h-9 pl-9 text-sm"
                :placeholder="legacyT('搜索提供商...')"
                @keydown.stop
              />
            </div>
            <div class="max-h-56 overflow-y-auto">
              <button
                v-for="provider in filteredAvailableProviders"
                :key="provider.value"
                type="button"
                class="flex min-h-10 w-full items-center gap-2 rounded-md px-3 text-left text-sm transition-colors hover:bg-muted/60 disabled:cursor-wait disabled:opacity-60"
                :disabled="Boolean(keyLoadingByProvider[provider.value])"
                @click="addPolicy(provider.value)"
              >
                <Loader2
                  v-if="keyLoadingByProvider[provider.value]"
                  class="h-4 w-4 shrink-0 animate-spin text-muted-foreground"
                />
                <KeyRound
                  v-else
                  class="h-4 w-4 shrink-0 text-muted-foreground"
                />
                <span class="min-w-0 flex-1 truncate">{{ provider.label }}</span>
              </button>
              <p
                v-if="filteredAvailableProviders.length === 0"
                class="px-3 py-5 text-center text-sm text-muted-foreground"
              >
                {{ legacyT(availableProviders.length === 0 ? '所有已选 Provider 均已配置' : '未找到匹配项') }}
              </p>
            </div>
          </div>
        </PopoverContent>
      </Popover>
    </div>

    <div
      v-if="!disabled && policyProviderIds.length > 0"
      class="space-y-2"
    >
      <section
        v-for="providerId in policyProviderIds"
        :key="providerId"
        class="rounded-lg border border-border/70 bg-muted/15"
      >
        <header class="flex min-h-11 items-center gap-3 border-b border-border/60 px-3 py-2">
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-x-2 gap-y-1">
              <span class="truncate text-sm font-medium">{{ providerLabel(providerId) }}</span>
              <Badge
                variant="outline"
                class="shrink-0 rounded-md px-1.5 py-0 text-[10px] font-medium"
              >
                {{ policySummary(providerId) }}
              </Badge>
            </div>
          </div>
          <TooltipProvider>
            <Tooltip>
              <TooltipTrigger as-child>
                <button
                  type="button"
                  class="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                  :aria-label="`${legacyT('删除限制并恢复全部 Key')} ${providerLabel(providerId)}`"
                  @click="removePolicy(providerId)"
                >
                  <X class="h-4 w-4" />
                </button>
              </TooltipTrigger>
              <TooltipContent>{{ legacyT('删除限制并恢复全部 Key') }}</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </header>

        <div class="p-2">
          <div
            v-if="keyLoadingByProvider[providerId]"
            class="flex min-h-20 items-center justify-center gap-2 text-xs text-muted-foreground"
          >
            <Loader2 class="h-4 w-4 animate-spin" />
            {{ legacyT('正在加载 Key...') }}
          </div>

          <div
            v-else-if="keyErrorByProvider[providerId]"
            class="flex min-h-20 flex-col items-center justify-center gap-2 text-center text-xs text-muted-foreground"
          >
            <span>{{ legacyT('Key 加载失败') }}</span>
            <Button
              type="button"
              variant="outline"
              size="sm"
              @click="loadProviderKeys(providerId)"
            >
              <RefreshCw class="mr-1.5 h-3.5 w-3.5" />
              {{ legacyT('重试') }}
            </Button>
          </div>

          <div
            v-else
            class="space-y-1"
          >
            <label
              v-for="key in providerKeys(providerId)"
              :key="key.id"
              class="flex min-h-10 cursor-pointer items-center gap-3 rounded-md px-2 py-1.5 transition-colors hover:bg-muted/60"
            >
              <Checkbox
                :checked="isKeyAllowed(providerId, key.id)"
                @update:checked="setKeyAllowed(providerId, key.id, $event)"
              />
              <span class="min-w-0 flex-1">
                <span class="block truncate text-sm">{{ key.name }}</span>
                <span class="block truncate font-mono text-[10px] text-muted-foreground">
                  {{ maskedKeyTail(key) }}
                </span>
              </span>
              <Badge
                v-if="!key.is_active"
                variant="outline"
                class="shrink-0 rounded-md px-1.5 py-0 text-[10px] font-medium text-muted-foreground"
              >
                {{ legacyT('停用中，启用后生效') }}
              </Badge>
            </label>

            <label
              v-for="keyId in missingPolicyKeyIds(providerId)"
              :key="`missing-${keyId}`"
              class="flex min-h-10 cursor-pointer items-center gap-3 rounded-md bg-destructive/5 px-2 py-1.5"
            >
              <Checkbox
                :checked="true"
                @update:checked="setKeyAllowed(providerId, keyId, false)"
              />
              <span class="min-w-0 flex-1">
                <span class="block truncate text-sm text-destructive">{{ legacyT('已失效 Key') }}</span>
                <span class="block truncate font-mono text-[10px] text-destructive/70">
                  {{ compactId(keyId) }}
                </span>
              </span>
            </label>

            <p
              v-if="providerKeys(providerId).length === 0 && missingPolicyKeyIds(providerId).length === 0"
              class="px-2 py-4 text-center text-xs text-muted-foreground"
            >
              {{ legacyT('该 Provider 暂无 Key，此规则将不允许任何 Key') }}
            </p>
          </div>
        </div>

        <footer class="flex items-center gap-2 border-t border-border/60 px-3 py-2 text-[11px] text-muted-foreground">
          <ShieldCheck class="h-3.5 w-3.5 shrink-0" />
          {{ legacyT('以后新增的 Key 需手动添加') }}
        </footer>
      </section>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import {
  KeyRound,
  Loader2,
  Plus,
  RefreshCw,
  Search,
  ShieldCheck,
  X,
} from 'lucide-vue-next'
import { getProviderKeys } from '@/api/endpoints/keys'
import type { EndpointAPIKey } from '@/api/endpoints/types'
import {
  Badge,
  Button,
  Checkbox,
  Input,
  Label,
  Popover,
  PopoverContent,
  PopoverTrigger,
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useI18n } from '@/i18n'
import { matchesSearchQuery } from '@/utils/search'
import type { UserSelectOption } from './user-management-types'

const props = defineProps<{
  selectedProviderIds: string[]
  keyPolicies: Record<string, string[]>
  providerOptions: UserSelectOption[]
  disabled?: boolean
}>()

const emit = defineEmits<{
  'update:keyPolicies': [value: Record<string, string[]>]
}>()

const { legacyT } = useI18n()
const { error } = useToast()
const addPopoverOpen = ref(false)
const providerSearch = ref('')
const providerKeysById = reactive<Record<string, EndpointAPIKey[]>>({})
const keyLoadingByProvider = reactive<Record<string, boolean>>({})
const keyErrorByProvider = reactive<Record<string, boolean>>({})

const policyProviderIds = computed(() => {
  const selected = new Set(props.selectedProviderIds)
  return Object.keys(props.keyPolicies)
    .filter((providerId) => selected.has(providerId))
    .sort((left, right) => providerLabel(left).localeCompare(providerLabel(right)))
})

const availableProviders = computed(() => {
  const selected = new Set(props.selectedProviderIds)
  return props.providerOptions.filter((provider) =>
    selected.has(provider.value) && !hasPolicy(provider.value),
  )
})

const filteredAvailableProviders = computed(() => {
  const query = providerSearch.value.trim()
  if (!query) return availableProviders.value
  return availableProviders.value.filter((provider) =>
    matchesSearchQuery(query, provider.label, provider.value),
  )
})

watch(
  policyProviderIds,
  (providerIds) => {
    for (const providerId of providerIds) void loadProviderKeys(providerId)
  },
  { immediate: true },
)

function hasPolicy(providerId: string): boolean {
  return Object.prototype.hasOwnProperty.call(props.keyPolicies, providerId)
}

function providerLabel(providerId: string): string {
  return props.providerOptions.find((provider) => provider.value === providerId)?.label
    ?? compactId(providerId)
}

function providerKeys(providerId: string): EndpointAPIKey[] {
  return providerKeysById[providerId] ?? []
}

function policySummary(providerId: string): string {
  const count = props.keyPolicies[providerId]?.length ?? 0
  return count === 0
    ? legacyT('未允许任何 Key')
    : `${legacyT('仅允许')} ${count} ${legacyT('个 Key')}`
}

function compactId(value: string): string {
  const trimmed = value.trim()
  if (trimmed.length <= 12) return trimmed
  return `${trimmed.slice(0, 6)}...${trimmed.slice(-4)}`
}

function maskedKeyTail(key: EndpointAPIKey): string {
  const masked = key.api_key_masked?.trim()
  if (!masked) return compactId(key.id)
  const tail = masked.match(/([A-Za-z0-9]{4,8})$/)?.[1]
  return tail ? `····${tail}` : masked
}

function isKeyAllowed(providerId: string, keyId: string): boolean {
  return props.keyPolicies[providerId]?.includes(keyId) ?? false
}

function missingPolicyKeyIds(providerId: string): string[] {
  const known = new Set(providerKeys(providerId).map((key) => key.id))
  return (props.keyPolicies[providerId] ?? []).filter((keyId) => !known.has(keyId))
}

function setAddPopoverOpen(value: boolean): void {
  addPopoverOpen.value = value
  if (!value) providerSearch.value = ''
}

async function loadProviderKeys(providerId: string): Promise<EndpointAPIKey[] | null> {
  if (providerKeysById[providerId]) return providerKeysById[providerId]
  if (keyLoadingByProvider[providerId]) return null
  keyLoadingByProvider[providerId] = true
  keyErrorByProvider[providerId] = false
  try {
    const keys = await getProviderKeys(providerId)
    providerKeysById[providerId] = [...keys].sort((left, right) =>
      left.internal_priority - right.internal_priority
        || left.name.localeCompare(right.name),
    )
    return providerKeysById[providerId]
  } catch {
    keyErrorByProvider[providerId] = true
    return null
  } finally {
    keyLoadingByProvider[providerId] = false
  }
}

async function addPolicy(providerId: string): Promise<void> {
  const keys = await loadProviderKeys(providerId)
  if (!keys) {
    error(legacyT('无法加载 Provider Key，请重试'), legacyT('添加 Key 规则失败'))
    return
  }
  emit('update:keyPolicies', {
    ...props.keyPolicies,
    [providerId]: keys.map((key) => key.id).sort(),
  })
  setAddPopoverOpen(false)
}

function removePolicy(providerId: string): void {
  const next = { ...props.keyPolicies }
  delete next[providerId]
  emit('update:keyPolicies', next)
}

function setKeyAllowed(providerId: string, keyId: string, allowed: boolean): void {
  const selected = new Set(props.keyPolicies[providerId] ?? [])
  if (allowed) selected.add(keyId)
  else selected.delete(keyId)
  emit('update:keyPolicies', {
    ...props.keyPolicies,
    [providerId]: [...selected].sort(),
  })
}
</script>
