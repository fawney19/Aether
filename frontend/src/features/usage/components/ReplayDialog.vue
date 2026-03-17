<template>
  <Teleport to="body">
    <Transition name="fade">
      <div
        v-if="isOpen"
        class="fixed inset-0 z-[60] flex items-center justify-center"
        @click.self="handleClose"
      >
        <div
          class="absolute inset-0 bg-black/30 backdrop-blur-sm"
          @click="handleClose"
        />
        <Card class="relative w-full max-w-6xl max-h-[85vh] min-h-[60vh] mx-4 shadow-2xl flex flex-col">
          <!-- 头部：标题 + 提供商/Key 选择 + 发送 -->
          <div class="px-4 py-2.5 border-b flex items-center gap-3 shrink-0 flex-wrap">
            <h3 class="text-sm font-semibold shrink-0">
              请求回放
            </h3>
            <Separator
              orientation="vertical"
              class="h-4"
            />

            <!-- 提供商选择 -->
            <div class="flex items-center gap-1.5 min-w-0">
              <label class="text-xs text-muted-foreground shrink-0">提供商</label>
              <select
                v-model="selectedProviderId"
                class="h-7 rounded-md border border-input bg-background px-2 text-xs min-w-[140px]"
                :disabled="replaying"
                @change="onProviderChange"
              >
                <option value="">
                  原始 ({{ detail?.provider || '-' }})
                </option>
                <option
                  v-for="p in providers"
                  :key="p.id"
                  :value="p.id"
                >
                  {{ p.name }}
                </option>
              </select>
            </div>

            <!-- Key 选择 -->
            <div class="flex items-center gap-1.5 min-w-0">
              <label class="text-xs text-muted-foreground shrink-0">Key</label>
              <select
                v-model="selectedKeyId"
                class="h-7 rounded-md border border-input bg-background px-2 text-xs min-w-[140px]"
                :disabled="replaying || loadingKeys"
              >
                <option value="">
                  {{ loadingKeys ? '加载中...' : selectedProviderId ? '自动选择' : '原始 Key' }}
                </option>
                <option
                  v-for="k in keys"
                  :key="k.id"
                  :value="k.id"
                >
                  {{ k.name }} ({{ k.api_key_masked }})
                </option>
              </select>
            </div>

            <!-- 右侧：发送 + 关闭 -->
            <div class="flex items-center gap-1 ml-auto shrink-0">
              <Button
                size="sm"
                :disabled="replaying"
                class="gap-1.5 h-7 text-xs"
                @click="doReplay"
              >
                <Loader2
                  v-if="replaying"
                  class="w-3.5 h-3.5 animate-spin"
                />
                <Play
                  v-else
                  class="w-3.5 h-3.5"
                />
                {{ replaying ? '请求中...' : '发送' }}
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-7 w-7"
                @click="handleClose"
              >
                <X class="w-4 h-4" />
              </Button>
            </div>
          </div>

          <!-- 双栏内容区 -->
          <div class="flex-1 min-h-0 flex">
            <!-- ===== 左栏：请求 ===== -->
            <div class="w-1/2 flex flex-col min-h-0 border-r">
              <!-- 左栏头 -->
              <div class="px-4 py-1.5 border-b bg-muted/30 flex items-center justify-between shrink-0">
                <div class="flex items-center gap-2 min-w-0">
                  <span class="text-xs font-medium text-muted-foreground">请求</span>
                  <span
                    v-if="detail?.model"
                    class="text-[11px] text-muted-foreground/60 font-mono truncate"
                  >{{ detail.model }}</span>
                </div>
                <button
                  class="p-1 rounded transition-colors text-muted-foreground hover:bg-muted shrink-0"
                  :title="requestCopied ? '已复制' : '复制请求体'"
                  @click="copyRequestBody"
                >
                  <Check
                    v-if="requestCopied"
                    class="w-3 h-3 text-green-500"
                  />
                  <Copy
                    v-else
                    class="w-3 h-3"
                  />
                </button>
              </div>
              <!-- 左栏内容 -->
              <div class="flex-1 overflow-y-auto scrollbar-stable">
                <!-- 请求头（可折叠） -->
                <div class="border-b">
                  <button
                    class="w-full px-4 py-1.5 flex items-center gap-1.5 text-xs text-muted-foreground hover:bg-muted/30 transition-colors"
                    @click="showRequestHeaders = !showRequestHeaders"
                  >
                    <ChevronRight
                      class="w-3 h-3 transition-transform shrink-0"
                      :class="{ 'rotate-90': showRequestHeaders }"
                    />
                    <span class="font-medium">Headers</span>
                    <span class="text-muted-foreground/60 ml-0.5">({{ requestHeaderCount }})</span>
                  </button>
                  <div
                    v-if="showRequestHeaders && hasRequestHeaders"
                    class="px-4 pb-2.5"
                  >
                    <div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[11px] font-mono">
                      <template
                        v-for="(value, key) in displayRequestHeaders"
                        :key="key"
                      >
                        <span class="text-muted-foreground/70 text-right select-none">{{ key }}</span>
                        <span class="break-all">{{ value }}</span>
                      </template>
                    </div>
                  </div>
                </div>
                <!-- 请求体 -->
                <div class="border-b">
                  <div class="px-4 py-1.5 flex items-center gap-1.5 text-xs text-muted-foreground">
                    <span class="w-3 h-3 shrink-0" />
                    <span class="font-medium">Body</span>
                  </div>
                </div>
                <div class="px-4 py-3">
                  <pre
                    v-if="formattedRequestBody"
                    class="text-xs font-mono whitespace-pre-wrap break-all leading-relaxed"
                  >{{ formattedRequestBody }}</pre>
                  <div
                    v-else
                    class="text-xs text-muted-foreground/50 italic"
                  >
                    无请求体
                  </div>
                </div>
              </div>
            </div>

            <!-- ===== 右栏：响应 ===== -->
            <div class="w-1/2 flex flex-col min-h-0">
              <!-- 右栏头 -->
              <div class="px-4 py-1.5 border-b bg-muted/30 flex items-center justify-between shrink-0">
                <div class="flex items-center gap-2 min-w-0">
                  <span class="text-xs font-medium text-muted-foreground">响应</span>
                  <template v-if="replayResult">
                    <Badge
                      :variant="replayResult.status_code < 400 ? 'success' : 'destructive'"
                      class="text-[10px] px-1.5 py-0 h-4"
                    >
                      {{ replayResult.status_code }}
                    </Badge>
                    <span class="text-[11px] text-muted-foreground/60">{{ replayResult.response_time_ms }}ms</span>
                    <span class="text-[11px] text-muted-foreground/60 font-mono truncate">{{ replayResult.provider }}</span>
                  </template>
                </div>
                <button
                  v-if="replayResult"
                  class="p-1 rounded transition-colors text-muted-foreground hover:bg-muted shrink-0"
                  :title="responseCopied ? '已复制' : '复制响应体'"
                  @click="copyResponseBody"
                >
                  <Check
                    v-if="responseCopied"
                    class="w-3 h-3 text-green-500"
                  />
                  <Copy
                    v-else
                    class="w-3 h-3"
                  />
                </button>
              </div>
              <!-- 右栏内容 -->
              <div class="flex-1 overflow-y-auto scrollbar-stable">
                <!-- 空状态 -->
                <div
                  v-if="!replayResult && !replayError && !replaying"
                  class="flex flex-col items-center justify-center h-full text-muted-foreground/40 gap-2"
                >
                  <Play class="w-8 h-8" />
                  <span class="text-xs">点击发送查看响应</span>
                </div>

                <!-- Loading -->
                <div
                  v-else-if="replaying && !replayResult"
                  class="flex items-center justify-center h-full"
                >
                  <Loader2 class="w-6 h-6 animate-spin text-muted-foreground" />
                </div>

                <!-- 错误 -->
                <div
                  v-else-if="replayError"
                  class="px-4 py-4"
                >
                  <div class="rounded-lg bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 p-3">
                    <p class="text-sm text-red-600 dark:text-red-400">
                      {{ replayError }}
                    </p>
                  </div>
                </div>

                <!-- 响应结果 -->
                <template v-if="replayResult">
                  <div
                    v-if="replayResult.mapping"
                    class="border-b"
                  >
                    <div class="px-4 py-2 text-[11px] text-muted-foreground flex flex-col gap-1">
                      <div class="flex flex-wrap items-center gap-2">
                        <span class="font-mono truncate">{{ replayResult.mapping.source_model }}</span>
                        <span class="text-muted-foreground/50">→</span>
                        <span class="font-mono truncate">{{ replayResult.mapping.resolved_model }}</span>
                        <Badge
                          variant="outline"
                          class="text-[10px] px-1.5 py-0 h-4"
                        >
                          {{ formatReplayMode(replayResult.mapping.replay_mode) }}
                        </Badge>
                      </div>
                      <div class="flex flex-wrap gap-2 text-muted-foreground/60">
                        <span>Provider: {{ replayResult.mapping.target_provider }}</span>
                        <span>Endpoint: {{ replayResult.mapping.target_endpoint_id }}</span>
                        <span>Format: {{ replayResult.mapping.target_api_format || '-' }}</span>
                      </div>
                      <div
                        v-if="replayResult.mapping.mapping_source && replayResult.mapping.mapping_source !== 'none'"
                        class="text-muted-foreground/50"
                      >
                        {{ formatMappingSource(replayResult.mapping.mapping_source) }}
                      </div>
                    </div>
                  </div>
                  <!-- 响应头（可折叠） -->
                  <div class="border-b">
                    <button
                      class="w-full px-4 py-1.5 flex items-center gap-1.5 text-xs text-muted-foreground hover:bg-muted/30 transition-colors"
                      @click="showResponseHeaders = !showResponseHeaders"
                    >
                      <ChevronRight
                        class="w-3 h-3 transition-transform shrink-0"
                        :class="{ 'rotate-90': showResponseHeaders }"
                      />
                      <span class="font-medium">Headers</span>
                      <span class="text-muted-foreground/60 ml-0.5">({{ responseHeaderCount }})</span>
                    </button>
                    <div
                      v-if="showResponseHeaders"
                      class="px-4 pb-2.5"
                    >
                      <div class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-[11px] font-mono">
                        <template
                          v-for="(value, key) in replayResult.response_headers"
                          :key="key"
                        >
                          <span class="text-muted-foreground/70 text-right select-none">{{ key }}</span>
                          <span class="break-all">{{ value }}</span>
                        </template>
                      </div>
                    </div>
                  </div>
                  <!-- 响应体 -->
                  <div class="border-b">
                    <div class="px-4 py-1.5 flex items-center gap-1.5 text-xs text-muted-foreground">
                      <span class="w-3 h-3 shrink-0" />
                      <span class="font-medium">Body</span>
                      <span
                        v-if="replayResult.url"
                        class="text-muted-foreground/40 font-mono text-[10px] truncate ml-1"
                      >{{ replayResult.url }}</span>
                    </div>
                  </div>
                  <div class="px-4 py-3">
                    <pre class="text-xs font-mono whitespace-pre-wrap break-all leading-relaxed">{{ formattedResponseBody }}</pre>
                  </div>
                </template>
              </div>
            </div>
          </div>
        </Card>
      </div>
    </Transition>
  </Teleport>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { dashboardApi, type ReplayResponse, type RequestDetail } from '@/api/dashboard'
