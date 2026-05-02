<template>
  <Dialog
    :model-value="isOpen"
    title="添加账号"
    :icon="UserPlus"
    size="md"
    @update:model-value="handleDialogUpdate"
  >
    <!-- 右上角代理按钮 -->
    <template #header-actions>
      <Popover
        :open="proxyPopoverOpen"
        @update:open="(v: boolean) => { proxyPopoverOpen = v; if (v) proxyNodesStore.ensureLoaded() }"
      >
        <PopoverTrigger as-child>
          <button
            class="flex items-center justify-center w-8 h-8 rounded-md transition-colors shrink-0"
            :class="selectedProxyNodeId
              ? 'text-blue-500 bg-blue-500/10 hover:bg-blue-500/20'
              : 'text-muted-foreground hover:text-foreground hover:bg-muted'"
            :title="selectedProxyNodeId ? `代理: ${getSelectedNodeLabel()}` : '设置代理节点'"
          >
            <Globe class="w-4 h-4" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          class="w-72 p-3 z-[80]"
          side="bottom"
          align="end"
        >
          <div class="space-y-2">
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-1.5">
                <span class="text-xs font-medium">代理节点</span>
                <span
                  v-if="!proxyNodesStore.loading && proxyNodesStore.onlineNodes.length === 0"
                  class="text-[10px] text-muted-foreground"
                >· 前往「模块管理 · 代理节点」添加</span>
              </div>
              <button
                v-if="selectedProxyNodeId"
                class="text-[10px] text-muted-foreground hover:text-foreground transition-colors"
                @click="selectedProxyNodeId = ''; proxyPopoverOpen = false"
              >
                清除
              </button>
            </div>
            <ProxyNodeSelect
              :model-value="selectedProxyNodeId"
              trigger-class="h-8"
              @update:model-value="(v: string) => { selectedProxyNodeId = v; proxyPopoverOpen = false }"
            />
            <p class="text-[10px] text-muted-foreground">
              {{ selectedProxyNodeId ? '授权、刷新、额度查询均走此代理' : '未设置，依次回退到提供商代理 → 系统代理' }}
            </p>
          </div>
        </PopoverContent>
      </Popover>
    </template>

    <div class="space-y-4">
      <!-- Tab 切换 -->
      <div class="flex rounded-lg border border-border p-0.5 bg-muted/30">
        <button
          class="flex-1 px-3 py-1.5 text-xs font-medium rounded-md transition-all"
          :class="[
            mode === 'oauth'
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground',
          ]"
          @click="switchMode('oauth')"
        >
          {{ isKiroProvider ? '设备授权' : '获取授权' }}
        </button>
        <button
          class="flex-1 px-3 py-1.5 text-xs font-medium rounded-md transition-all"
          :class="mode === 'import'
            ? 'bg-background text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'"
          @click="switchMode('import')"
        >
          导入授权
        </button>
      </div>

      <!-- Tab 内容：grid 叠放，高度取较高者 -->
      <div class="grid [&>*]:col-start-1 [&>*]:row-start-1">
        <!-- ===== 获取授权 / 设备授权 ===== -->
        <div
          class="space-y-4 transition-opacity duration-150"
          :class="mode === 'oauth' ? 'opacity-100' : 'opacity-0 pointer-events-none'"
        >
          <!-- Kiro: 设备授权模式 -->
          <template v-if="isKiroProvider">
            <!-- 初始状态：选择授权类型 + 开始 -->
            <div
              v-if="!device.session_id && !device.starting"
              class="space-y-3"
            >
              <!-- Builder ID / Identity Center 切换 -->
              <div class="grid grid-cols-2 gap-1.5">
                <button
                  v-for="opt in ([
                    { key: 'builder_id', label: 'Builder ID' },
                    { key: 'identity_center', label: 'Identity Center' },
                  ] as const)"
                  :key="opt.key"
                  class="h-8 text-xs font-medium rounded-md border transition-colors"
                  :class="device.auth_type === opt.key
                    ? 'border-primary bg-primary/5 text-foreground'
                    : 'border-border text-muted-foreground hover:text-foreground hover:border-foreground/20'"
                  @click="device.auth_type = opt.key"
                >
                  {{ opt.label }}
                </button>
              </div>

              <!-- grid 叠放保持高度稳定 -->
              <div class="grid [&>*]:col-start-1 [&>*]:row-start-1">
                <!-- Builder ID: 说明文字 -->
                <div
                  class="flex items-center justify-center transition-opacity duration-150"
                  :class="device.auth_type === 'builder_id' ? 'opacity-100' : 'opacity-0 pointer-events-none'"
                >
                  <p class="text-xs text-muted-foreground">
                    使用个人 AWS Builder ID 进行设备授权，无需额外配置。
                  </p>
                </div>

                <!-- Identity Center: Start URL + Region -->
                <div
                  class="space-y-3 transition-opacity duration-150"
                  :class="device.auth_type === 'identity_center' ? 'opacity-100' : 'opacity-0 pointer-events-none'"
                >
                  <div class="space-y-1.5">
                    <label class="text-xs font-medium">Start URL</label>
                    <input
                      v-model="device.start_url"
                      type="text"
                      placeholder="https://your-org.awsapps.com/start"
                      class="w-full h-8 px-2 text-xs rounded-md border border-border bg-background font-mono focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                      spellcheck="false"
                    >
                  </div>
                  <div class="space-y-1.5">
                    <label class="text-xs font-medium">Region</label>
                    <ComboboxRoot
                      :model-value="device.region"
                      :open="regionComboboxOpen"
                      @update:model-value="(v: string) => { if (v) device.region = v }"
                      @update:open="(v: boolean) => { regionComboboxOpen = v; if (v) ensureAwsRegions() }"
                    >
                      <ComboboxAnchor class="relative w-full">
                        <ComboboxInput
                          :display-value="() => device.region"
                          placeholder="输入或选择 Region"
                          class="w-full h-8 px-2 pr-7 text-xs rounded-md border border-border bg-background font-mono focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                          spellcheck="false"
                          @input="(e: Event) => regionSearch = (e.target as HTMLInputElement).value"
                          @keydown.enter.prevent="onRegionEnter"
                        />
                        <ComboboxTrigger class="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground">
                          <ChevronsUpDown class="w-3.5 h-3.5" />
                        </ComboboxTrigger>
                      </ComboboxAnchor>
                      <ComboboxContent
                        position="popper"
                        class="z-[99] mt-1 max-h-[200px] w-[--radix-combobox-trigger-width] overflow-y-auto rounded-md border border-border bg-popover shadow-md"
                      >
                        <ComboboxViewport>
                          <ComboboxEmpty class="px-2 py-1.5 text-xs text-muted-foreground">
                            {{ awsRegionsLoaded ? '无匹配结果，回车使用自定义值' : '加载中...' }}
                          </ComboboxEmpty>
                          <ComboboxItem
                            v-for="r in filteredRegions"
                            :key="r"
                            :value="r"
                            class="flex items-center gap-1.5 px-2 py-1.5 text-xs font-mono cursor-pointer rounded-sm outline-none data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground"
                          >
                            <Check
                              class="w-3 h-3 shrink-0"
                              :class="device.region === r ? 'opacity-100' : 'opacity-0'"
                            />
                            {{ r }}
                          </ComboboxItem>
                        </ComboboxViewport>
                      </ComboboxContent>
                    </ComboboxRoot>
                  </div>
                  <div class="space-y-1.5">
                    <label class="text-xs font-medium text-muted-foreground">TOTP Secret (可选, 2FA认证)</label>
                    <input
                      v-model="device.totp_secret"
                      type="text"
                      placeholder="Base32 secret, 如 JBSWY3DPEHPK3PXP"
                      class="w-full h-8 px-2 text-xs rounded-md border border-border bg-background font-mono focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                      spellcheck="false"
                    >
                  </div>
                </div>
              </div>

              <Button
                class="w-full"
                :disabled="device.auth_type === 'identity_center' && !device.start_url.trim()"
                @click="startDeviceAuth"
              >
                开始授权
              </Button>
            </div>

            <!-- 发起中 -->
            <div
              v-else-if="device.starting"
              class="flex items-center justify-center py-12"
            >
              <div class="text-center">
                <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-primary mx-auto mb-3" />
                <p class="text-xs text-muted-foreground">
                  正在注册设备...
                </p>
              </div>
            </div>

            <!-- 等待用户授权 -->
            <template v-else-if="device.session_id && device.status === 'pending'">
              <div class="rounded-xl border border-border bg-muted/20 p-5">
                <div class="flex flex-col items-center text-center space-y-4">
                  <!-- 脉冲动画图标 -->
                  <div class="relative">
                    <div class="absolute inset-0 rounded-full bg-primary/20 animate-ping" />
                    <div class="relative w-10 h-10 rounded-full bg-primary/10 flex items-center justify-center">
                      <ExternalLink class="w-5 h-5 text-primary" />
                    </div>
                  </div>

                  <!-- 提示文字 -->
                  <div class="space-y-1">
                    <p class="text-sm font-medium">
                      在浏览器中完成授权
                    </p>
                    <p class="text-xs text-muted-foreground">
                      授权完成后此页面将自动更新
                    </p>
                  </div>

                  <!-- 倒计时 -->
                  <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                    <div class="animate-spin rounded-full h-3 w-3 border-[1.5px] border-primary/30 border-t-primary" />
                    <span>剩余 {{ deviceCountdownFormatted }}</span>
                  </div>

                  <!-- TOTP 验证码 -->
                  <div
                    v-if="totp.code.value"
                    class="w-full rounded-lg border border-border bg-background p-3"
                  >
                    <div class="flex items-center justify-between">
                      <div class="flex items-center gap-2">
                        <ShieldCheck class="w-3.5 h-3.5 text-primary" />
                        <span class="text-[10px] text-muted-foreground">MFA 验证码</span>
                      </div>
                      <div class="flex items-center gap-1.5">
                        <span
                          class="text-lg font-mono font-bold tracking-[0.25em]"
                        >{{ totp.code.value }}</span>
                        <button
                          class="p-1 rounded hover:bg-muted transition-colors"
                          title="复制验证码"
                          @click="copyToClipboard(totp.code.value)"
                        >
                          <Copy class="w-3 h-3 text-muted-foreground" />
                        </button>
                      </div>
                    </div>
                    <div class="mt-2 flex items-center gap-2">
                      <div class="flex-1 h-1 rounded-full bg-muted overflow-hidden">
                        <div
                          class="h-full rounded-full transition-all duration-1000 ease-linear"
                          :class="totp.remaining.value <= 5 ? 'bg-red-500' : 'bg-primary'"
                          :style="{ width: `${(totp.remaining.value / 30) * 100}%` }"
                        />
                      </div>
                      <span
                        class="text-[10px] font-mono tabular-nums shrink-0"
                        :class="totp.remaining.value <= 5 ? 'text-red-500' : 'text-muted-foreground'"
                      >{{ totp.remaining.value }}s</span>
                    </div>
                  </div>

                  <!-- 操作按钮 -->
                  <div class="flex gap-2 w-full">
                    <Button
                      class="flex-1"
                      size="sm"
                      @click="openDeviceVerificationUrl"
                    >
                      <ExternalLink class="w-3.5 h-3.5 mr-1.5" />
                      打开授权页面
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      @click="copyToClipboard(device.verification_uri_complete)"
                    >
                      <Copy class="w-3.5 h-3.5" />
                    </Button>
                  </div>
                </div>
              </div>
            </template>

            <!-- 错误/过期 -->
            <div
              v-else-if="device.status === 'error' || device.status === 'expired'"
            >
              <div class="rounded-xl border border-destructive/20 bg-destructive/5 p-5">
                <div class="flex flex-col items-center text-center space-y-3">
                  <div class="w-10 h-10 rounded-full bg-destructive/10 flex items-center justify-center">
                    <AlertCircle class="w-5 h-5 text-destructive" />
                  </div>
                  <div class="space-y-1">
                    <p class="text-sm font-medium text-destructive">
                      {{ device.status === 'expired' ? '授权已过期' : '授权失败' }}
                    </p>
                    <p class="text-xs text-muted-foreground">
                      {{ device.error || '请重试' }}
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    @click="resetDevice"
                  >
                    重新开始
                  </Button>
                </div>
              </div>
            </div>
          </template>

          <!-- 非 Kiro: 原有 OAuth 流程 -->
          <template v-else>
            <div
              v-if="oauth.starting && !oauth.authorization_url"
              class="flex items-center justify-center py-12"
            >
              <div class="text-center">
                <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-primary mx-auto mb-3" />
                <p class="text-xs text-muted-foreground">
                  正在准备授权...
                </p>
              </div>
            </div>

            <template v-else-if="oauth.authorization_url">
              <div class="space-y-2">
                <div class="flex items-center gap-2">
                  <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">1</span>
                  <span class="text-xs font-medium">前往授权</span>
                </div>
                <div class="flex gap-2 pl-6">
                  <Button
                    size="sm"
                    :disabled="oauthBusy"
                    @click="openAuthorizationUrl"
                  >
                    <ExternalLink class="w-3 h-3 mr-1" />
                    打开
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    :disabled="oauthBusy"
                    @click="copyToClipboard(oauth.authorization_url)"
                  >
                    <Copy class="w-3 h-3 mr-1" />
                    复制
                  </Button>
                </div>
              </div>

              <div class="space-y-2">
                <div class="flex items-center gap-2">
                  <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">2</span>
                  <span class="text-xs font-medium">粘贴回调 URL</span>
                </div>
                <div class="pl-6">
                  <Textarea
                    v-model="oauth.callback_url"
                    :disabled="oauthBusy"
                    placeholder="http://localhost:xxx/callback?code=..."
                    class="min-h-[120px] text-xs font-mono break-all !rounded-xl"
                    spellcheck="false"
                  />
                </div>
              </div>
            </template>
          </template>
        </div>

        <!-- ===== 导入授权 ===== -->
        <div
          class="flex flex-col gap-3 justify-center transition-opacity duration-150"
          :class="mode === 'import' ? 'opacity-100' : 'opacity-0 pointer-events-none'"
        >
          <JsonImportInput
            v-model="importText"
            :disabled="importing"
            :reset-key="importInputResetKey"
            drop-title="拖入授权文件或点击选择"
            drop-hint="支持 .json / .txt，可多选"
            manual-placeholder="粘贴 Refresh Token / Access Token 或 JSON 内容"
            paste-toggle-text="或手动粘贴 Token"
            file-toggle-text="或选择 JSON 文件导入"
            textarea-class="min-h-[200px] text-xs font-mono break-all !rounded-xl"
            @error="handleImportInputError"
          />

          <div
            v-if="importTask"
            class="rounded-xl border border-border bg-muted/20 p-3 space-y-2"
          >
            <div class="flex items-center justify-between text-xs">
              <span class="font-medium">
                {{ getImportTaskStatusText(importTask.status) }}
              </span>
              <span class="font-mono tabular-nums">
                {{ importTask.progress_percent }}%
              </span>
            </div>
            <div class="h-1.5 rounded-full bg-muted overflow-hidden">
              <div
                class="h-full rounded-full bg-primary transition-all duration-300"
                :style="{ width: `${Math.max(0, Math.min(importTask.progress_percent, 100))}%` }"
              />
            </div>
            <div class="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>进度 {{ importTask.processed }}/{{ importTask.total }}</span>
              <span>成功 {{ importTask.success }} · 失败 {{ importTask.failed }}</span>
            </div>
            <p
              v-if="importTask.message"
              class="text-[11px] text-muted-foreground"
            >
              {{ importTask.message }}
            </p>
            <div
              v-if="importTask.error_samples.length > 0"
              class="space-y-1"
            >
              <p class="text-[11px] text-destructive">
                最近错误
              </p>
              <p
                v-for="item in importTask.error_samples.slice(0, 3)"
                :key="`${item.index}-${item.error || item.status}`"
                class="text-[11px] text-destructive/90"
              >
                #{{ item.index + 1 }} {{ item.error || '导入失败' }}
              </p>
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="handleClose"
      >
        取消
      </Button>
      <Button
        v-if="mode === 'oauth' && !isKiroProvider"
        :disabled="!canCompleteOAuth"
        @click="handleCompleteOAuth"
      >
        {{ oauth.completing ? '验证中...' : '验证' }}
      </Button>
      <Button
        v-if="mode === 'import'"
        :disabled="!canImport"
        @click="handleImport"
      >
        {{ importing ? (importTask ? `导入中 ${importTask.progress_percent}%` : '导入中...') : '导入' }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch, onBeforeUnmount } from 'vue'
