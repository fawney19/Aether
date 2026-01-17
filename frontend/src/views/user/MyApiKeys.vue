<template>
  <div class="space-y-6 pb-8">
    <!-- API Keys 表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和操作栏 -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
          <h3 class="text-sm sm:text-base font-semibold shrink-0">
            我的 API Keys
          </h3>

          <!-- 操作按钮 -->
          <div class="flex items-center gap-2">
            <!-- 新增 API Key 按钮 -->
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="创建新 API Key"
              @click="showCreateDialog = true"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>

            <!-- 刷新按钮 -->
            <RefreshButton
              :loading="loading"
              @click="loadApiKeys"
            />
          </div>
        </div>
      </div>

      <!-- 加载状态 -->
      <div
        v-if="loading"
        class="flex items-center justify-center py-12"
      >
        <LoadingState message="加载中..." />
      </div>

      <!-- 空状态 -->
      <div
        v-else-if="apiKeys.length === 0"
        class="flex items-center justify-center py-12"
      >
        <EmptyState
          title="暂无 API 密钥"
          description="创建你的第一个 API 密钥开始使用"
          :icon="Key"
        >
          <template #actions>
            <Button
              size="lg"
              class="shadow-lg shadow-primary/20"
              @click="showCreateDialog = true"
            >
              <Plus class="mr-2 h-4 w-4" />
              创建新 API Key
            </Button>
          </template>
        </EmptyState>
      </div>

      <!-- 桌面端表格 -->
      <div
        v-else
        class="hidden md:block overflow-x-auto"
      >
        <Table>
          <TableHeader>
            <TableRow class="border-b border-border/60 hover:bg-transparent">
              <TableHead class="min-w-[200px] h-12 font-semibold">
                密钥名称
              </TableHead>
              <TableHead class="min-w-[80px] h-12 font-semibold">
                能力
              </TableHead>
              <TableHead class="min-w-[160px] h-12 font-semibold">
                密钥
              </TableHead>
              <TableHead class="min-w-[100px] h-12 font-semibold">
                费用(USD)
              </TableHead>
              <TableHead class="min-w-[100px] h-12 font-semibold">
                请求次数
              </TableHead>
              <TableHead class="min-w-[70px] h-12 font-semibold text-center">
                状态
              </TableHead>
              <TableHead class="min-w-[100px] h-12 font-semibold">
                最后使用
              </TableHead>
              <TableHead class="min-w-[80px] h-12 font-semibold text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="apiKey in paginatedApiKeys"
              :key="apiKey.id"
              class="border-b border-border/40 hover:bg-muted/30 transition-colors"
            >
              <!-- 密钥名称 -->
              <TableCell class="py-4">
                <div class="flex-1 min-w-0">
                  <div
                    class="text-sm font-semibold truncate"
                    :title="apiKey.name"
                  >
                    {{ apiKey.name }}
                  </div>
                  <div class="text-xs text-muted-foreground mt-0.5">
                    创建于 {{ formatDate(apiKey.created_at) }}
                  </div>
                </div>
              </TableCell>

              <!-- 能力 -->
              <TableCell class="py-4">
                <div class="flex gap-1.5 flex-wrap items-center">
                  <template v-if="userConfigurableCapabilities.length > 0">
                    <button
                      v-for="cap in userConfigurableCapabilities"
                      :key="cap.name"
                      class="inline-flex items-center gap-1 px-2 py-1 rounded-full text-xs font-medium transition-all"
                      :class="[
                        apiKey.is_locked ? 'opacity-50 cursor-not-allowed' : '',
                        isCapabilityEnabled(apiKey, cap.name)
                          ? 'bg-primary text-primary-foreground'
                          : 'bg-transparent text-muted-foreground border border-dashed border-muted-foreground/50 hover:border-primary/50 hover:text-foreground'
                      ]"
                      :title="apiKey.is_locked ? '已锁定' : getCapabilityTooltip(cap, isCapabilityEnabled(apiKey, cap.name))"
                      :disabled="apiKey.is_locked"
                      @click.stop="!apiKey.is_locked && toggleCapability(apiKey, cap.name)"
                    >
                      <Check
                        v-if="isCapabilityEnabled(apiKey, cap.name)"
                        class="w-3 h-3"
                      />
                      <Plus
                        v-else
                        class="w-3 h-3"
                      />
                      {{ cap.short_name || cap.display_name }}
                    </button>
                  </template>
                  <span
                    v-else
                    class="text-muted-foreground text-xs"
                  >-</span>
                </div>
              </TableCell>

              <!-- 密钥显示 -->
              <TableCell class="py-4">
                <div class="flex items-center gap-1.5">
                  <code class="text-xs font-mono text-muted-foreground bg-muted/30 px-2 py-1 rounded">
                    {{ apiKey.key_display || 'sk-••••••••' }}
                  </code>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-6 w-6"
                    title="复制完整密钥"
                    @click="copyApiKey(apiKey)"
                  >
                    <Copy class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </TableCell>

              <!-- 费用 -->
              <TableCell class="py-4">
                <span class="text-sm font-semibold text-amber-600 dark:text-amber-500">
                  ${{ (apiKey.total_cost_usd || 0).toFixed(4) }}
                </span>
              </TableCell>

              <!-- 请求次数 -->
              <TableCell class="py-4">
                <div class="flex items-center gap-1.5">
                  <Activity class="h-3.5 w-3.5 text-muted-foreground" />
                  <span class="text-sm font-medium text-foreground">
                    {{ formatNumber(apiKey.total_requests || 0) }}
                  </span>
                </div>
              </TableCell>

              <!-- 状态 -->
              <TableCell class="py-4 text-center">
                <div class="flex flex-col items-center gap-1">
                  <Badge
                    :variant="apiKey.is_active ? 'success' : 'secondary'"
                    class="font-medium px-3 py-1"
                  >
                    {{ apiKey.is_active ? '活跃' : '禁用' }}
                  </Badge>
                  <Badge
                    v-if="apiKey.is_locked"
                    variant="warning"
                    class="font-medium text-[10px]"
                  >
                    已锁定
                  </Badge>
                </div>
              </TableCell>

              <!-- 最后使用时间 -->
              <TableCell class="py-4 text-sm text-muted-foreground">
                {{ apiKey.last_used_at ? formatRelativeTime(apiKey.last_used_at) : '从未使用' }}
              </TableCell>

              <!-- 操作按钮 -->
              <TableCell class="py-4">
                <div class="flex justify-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="apiKey.is_locked ? '已锁定' : (apiKey.is_active ? '禁用' : '启用')"
                    :disabled="apiKey.is_locked"
                    @click="toggleApiKey(apiKey)"
                  >
                    <Power class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="apiKey.is_locked ? '已锁定' : '删除'"
                    :disabled="apiKey.is_locked"
                    @click="confirmDelete(apiKey)"
                  >
                    <Trash2 class="h-4 w-4" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>

      <!-- 移动端卡片列表 -->
      <div
        v-if="!loading && apiKeys.length > 0"
        class="md:hidden space-y-3 p-4"
      >
        <Card
          v-for="apiKey in paginatedApiKeys"
          :key="apiKey.id"
          variant="default"
          class="group hover:shadow-md hover:border-primary/30 transition-all duration-200"
        >
          <div class="p-4">
            <!-- 第一行：名称、状态、操作 -->
            <div class="flex items-center justify-between mb-2">
              <div class="flex items-center gap-2 min-w-0 flex-1">
                <h3 class="text-sm font-semibold text-foreground truncate">
                  {{ apiKey.name }}
                </h3>
                <Badge
                  :variant="apiKey.is_active ? 'success' : 'secondary'"
                  class="text-xs px-1.5 py-0"
                >
                  {{ apiKey.is_active ? '活跃' : '禁用' }}
                </Badge>
                <Badge
                  v-if="apiKey.is_locked"
                  variant="warning"
                  class="text-[10px] px-1.5 py-0"
                >
                  已锁定
                </Badge>
              </div>
              <div class="flex items-center gap-0.5 flex-shrink-0">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  title="复制"
                  @click="copyApiKey(apiKey)"
                >
                  <Copy class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="apiKey.is_locked ? '已锁定' : (apiKey.is_active ? '禁用' : '启用')"
                  :disabled="apiKey.is_locked"
                  @click="toggleApiKey(apiKey)"
                >
                  <Power class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="apiKey.is_locked ? '已锁定' : '删除'"
                  :disabled="apiKey.is_locked"
                  @click="confirmDelete(apiKey)"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>

            <!-- 第二行：密钥、时间、统计 -->
            <div class="space-y-1.5">
              <div class="flex items-center gap-2 text-xs">
                <code class="font-mono text-muted-foreground">{{ apiKey.key_display || 'sk-••••••••' }}</code>
                <span class="text-muted-foreground">•</span>
                <span class="text-muted-foreground">
                  {{ apiKey.last_used_at ? formatRelativeTime(apiKey.last_used_at) : '从未使用' }}
                </span>
              </div>
              <div class="flex items-center gap-3 text-xs">
                <span class="text-amber-600 dark:text-amber-500 font-semibold">
                  ${{ (apiKey.total_cost_usd || 0).toFixed(4) }}
                </span>
                <span class="text-muted-foreground">•</span>
                <span class="text-foreground font-medium">
                  {{ formatNumber(apiKey.total_requests || 0) }} 次
                </span>
              </div>
            </div>
          </div>
        </Card>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="apiKeys.length > 0"
        :current="currentPage"
        :total="apiKeys.length"
        :page-size="pageSize"
        cache-key="my-api-keys-page-size"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>

    <!-- 创建 API 密钥对话框 -->
    <Dialog v-model="showCreateDialog">
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
              <Key class="h-5 w-5 text-primary" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                创建 API 密钥
              </h3>
              <p class="text-xs text-muted-foreground">
                创建一个新的密钥用于访问 API 服务
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label
            for="key-name"
            class="text-sm font-semibold"
          >密钥名称</Label>
          <Input
            id="key-name"
            v-model="newKeyName"
            placeholder="例如：生产环境密钥"
            class="h-11 border-border/60"
            autocomplete="off"
            required
          />
          <p class="text-xs text-muted-foreground">
            给密钥起一个有意义的名称方便识别
          </p>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-11 px-6"
          @click="showCreateDialog = false"
        >
          取消
        </Button>
        <Button
          class="h-11 px-6 shadow-lg shadow-primary/20"
          :disabled="creating"
          @click="createApiKey"
        >
          <Loader2
            v-if="creating"
            class="animate-spin h-4 w-4 mr-2"
          />
          {{ creating ? '创建中...' : '创建' }}
        </Button>
      </template>
    </Dialog>

    <!-- 新密钥创建成功对话框 -->
    <Dialog
      v-model="showKeyDialog"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-emerald-100 dark:bg-emerald-900/30 flex-shrink-0">
              <CheckCircle class="h-5 w-5 text-emerald-600 dark:text-emerald-400" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                创建成功
              </h3>
              <p class="text-xs text-muted-foreground">
                请妥善保管, 切勿泄露给他人
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label class="text-sm font-medium">API 密钥</Label>
          <div class="flex items-center gap-2">
            <Input
              type="text"
              :value="newKeyValue"
              readonly
              class="flex-1 font-mono text-sm bg-muted/50 h-11"
              @click="($event.target as HTMLInputElement)?.select()"
            />
            <Button
              class="h-11"
              @click="copyTextToClipboard(newKeyValue)"
            >
              复制
            </Button>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          class="h-10 px-5"
          @click="showKeyDialog = false"
        >
          确定
        </Button>
      </template>
    </Dialog>

    <!-- 删除确认对话框 -->
    <AlertDialog
      v-model="showDeleteDialog"
      type="danger"
      title="确认删除"
      :description="`确定要删除密钥 &quot;${keyToDelete?.name}&quot; 吗？此操作不可恢复。`"
      confirm-text="删除"
      :loading="deleting"
      @confirm="deleteApiKey"
      @cancel="showDeleteDialog = false"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { meApi, type ApiKey } from '@/api/me'