import { getProvidersSummary } from '@/api/endpoints/providers'
import { getProviderKeys } from '@/api/endpoints/keys'
import type { EndpointAPIKey } from '@/api/endpoints/types'
import { useClipboard } from '@/composables/useClipboard'
import { useEscapeKey } from '@/composables/useEscapeKey'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import Separator from '@/components/ui/separator.vue'
import { X, Play, Loader2, ChevronRight, Copy, Check } from 'lucide-vue-next'
import { log } from '@/utils/logger'

interface ProviderOption {
  id: string
  name: string
}

const props = defineProps<{
  isOpen: boolean
  requestId: string | null
  detail: RequestDetail | null
}>()

const emit = defineEmits<{
  close: []
}>()

const selectedProviderId = ref('')
const selectedKeyId = ref('')
const providers = ref<ProviderOption[]>([])
const keys = ref<EndpointAPIKey[]>([])
const loadingKeys = ref(false)
const replaying = ref(false)
const replayResult = ref<ReplayResponse | null>(null)
const replayError = ref<string | null>(null)
const showRequestHeaders = ref(false)
const showResponseHeaders = ref(false)
const requestCopied = ref(false)
const responseCopied = ref(false)
const { copyToClipboard } = useClipboard()

// ---- 请求侧数据 ----