import { Dialog, Button, Textarea, Popover, PopoverTrigger, PopoverContent } from '@/components/ui'
import {
  ComboboxAnchor,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxRoot,
  ComboboxTrigger,
  ComboboxViewport,
} from 'radix-vue'
import { UserPlus, Copy, ExternalLink, Globe, AlertCircle, ShieldCheck, ChevronsUpDown, Check } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import { useTotp } from '@/composables/useTotp'
import { parseApiError } from '@/utils/errorParser'
import {
  startProviderLevelOAuth,
  completeProviderLevelOAuth,
  importProviderRefreshToken,
  startBatchImportOAuthTask,
  getBatchImportOAuthTaskStatus,
  startDeviceAuthorize,
  pollDeviceAuthorize,
  getAwsRegions,
} from '@/api/endpoints'
import type {
  OAuthBatchImportTaskStatus,
  OAuthBatchImportTaskStatusResponse,
} from '@/api/endpoints/provider_oauth'
import ProxyNodeSelect from './ProxyNodeSelect.vue'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import JsonImportInput from '@/components/common/JsonImportInput.vue'

const props = defineProps<{
  open: boolean
  providerId: string | null
  providerType: string | null
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, error: showError } = useToast()
const { copyToClipboard } = useClipboard()
const proxyNodesStore = useProxyNodesStore()
const totp = useTotp()