import { getAllCapabilities, type CapabilityDefinition } from '@/api/endpoints'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Badge from '@/components/ui/badge.vue'
import { Dialog, Pagination, getPageSizeCache } from '@/components/ui'
import { LoadingState, AlertDialog, EmptyState } from '@/components/common'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from '@/components/ui'
import RefreshButton from '@/components/ui/refresh-button.vue'
import { Plus, Key, Copy, Trash2, Loader2, Activity, CheckCircle, Power, Check } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { log } from '@/utils/logger'
import { computed } from 'vue'

const { success, error: showError } = useToast()

const apiKeys = ref<ApiKey[]>([])
const loading = ref(false)
const creating = ref(false)
const deleting = ref(false)

// 分页相关
const currentPage = ref(1)
const pageSize = ref(getPageSizeCache('my-api-keys-page-size', 10))

const paginatedApiKeys = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return apiKeys.value.slice(start, start + pageSize.value)
})

const showCreateDialog = ref(false)
const showKeyDialog = ref(false)
const showDeleteDialog = ref(false)

const newKeyName = ref('')
const newKeyValue = ref('')
const keyToDelete = ref<ApiKey | null>(null)

// 能力配置相关
const availableCapabilities = ref<CapabilityDefinition[]>([])
const userConfigurableCapabilities = computed(() =>
  availableCapabilities.value.filter(cap => cap.config_mode === 'user_configurable')
)
const savingCapability = ref<string | null>(null) // 正在保存的能力标识 "keyId:capName"