const displayRequestHeaders = computed(() => {
  if (!props.detail) return {}
  // 优先显示发送给提供商的请求头，否则显示客户端请求头
  return props.detail.provider_request_headers || props.detail.request_headers || {}
})

const hasRequestHeaders = computed(() => {
  return Object.keys(displayRequestHeaders.value).length > 0
})

const requestHeaderCount = computed(() => {
  return Object.keys(displayRequestHeaders.value).length
})

const formattedRequestBody = computed(() => {
  if (!props.detail?.request_body) return ''
  try {
    return JSON.stringify(props.detail.request_body, null, 2)
  } catch {
    return String(props.detail.request_body)
  }
})

// ---- 响应侧数据 ----

const formattedResponseBody = computed(() => {
  if (!replayResult.value?.response_body) return ''
  try {
    return JSON.stringify(replayResult.value.response_body, null, 2)
  } catch {
    return String(replayResult.value.response_body)
  }
})

const responseHeaderCount = computed(() => {
  if (!replayResult.value?.response_headers) return 0
  return Object.keys(replayResult.value.response_headers).length
})

function formatReplayMode(mode?: string) {
  switch (mode) {
    case 'same_endpoint_reuse':
      return '同端点复用'
    case 'same_provider_remap':
      return '同 Provider 重映射'
    case 'cross_provider_remap':
      return '跨 Provider 重映射'
    default:
      return mode || '-'
  }
}