// 代理节点选择
const proxyPopoverOpen = ref(false)
const selectedProxyNodeId = ref('')

// AWS Regions (动态获取 + 进程内缓存)
const awsRegions = ref<string[]>([])
const awsRegionsLoaded = ref(false)
const regionSearch = ref('')
const regionComboboxOpen = ref(false)

const filteredRegions = computed(() => {
  const q = regionSearch.value.trim().toLowerCase()
  if (!q) return awsRegions.value
  return awsRegions.value.filter(r => r.includes(q))
})

async function ensureAwsRegions() {
  if (awsRegionsLoaded.value) return
  try {
    awsRegions.value = await getAwsRegions()
  } catch {
    awsRegions.value = ['us-east-1', 'us-east-2', 'us-west-1', 'us-west-2', 'eu-north-1']
  }
  awsRegionsLoaded.value = true
}

function onRegionEnter() {
  // If no matching item is highlighted, accept the raw input as a custom region value.
  const raw = regionSearch.value.trim()
  if (raw && !filteredRegions.value.includes(raw)) {
    device.value.region = raw
    regionComboboxOpen.value = false
    regionSearch.value = ''
  }
}

/** 获取已选代理节点的显示名称 */
function getSelectedNodeLabel(): string {
  if (!selectedProxyNodeId.value) return ''
  const node = proxyNodesStore.nodes.find(n => n.id === selectedProxyNodeId.value)
  return node ? node.name : `${selectedProxyNodeId.value.slice(0, 8)  }...`
}