onMounted(() => {
  loadApiKeys()
  loadCapabilities()
})

async function loadCapabilities() {
  try {
    availableCapabilities.value = await getAllCapabilities()
  } catch (error) {
    log.error('Failed to load capabilities:', error)
  }
}

async function loadApiKeys() {
  loading.value = true
  try {
    apiKeys.value = await meApi.getApiKeys()
  } catch (error: any) {
    log.error('加载 API 密钥失败:', error)
    if (!error.response) {
      showError('无法连接到服务器，请检查后端服务是否运行')
    } else if (error.response.status === 401) {
      showError('认证失败，请重新登录')
    } else {
      showError(`加载 API 密钥失败：${  error.response?.data?.detail || error.message}`)
    }
  } finally {
    loading.value = false
  }
}

async function createApiKey() {
  if (!newKeyName.value.trim()) {
    showError('请输入密钥名称')
    return
  }

  creating.value = true
  try {
    const newKey = await meApi.createApiKey(newKeyName.value)
    newKeyValue.value = newKey.key || ''
    showCreateDialog.value = false
    showKeyDialog.value = true
    newKeyName.value = ''
    await loadApiKeys()
    success('API 密钥创建成功')
  } catch (error) {
    log.error('创建 API 密钥失败:', error)
    showError('创建 API 密钥失败')
  } finally {
    creating.value = false
  }
}