function formatMappingSource(source?: string) {
  switch (source) {
    case 'original_target_model':
      return '映射来源: 复用原目标模型'
    case 'model_mapping':
      return '映射来源: 模型映射'
    case 'none':
      return '映射来源: 未映射'
    default:
      return source ? `映射来源: ${source}` : ''
  }
}

// ---- 生命周期 ----

watch(() => props.isOpen, async (isOpen) => {
  if (isOpen) {
    replayResult.value = null
    replayError.value = null
    selectedProviderId.value = ''
    selectedKeyId.value = ''
    keys.value = []
    showRequestHeaders.value = false
    showResponseHeaders.value = false
    try {
      const response = await getProvidersSummary({ page_size: 9999 })
      providers.value = response.items
        .filter(p => p.is_active && p.active_endpoints > 0)
        .map(p => ({ id: p.id, name: p.name }))
    } catch (e) {
      log.error('Failed to load providers:', e)
    }
  }
})

async function onProviderChange() {
  selectedKeyId.value = ''
  keys.value = []
  if (!selectedProviderId.value) return

  loadingKeys.value = true
  try {
    const allKeys = await getProviderKeys(selectedProviderId.value)
    keys.value = allKeys.filter(k => k.health_score > 0 || allKeys.length <= 3)
    if (keys.value.length === 0) keys.value = allKeys
  } catch (e) {
    log.error('Failed to load keys:', e)
  } finally {
    loadingKeys.value = false
  }
}

async function doReplay() {
  if (!props.requestId || replaying.value) return

  replaying.value = true
  replayError.value = null
  replayResult.value = null

  try {
    const params: Record<string, string> = {}
    if (selectedProviderId.value) params.provider_id = selectedProviderId.value
    if (selectedKeyId.value) params.api_key_id = selectedKeyId.value

    replayResult.value = await dashboardApi.replayRequest(
      props.requestId,
      Object.keys(params).length > 0 ? params : undefined,
    )
  } catch (e: unknown) {
    const err = e as { response?: { data?: { detail?: string } }; message?: string }
    replayError.value = err?.response?.data?.detail || err?.message || '请求失败'
    log.error('Replay failed:', e)
  } finally {
    replaying.value = false
  }
}

function copyRequestBody() {
  if (!formattedRequestBody.value) return
  copyToClipboard(formattedRequestBody.value, false)
  requestCopied.value = true
  setTimeout(() => { requestCopied.value = false }, 2000)
}

function copyResponseBody() {
  if (!replayResult.value) return
  copyToClipboard(formattedResponseBody.value, false)
  responseCopied.value = true
  setTimeout(() => { responseCopied.value = false }, 2000)
}

function handleClose() {
  emit('close')
}

useEscapeKey(() => {
  if (props.isOpen) handleClose()
}, { disableOnInput: true, once: false })
</script>

<style scoped>
.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s ease;
}
.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