// 模式
type DialogMode = 'oauth' | 'import'
const mode = ref<DialogMode>('oauth')

// OAuth 状态
interface OAuthState {
  authorization_url: string
  redirect_uri: string
  instructions: string
  provider_type: string
  callback_url: string
  starting: boolean
  completing: boolean
}

function createInitialOAuthState(): OAuthState {
  return {
    authorization_url: '',
    redirect_uri: '',
    instructions: '',
    provider_type: '',
    callback_url: '',
    starting: false,
    completing: false,
  }
}

const oauth = ref<OAuthState>(createInitialOAuthState())
let oauthInitRequestId = 0
let oauthCompleteRequestId = 0

// 设备授权状态
type DeviceAuthType = 'builder_id' | 'identity_center'

interface DeviceAuthState {
  auth_type: DeviceAuthType
  start_url: string
  region: string
  totp_secret: string
  starting: boolean
  session_id: string
  user_code: string
  verification_uri: string
  verification_uri_complete: string
  expires_at: number  // unix timestamp (ms)
  interval: number    // 轮询间隔 (秒)
  status: 'idle' | 'pending' | 'authorized' | 'expired' | 'error'
  error: string
}

const BUILDER_ID_START_URL = 'https://view.awsapps.com/start'
const BUILDER_ID_REGION = 'us-east-1'