function confirmDelete(apiKey: ApiKey) {
  keyToDelete.value = apiKey
  showDeleteDialog.value = true
}

async function deleteApiKey() {
  if (!keyToDelete.value) return

  deleting.value = true
  try {
    await meApi.deleteApiKey(keyToDelete.value.id)
    apiKeys.value = apiKeys.value.filter(k => k.id !== keyToDelete.value!.id)
    showDeleteDialog.value = false
    success('API 密钥已删除')
  } catch (error) {
    log.error('删除 API 密钥失败:', error)
    showError('删除 API 密钥失败')
  } finally {
    deleting.value = false
    keyToDelete.value = null
  }
}

async function toggleApiKey(apiKey: ApiKey) {
  try {
    const updated = await meApi.toggleApiKey(apiKey.id)
    const index = apiKeys.value.findIndex(k => k.id === apiKey.id)
    if (index !== -1) {
      apiKeys.value[index].is_active = updated.is_active
    }
    success(updated.is_active ? '密钥已启用' : '密钥已禁用')
  } catch (error) {
    log.error('切换密钥状态失败:', error)
    showError('操作失败')
  }
}

// 检查某个能力是否已启用
function isCapabilityEnabled(apiKey: ApiKey, capName: string): boolean {
  return apiKey.force_capabilities?.[capName] || false
}