function createInitialDeviceState(): DeviceAuthState {
  return {
    auth_type: 'builder_id',
    start_url: '',
    region: 'eu-north-1',
    totp_secret: '',
    starting: false,
    session_id: '',
    user_code: '',
    verification_uri: '',
    verification_uri_complete: '',
    expires_at: 0,
    interval: 5,
    status: 'idle',
    error: '',
  }
}

const device = ref<DeviceAuthState>(createInitialDeviceState())
let devicePollTimer: ReturnType<typeof setTimeout> | null = null
const deviceCountdown = ref(0)
let countdownTimer: ReturnType<typeof setInterval> | null = null

// 导入状态
const importText = ref('')
const importing = ref(false)
const importInputResetKey = ref(0)
const importTask = ref<OAuthBatchImportTaskStatusResponse | null>(null)
let importPollTimer: ReturnType<typeof setTimeout> | null = null
const importPolling = ref(false)

const isOpen = computed(() => props.open)

const isKiroProvider = computed(() => (props.providerType || '').toLowerCase() === 'kiro')

const deviceCountdownFormatted = computed(() => {
  const s = deviceCountdown.value
  const min = Math.floor(s / 60)
  const sec = s % 60
  return `${min}:${String(sec).padStart(2, '0')}`
})

const oauthBusy = computed(() =>
  oauth.value.starting || oauth.value.completing
)

const canCompleteOAuth = computed(() => {
  if (!oauth.value.authorization_url) return false
  if (!oauth.value.callback_url.trim()) return false
  return !oauthBusy.value
})

const canImport = computed(() => {
  return importText.value.trim().length > 0 && !importing.value
})

function stopImportPolling() {
  if (importPollTimer) {
    clearTimeout(importPollTimer)
    importPollTimer = null
  }
  importPolling.value = false
}

function getImportTaskStatusText(status: OAuthBatchImportTaskStatus): string {
  switch (status) {
    case 'submitted':
      return '任务已提交'
    case 'processing':
      return '正在导入'
    case 'completed':
      return '导入完成'
    case 'failed':
      return '导入失败'
    default:
      return '处理中'
  }
}

function getOAuthSuccessMessage(
  action: '授权' | '导入',
  options?: { email?: string | null; replaced?: boolean }
): string {
  const email = typeof options?.email === 'string' ? options.email.trim() : ''
  const replaced = options?.replaced === true
  if (email) {
    return replaced
      ? `${action}成功: ${email}（已替换旧账号）`
      : `${action}成功: ${email}`
  }
  return replaced
    ? `${action}成功，已替换旧账号`
    : `${action}成功，账号已添加`
}