// 切换能力配置
async function toggleCapability(apiKey: ApiKey, capName: string) {
  const capKey = `${apiKey.id}:${capName}`
  if (savingCapability.value === capKey) return // 防止重复点击

  savingCapability.value = capKey
  try {
    const currentEnabled = isCapabilityEnabled(apiKey, capName)
    const newEnabled = !currentEnabled

    // 构建新的能力配置
    const newCapabilities: Record<string, boolean> = { ...(apiKey.force_capabilities || {}) }

    if (newEnabled) {
      newCapabilities[capName] = true
    } else {
      delete newCapabilities[capName]
    }

    const capabilitiesData = Object.keys(newCapabilities).length > 0 ? newCapabilities : null

    // 调用 API 保存
    await meApi.updateApiKeyCapabilities(apiKey.id, {
      force_capabilities: capabilitiesData
    })

    // 更新本地数据
    const index = apiKeys.value.findIndex(k => k.id === apiKey.id)
    if (index !== -1) {
      apiKeys.value[index].force_capabilities = capabilitiesData
    }
  } catch (err) {
    log.error('保存能力配置失败:', err)
    showError('保存失败，请重试')
  } finally {
    savingCapability.value = null
  }
}

async function copyApiKey(apiKey: ApiKey) {
  try {
    // 调用后端 API 获取完整密钥
    const response = await meApi.getFullApiKey(apiKey.id)
    await copyTextToClipboard(response.key, false) // 不显示内部提示
    success('完整密钥已复制到剪贴板')
  } catch (error) {
    log.error('复制密钥失败:', error)
    showError('复制失败，请重试')
  }
}

async function copyTextToClipboard(text: string, showToast: boolean = true) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      if (showToast) success('已复制到剪贴板')
    } else {
      const textArea = document.createElement('textarea')
      textArea.value = text
      textArea.style.position = 'fixed'
      textArea.style.left = '-999999px'
      textArea.style.top = '-999999px'
      document.body.appendChild(textArea)
      textArea.focus()
      textArea.select()

      try {
        const successful = document.execCommand('copy')
        if (successful && showToast) {
          success('已复制到剪贴板')
        } else if (!successful) {
          showError('复制失败，请手动复制')
        }
      } finally {
        document.body.removeChild(textArea)
      }
    }
  } catch (error) {
    log.error('复制失败:', error)
    showError('复制失败，请手动选择文本进行复制')
  }
}

function formatNumber(num: number | undefined | null): string {
  if (num === undefined || num === null) {
    return '0'
  }
  return num.toLocaleString('zh-CN')
}

function formatDate(dateString: string): string {
  const date = new Date(dateString)
  return date.toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit'
  })
}

function formatRelativeTime(dateString: string): string {
  const date = new Date(dateString)
  const now = new Date()
  const diffMs = now.getTime() - date.getTime()
  const diffMins = Math.floor(diffMs / (1000 * 60))
  const diffHours = Math.floor(diffMs / (1000 * 60 * 60))
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24))

  if (diffMins < 1) return '刚刚'
  if (diffMins < 60) return `${diffMins}分钟前`
  if (diffHours < 24) return `${diffHours}小时前`
  if (diffDays < 7) return `${diffDays}天前`

  return formatDate(dateString)
}

// 获取能力按钮的提示文字
function getCapabilityTooltip(cap: CapabilityDefinition, isEnabled: boolean): string {
  if (isEnabled) {
    return `[已启用] 此密钥只能访问支持${cap.display_name}的模型`
  }
  return `${cap.description}`
}

</script>