function getBatchImportSuccessMessage(task: OAuthBatchImportTaskStatusResponse): string {
  const replacedCount = Math.max(task.replaced_count ?? 0, 0)
  const createdCount = Math.max(task.created_count ?? task.success - replacedCount, 0)
  const parts: string[] = []

  if (createdCount > 0) {
    parts.push(`新增 ${createdCount} 个`)
  }
  if (replacedCount > 0) {
    parts.push(`替换 ${replacedCount} 个`)
  }
  if (task.failed > 0) {
    parts.push(`失败 ${task.failed} 个`)
  }

  if (parts.length === 0) {
    return task.failed > 0 ? `批量导入完成：失败 ${task.failed} 个` : '批量导入完成'
  }
  if (task.failed === 0 && createdCount > 0 && replacedCount === 0) {
    return `批量导入成功：${createdCount} 个账号已添加`
  }
  if (task.failed === 0 && createdCount === 0 && replacedCount > 0) {
    return `批量导入成功：已替换 ${replacedCount} 个旧账号`
  }

  const prefix = task.failed > 0 ? '批量导入完成' : '批量导入成功'
  return `${prefix}：${parts.join('，')}`
}

function scheduleImportPoll(taskId: string, delayMs = 1200) {
  stopImportPolling()
  importPollTimer = setTimeout(() => {
    void pollImportTaskStatus(taskId)
  }, delayMs)
}

async function pollImportTaskStatus(taskId: string) {
  if (!props.providerId || importPolling.value) return

  importPolling.value = true
  try {
    const task = await getBatchImportOAuthTaskStatus(props.providerId, taskId)
    importTask.value = task

    if (task.status === 'completed') {
      stopImportPolling()
      importing.value = false
      if (task.success > 0) {
        success(getBatchImportSuccessMessage(task))
        emit('saved')
        handleClose()
      } else {
        showError(task.error || '批量导入失败', '导入失败')
      }
      return
    }

    if (task.status === 'failed') {
      stopImportPolling()
      importing.value = false
      showError(task.error || task.message || '批量导入失败', '导入失败')
      return
    }

    scheduleImportPoll(taskId)
  } catch {
    if (importing.value) {
      scheduleImportPoll(taskId, 2000)
    }
  } finally {
    importPolling.value = false
  }
}

function stopDevicePolling() {
  if (devicePollTimer) {
    clearTimeout(devicePollTimer)
    devicePollTimer = null
  }
  if (countdownTimer) {
    clearInterval(countdownTimer)
    countdownTimer = null
  }
}

function resetDevice() {
  stopDevicePolling()
  totp.stop()
  const { auth_type, start_url, region, totp_secret } = device.value
  device.value = createInitialDeviceState()
  device.value.auth_type = auth_type
  device.value.start_url = start_url
  device.value.region = region
  device.value.totp_secret = totp_secret
}

function resetForm() {
  oauthInitRequestId += 1
  oauthCompleteRequestId += 1
  oauth.value = createInitialOAuthState()
  stopImportPolling()
  stopDevicePolling()
  totp.stop()
  device.value = createInitialDeviceState()
  importText.value = ''
  importing.value = false
  importTask.value = null
  importInputResetKey.value += 1
  proxyPopoverOpen.value = false
  selectedProxyNodeId.value = ''
  mode.value = 'oauth'
}

function switchMode(newMode: DialogMode) {
  if (mode.value === newMode) return

  mode.value = newMode
  if (newMode === 'oauth' && !isKiroProvider.value && !oauth.value.authorization_url && !oauth.value.starting) {
    initOAuth()
  }
}

function handleDialogUpdate(value: boolean) {
  if (!value) {
    handleClose()
  }
}

function handleClose() {
  resetForm()
  emit('close')
}

function openAuthorizationUrl() {
  const url = oauth.value.authorization_url
  if (!url) return
  window.open(url, '_blank', 'noopener,noreferrer')
}

async function initOAuth() {
  if (!props.providerId) return
  if (isKiroProvider.value) return
  if (oauth.value.starting) return

  const requestId = ++oauthInitRequestId
  oauth.value.starting = true
  try {
    const resp = await startProviderLevelOAuth(props.providerId)
    if (requestId !== oauthInitRequestId) return
    oauth.value.authorization_url = resp.authorization_url
    oauth.value.redirect_uri = resp.redirect_uri
    oauth.value.instructions = resp.instructions
    oauth.value.provider_type = resp.provider_type
  } catch (err: unknown) {
    if (requestId !== oauthInitRequestId) return
    const errorMessage = parseApiError(err, '初始化授权失败')
    showError(errorMessage, '错误')
    mode.value = 'import'
  } finally {
    if (requestId === oauthInitRequestId) {
      oauth.value.starting = false
    }
  }
}

async function handleCompleteOAuth() {
  if (oauth.value.completing) return
  if (!canCompleteOAuth.value || !props.providerId) return
  const requestId = ++oauthCompleteRequestId
  oauth.value.completing = true
  try {
    const result = await completeProviderLevelOAuth(props.providerId, {
      callback_url: oauth.value.callback_url.trim(),
      proxy_node_id: selectedProxyNodeId.value || undefined,
    })
    if (requestId !== oauthCompleteRequestId) return
    success(getOAuthSuccessMessage('授权', result))
    emit('saved')
    handleClose()
  } catch (err: unknown) {
    if (requestId !== oauthCompleteRequestId) return
    const errorMessage = parseApiError(err, '完成授权失败')
    showError(errorMessage, '错误')
  } finally {
    if (requestId === oauthCompleteRequestId) {
      oauth.value.completing = false
    }
  }
}

// 检测是否为批量导入格式
function isBatchImport(text: string): boolean {
  const trimmed = text.trim()
  // JSON 数组（含单元素数组）
  if (trimmed.startsWith('[')) {
    try {
      const parsed = JSON.parse(trimmed)
      return Array.isArray(parsed) && parsed.length >= 1
    } catch {
      return false
    }
  }
  // 单个 JSON 对象（可能是 pretty-printed 多行）不算批量导入
  if (trimmed.startsWith('{')) {
    try {
      JSON.parse(trimmed)
      return false // 可解析的单个 JSON 对象，走单条导入
    } catch {
      // 解析失败：可能是多个 JSON 对象（JSON Lines 格式），继续检查
    }
  }
  // 多行文本（纯 Token 一行一个）
  const lines = trimmed.split('\n').filter(line => line.trim() && !line.trim().startsWith('#'))
  return lines.length > 1
}

function parseImportText(text: string): { refresh_token?: string; access_token?: string; name?: string } | null {
  const trimmed = text.trim()
  if (!trimmed) return null

  // Kiro: keep full JSON so backend can extract auth_method/region/client_id, etc.
  if (isKiroProvider.value) {
    return { refresh_token: trimmed }
  }

  try {
    const parsed: unknown = JSON.parse(trimmed)
    if (typeof parsed === 'object' && parsed !== null) {
      const obj = parsed as Record<string, unknown>
      const refreshToken = obj.refresh_token
      const refreshTokenCamel = obj.refreshToken
      const accessToken = obj.access_token
      const accessTokenCamel = obj.accessToken
      const normalizedRefreshToken = typeof refreshToken === 'string' && refreshToken.trim()
        ? refreshToken.trim()
        : (typeof refreshTokenCamel === 'string' && refreshTokenCamel.trim() ? refreshTokenCamel.trim() : undefined)
      const normalizedAccessToken = typeof accessToken === 'string' && accessToken.trim()
        ? accessToken.trim()
        : (typeof accessTokenCamel === 'string' && accessTokenCamel.trim() ? accessTokenCamel.trim() : undefined)
      if (normalizedRefreshToken || normalizedAccessToken) {
        return {
          refresh_token: normalizedRefreshToken,
          access_token: normalizedAccessToken,
          name: (typeof obj.name === 'string' ? obj.name : undefined) || (typeof obj.oauth_email === 'string' ? obj.oauth_email : undefined),
        }
      }
      return null
    }
  } catch {
    // Not JSON: treat as raw token.
  }

  if (isLikelyJwtToken(trimmed)) {
    return { access_token: trimmed }
  }

  return { refresh_token: trimmed }
}

function isLikelyJwtToken(token: string): boolean {
  const parts = token.trim().split('.')
  if (parts.length !== 3 || parts.some(part => !part)) return false

  try {
    const header = JSON.parse(decodeBase64Url(parts[0])) as Record<string, unknown>
    const payload = JSON.parse(decodeBase64Url(parts[1])) as Record<string, unknown>
    const tokenType = typeof header.typ === 'string' ? header.typ.toLowerCase() : ''
    if (tokenType && tokenType !== 'jwt' && tokenType !== 'at+jwt') return false
    return ['exp', 'aud', 'iss', 'scope', 'scp'].some(key => key in payload)
  } catch {
    return false
  }
}

function decodeBase64Url(value: string): string {
  const normalized = value.replace(/-/g, '+').replace(/_/g, '/')
  const padded = normalized.padEnd(normalized.length + ((4 - (normalized.length % 4)) % 4), '=')
  return atob(padded)
}

function handleImportInputError(payload: { message: string; title?: string }) {
  showError(payload.message, payload.title)
}

async function handleImport() {
  if (!canImport.value || !props.providerId) return

  const inputText = importText.value.trim()
  if (!inputText) {
    showError('请输入凭据数据', '格式错误')
    return
  }

  importing.value = true
  let keepImporting = false
  try {
    const proxyNodeId = selectedProxyNodeId.value || undefined
    // Kiro 的单条 JSON 凭据也必须走 batch-import 路径，后端需要完整 auth_config。
    if (isKiroProvider.value || isBatchImport(inputText)) {
      const task = await startBatchImportOAuthTask(props.providerId, inputText, proxyNodeId)
      importTask.value = {
        task_id: task.task_id,
        provider_id: props.providerId,
        provider_type: props.providerType || '',
        status: task.status,
        total: task.total,
        processed: task.processed,
        success: task.success,
        failed: task.failed,
        created_count: task.created_count ?? 0,
        replaced_count: task.replaced_count ?? 0,
        progress_percent: task.progress_percent,
        message: task.message || null,
        error: null,
        error_samples: [],
        created_at: Math.floor(Date.now() / 1000),
        started_at: null,
        finished_at: null,
        updated_at: Math.floor(Date.now() / 1000),
      }
      keepImporting = true
      scheduleImportPoll(task.task_id, 400)
    } else {
      // 单条导入
      const parsed = parseImportText(inputText)
      if (!parsed) {
        showError('无法解析输入内容，请检查格式', '格式错误')
        return
      }
      const result = await importProviderRefreshToken(props.providerId, {
        ...parsed,
        proxy_node_id: proxyNodeId,
      })
      success(getOAuthSuccessMessage('导入', result))
      emit('saved')
      handleClose()
    }
  } catch (err: unknown) {
    const errorMessage = parseApiError(err, '导入失败')
    showError(errorMessage, '错误')
  } finally {
    if (!keepImporting) {
      importing.value = false
    }
  }
}

// ==== 设备授权 ====

function openDeviceVerificationUrl() {
  const url = device.value.verification_uri_complete || device.value.verification_uri
  if (url) window.open(url, '_blank', 'noopener,noreferrer')
}

function startCountdown() {
  if (countdownTimer) clearInterval(countdownTimer)
  deviceCountdown.value = Math.max(0, Math.round((device.value.expires_at - Date.now()) / 1000))
  countdownTimer = setInterval(() => {
    deviceCountdown.value = Math.max(0, Math.round((device.value.expires_at - Date.now()) / 1000))
    if (deviceCountdown.value <= 0 && countdownTimer) {
      clearInterval(countdownTimer)
      countdownTimer = null
    }
  }, 1000)
}

async function startDeviceAuth() {
  if (!props.providerId) return
  device.value.starting = true
  device.value.error = ''
  try {
    const isBuilderID = device.value.auth_type === 'builder_id'
    const resp = await startDeviceAuthorize(props.providerId, {
      start_url: isBuilderID ? BUILDER_ID_START_URL : (device.value.start_url.trim() || undefined),
      region: isBuilderID ? BUILDER_ID_REGION : (device.value.region.trim() || undefined),
      proxy_node_id: selectedProxyNodeId.value || undefined,
    })
    device.value.session_id = resp.session_id
    device.value.user_code = resp.user_code
    device.value.verification_uri = resp.verification_uri
    device.value.verification_uri_complete = resp.verification_uri_complete
    device.value.expires_at = Date.now() + resp.expires_in * 1000
    device.value.interval = resp.interval || 5
    device.value.status = 'pending'
    startCountdown()
    scheduleDevicePoll()
    // 如果配置了 TOTP secret，启动验证码生成
    if (device.value.totp_secret.trim()) {
      totp.start(device.value.totp_secret.trim())
    }
  } catch (err: unknown) {
    const errorMessage = parseApiError(err, '发起设备授权失败')
    showError(errorMessage, '错误')
    device.value.status = 'error'
    device.value.error = errorMessage
  } finally {
    device.value.starting = false
  }
}

function scheduleDevicePoll() {
  if (devicePollTimer) clearTimeout(devicePollTimer)
  devicePollTimer = setTimeout(() => pollDevice(), device.value.interval * 1000)
}

async function pollDevice() {
  if (!props.providerId || !device.value.session_id || device.value.status !== 'pending') return

  try {
    const result = await pollDeviceAuthorize(props.providerId, {
      session_id: device.value.session_id,
    })

    switch (result.status) {
      case 'authorized':
        stopDevicePolling()
        totp.stop()
        device.value.status = 'authorized'
        success(getOAuthSuccessMessage('授权', result))
        emit('saved')
        handleClose()
        return
      case 'pending':
        scheduleDevicePoll()
        return
      case 'slow_down':
        device.value.interval = Math.min(device.value.interval + 5, 30)
        scheduleDevicePoll()
        return
      case 'expired':
        stopDevicePolling()
        device.value.status = 'expired'
        device.value.error = result.error || '设备码已过期'
        return
      case 'error':
        stopDevicePolling()
        device.value.status = 'error'
        device.value.error = result.error || '授权失败'
        return
    }
  } catch {
    // 网络错误等，继续轮询
    scheduleDevicePoll()
  }
}

onBeforeUnmount(() => {
  stopImportPolling()
  stopDevicePolling()
})

watch(() => props.open, (newOpen) => {
  if (newOpen) {
    proxyNodesStore.ensureLoaded()
    if (!isKiroProvider.value) {
      initOAuth()
    }
  } else {
    resetForm()
  }
})
</script>
