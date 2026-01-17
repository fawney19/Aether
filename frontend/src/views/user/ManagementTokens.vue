<template>
  <div class="space-y-6 pb-8">
    <!-- 访问令牌表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和操作栏 -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
          <div>
            <h3 class="text-sm sm:text-base font-semibold">
              访问令牌
            </h3>
            <p class="text-xs text-muted-foreground mt-0.5">
              <template v-if="quota">
                已创建 {{ quota.used }}/{{ quota.max }} 个令牌
                <span
                  v-if="quota.used >= quota.max"
                  class="text-destructive font-medium"
                >（已达上限）</span>
              </template>
              <template v-else>
                用于程序化访问管理 API 的令牌
              </template>
            </p>
          </div>

          <!-- 操作按钮 -->
          <div class="flex items-center gap-2">
            <!-- 新增按钮 -->
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="创建新令牌"
              :disabled="quota ? quota.used >= quota.max : false"
              @click="showCreateDialog = true"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>

            <!-- 刷新按钮 -->
            <RefreshButton
              :loading="loading"
              @click="loadTokens"
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
        v-else-if="tokens.length === 0"
        class="flex items-center justify-center py-12"
      >
        <EmptyState
          title="暂无访问令牌"
          description="创建你的第一个访问令牌开始使用管理 API"
          :icon="KeyRound"
        >
          <template #actions>
            <Button
              size="lg"
              class="shadow-lg shadow-primary/20"
              @click="showCreateDialog = true"
            >
              <Plus class="mr-2 h-4 w-4" />
              创建访问令牌
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
              <TableHead class="min-w-[180px] h-12 font-semibold">
                名称
              </TableHead>
              <TableHead class="min-w-[160px] h-12 font-semibold">
                令牌
              </TableHead>
              <TableHead class="min-w-[80px] h-12 font-semibold text-center">
                使用次数
              </TableHead>
              <TableHead class="min-w-[70px] h-12 font-semibold text-center">
                状态
              </TableHead>
              <TableHead class="min-w-[100px] h-12 font-semibold">
                时间
              </TableHead>
              <TableHead class="min-w-[100px] h-12 font-semibold text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="token in paginatedTokens"
              :key="token.id"
              class="border-b border-border/40 hover:bg-muted/30 transition-colors"
            >
              <!-- 名称 -->
              <TableCell class="py-4">
                <div class="flex-1 min-w-0">
                  <div
                    class="text-sm font-semibold truncate"
                    :title="token.name"
                  >
                    {{ token.name }}
                  </div>
                  <div
                    v-if="token.description"
                    class="text-xs text-muted-foreground mt-0.5 truncate"
                    :title="token.description"
                  >
                    {{ token.description }}
                  </div>
                </div>
              </TableCell>

              <!-- Token 显示 -->
              <TableCell class="py-4">
                <div class="flex items-center gap-1.5">
                  <code class="text-xs font-mono text-muted-foreground bg-muted/30 px-2 py-1 rounded">
                    {{ token.token_display }}
                  </code>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-6 w-6"
                    title="重新生成令牌"
                    @click="confirmRegenerate(token)"
                  >
                    <RefreshCw class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </TableCell>

              <!-- 使用次数 -->
              <TableCell class="py-4 text-center">
                <span class="text-sm font-medium">
                  {{ formatNumber(token.usage_count || 0) }}
                </span>
              </TableCell>

              <!-- 状态 -->
              <TableCell class="py-4 text-center">
                <Badge
                  :variant="getStatusVariant(token)"
                  class="font-medium px-3 py-1"
                >
                  {{ getStatusText(token) }}
                </Badge>
              </TableCell>

              <!-- 时间 -->
              <TableCell class="py-4 text-sm text-muted-foreground">
                <div class="text-xs">
                  创建于 {{ formatDate(token.created_at) }}
                </div>
                <div class="text-xs mt-1">
                  {{ token.last_used_at ? `最后使用 ${formatRelativeTime(token.last_used_at)}` : '从未使用' }}
                </div>
              </TableCell>

              <!-- 操作按钮 -->
              <TableCell class="py-4">
                <div class="flex justify-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="编辑"
                    @click="openEditDialog(token)"
                  >
                    <Pencil class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="token.is_active ? '禁用' : '启用'"
                    @click="toggleToken(token)"
                  >
                    <Power class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="删除"
                    @click="confirmDelete(token)"
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
        v-if="!loading && tokens.length > 0"
        class="md:hidden space-y-3 p-4"
      >
        <Card
          v-for="token in paginatedTokens"
          :key="token.id"
          variant="default"
          class="group hover:shadow-md hover:border-primary/30 transition-all duration-200"
        >
          <div class="p-4">
            <!-- 第一行：名称、状态、操作 -->
            <div class="flex items-center justify-between mb-2">
              <div class="flex items-center gap-2 min-w-0 flex-1">
                <h3 class="text-sm font-semibold text-foreground truncate">
                  {{ token.name }}
                </h3>
                <Badge
                  :variant="getStatusVariant(token)"
                  class="text-xs px-1.5 py-0"
                >
                  {{ getStatusText(token) }}
                </Badge>
              </div>
              <div class="flex items-center gap-0.5 flex-shrink-0">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  title="编辑"
                  @click="openEditDialog(token)"
                >
                  <Pencil class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="token.is_active ? '禁用' : '启用'"
                  @click="toggleToken(token)"
                >
                  <Power class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  title="删除"
                  @click="confirmDelete(token)"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>

            <!-- Token 显示 -->
            <div class="flex items-center gap-2 text-xs mb-2">
              <code class="font-mono text-muted-foreground">{{ token.token_display }}</code>
              <Button
                variant="ghost"
                size="icon"
                class="h-5 w-5"
                title="重新生成"
                @click="confirmRegenerate(token)"
              >
                <RefreshCw class="h-3 w-3" />
              </Button>
            </div>

            <!-- 统计信息 -->
            <div class="flex items-center gap-3 text-xs text-muted-foreground">
              <span>{{ formatNumber(token.usage_count || 0) }} 次使用</span>
              <span>·</span>
              <span>{{ token.last_used_at ? formatRelativeTime(token.last_used_at) : '从未使用' }}</span>
            </div>
          </div>
        </Card>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="totalTokens > 0"
        :current="currentPage"
        :total="totalTokens"
        :page-size="pageSize"
        cache-key="management-tokens-page-size"
        @update:current="currentPage = $event"
        @update:page-size="handlePageSizeChange"
      />
    </Card>

    <!-- 创建/编辑 Token 对话框 -->
    <Dialog
      v-model="showCreateDialog"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
              <KeyRound class="h-5 w-5 text-primary" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                {{ editingToken ? '编辑访问令牌' : '创建访问令牌' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                {{ editingToken ? '修改令牌配置' : '创建一个新的令牌用于访问管理 API' }}
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <!-- 名称 -->
        <div class="space-y-2">
          <Label
            for="token-name"
            class="text-sm font-semibold"
          >名称 *</Label>
          <Input
            id="token-name"
            v-model="formData.name"
            placeholder="例如：CI/CD 自动化"
            class="h-11 border-border/60"
            autocomplete="off"
            required
          />
        </div>

        <!-- 描述 -->
        <div class="space-y-2">
          <Label
            for="token-description"
            class="text-sm font-semibold"
          >描述</Label>
          <Input
            id="token-description"
            v-model="formData.description"
            placeholder="用途说明（可选）"
            class="h-11 border-border/60"
            autocomplete="off"
          />
        </div>


        <!-- IP 白名单 -->
        <div class="space-y-2">
          <Label
            for="token-ips"
            class="text-sm font-semibold"
          >IP 白名单</Label>
          <Input
            id="token-ips"
            v-model="formData.allowedIpsText"
            placeholder="例如：192.168.1.0/24, 10.0.0.1（逗号分隔，留空不限制）"
            class="h-11 border-border/60"
            autocomplete="off"
          />
          <p class="text-xs text-muted-foreground">
            限制只能从指定 IP 地址使用此令牌，支持 CIDR 格式
          </p>
        </div>

        <!-- 过期时间 -->
        <div class="space-y-2">
          <Label
            for="token-expires"
            class="text-sm font-semibold"
          >过期时间</Label>
          <Input
            id="token-expires"
            v-model="formData.expiresAt"
            type="datetime-local"
            class="h-11 border-border/60"
          />
          <p class="text-xs text-muted-foreground">
            留空表示永不过期
          </p>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-11 px-6"
          @click="closeDialog"
        >
          取消
        </Button>
        <Button
          class="h-11 px-6 shadow-lg shadow-primary/20"
          :disabled="saving || !isFormValid"
          @click="saveToken"
        >
          <Loader2
            v-if="saving"
            class="animate-spin h-4 w-4 mr-2"
          />
          {{ saving ? '保存中...' : (editingToken ? '保存' : '创建') }}
        </Button>
      </template>
    </Dialog>

    <!-- 新 Token 创建成功对话框 -->
    <Dialog
      v-model="showTokenDialog"
      size="lg"
      persistent
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-emerald-100 dark:bg-emerald-900/30 flex-shrink-0">
              <CheckCircle class="h-5 w-5 text-emerald-600 dark:text-emerald-400" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                {{ isRegenerating ? '令牌已重新生成' : '创建成功' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                请妥善保管，此令牌只会显示一次
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label class="text-sm font-medium">访问令牌</Label>
          <div class="flex items-center gap-2">
            <Input
              type="text"
              :value="newTokenValue"
              readonly
              class="flex-1 font-mono text-sm bg-muted/50 h-11"
              @click="($event.target as HTMLInputElement)?.select()"
            />
            <Button
              class="h-11"
              @click="copyToken(newTokenValue)"
            >
              复制
            </Button>
          </div>
        </div>
        <div class="p-3 rounded-lg bg-amber-50 dark:bg-amber-950/30 border border-amber-200 dark:border-amber-800">
          <div class="flex gap-2">
            <AlertTriangle class="h-4 w-4 text-amber-600 dark:text-amber-400 flex-shrink-0 mt-0.5" />
            <p class="text-sm text-amber-800 dark:text-amber-200">
              此令牌只会显示一次，关闭后将无法再次查看，请妥善保管。
            </p>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          class="h-10 px-5"
          @click="showTokenDialog = false"
        >
          我已安全保存
        </Button>
      </template>
    </Dialog>

    <!-- 删除确认对话框 -->
    <AlertDialog
      v-model="showDeleteDialog"
      type="danger"
      title="确认删除"
      :description="`确定要删除令牌「${tokenToDelete?.name}」吗？此操作不可恢复。`"
      confirm-text="删除"
      :loading="deleting"
      @confirm="deleteToken"
      @cancel="showDeleteDialog = false"
    />

    <!-- 重新生成确认对话框 -->
    <AlertDialog
      v-model="showRegenerateDialog"
      type="warning"
      title="确认重新生成"
      :description="`重新生成后，原令牌将立即失效。确定要重新生成「${tokenToRegenerate?.name}」吗？`"
      confirm-text="重新生成"
      :loading="regenerating"
      @confirm="regenerateToken"
      @cancel="showRegenerateDialog = false"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, reactive, watch } from 'vue'
import {
  managementTokenApi,
  type ManagementToken
} from '@/api/management-tokens'
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
import {
  Plus,
  KeyRound,
  Trash2,
  Loader2,
  CheckCircle,
  Power,
  Pencil,
  RefreshCw,
  AlertTriangle
} from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { log } from '@/utils/logger'

const { success, error: showError } = useToast()

// 数据
const tokens = ref<ManagementToken[]>([])
const totalTokens = ref(0)
const loading = ref(false)
const saving = ref(false)
const deleting = ref(false)
const regenerating = ref(false)

// 配额信息
const quota = ref<{ used: number; max: number } | null>(null)

// 分页
const currentPage = ref(1)
const pageSize = ref(getPageSizeCache('management-tokens-page-size', 10))

const paginatedTokens = computed(() => tokens.value)

// 监听分页变化
watch([currentPage, pageSize], () => {
  loadTokens()
})

function handlePageSizeChange(newSize: number) {
  pageSize.value = newSize
  currentPage.value = 1
}

// 对话框状态
const showCreateDialog = ref(false)
const showTokenDialog = ref(false)
const showDeleteDialog = ref(false)
const showRegenerateDialog = ref(false)

// 表单数据
const editingToken = ref<ManagementToken | null>(null)
const formData = reactive({
  name: '',
  description: '',
  allowedIpsText: '',
  expiresAt: ''
})

const newTokenValue = ref('')
const isRegenerating = ref(false)
const tokenToDelete = ref<ManagementToken | null>(null)
const tokenToRegenerate = ref<ManagementToken | null>(null)

// 表单验证
const isFormValid = computed(() => {
  return formData.name.trim().length > 0
})

function getStatusVariant(token: ManagementToken): 'success' | 'secondary' | 'destructive' {
  if (token.expires_at && isExpired(token.expires_at)) {
    return 'destructive'
  }
  return token.is_active ? 'success' : 'secondary'
}

function getStatusText(token: ManagementToken): string {
  if (token.expires_at && isExpired(token.expires_at)) {
    return '已过期'
  }
  return token.is_active ? '活跃' : '禁用'
}

function isExpired(dateString: string): boolean {
  return new Date(dateString) < new Date()
}

// 加载数据
onMounted(() => {
  loadTokens()
})

async function loadTokens() {
  loading.value = true
  try {
    const skip = (currentPage.value - 1) * pageSize.value
    const response = await managementTokenApi.listTokens({ skip, limit: pageSize.value })

    tokens.value = response.items
    totalTokens.value = response.total

    if (response.quota) {
      quota.value = response.quota
    }

    // 如果当前页超出范围，重置到第一页
    if (tokens.value.length === 0 && currentPage.value > 1) {
      currentPage.value = 1
    }
  } catch (err: any) {
    log.error('加载 Management Tokens 失败:', err)
    if (!err.response) {
      showError('无法连接到服务器')
    } else {
      showError(`加载失败：${err.response?.data?.detail || err.message}`)
    }
  } finally {
    loading.value = false
  }
}

// 打开编辑对话框
function openEditDialog(token: ManagementToken) {
  editingToken.value = token
  formData.name = token.name
  formData.description = token.description || ''
  formData.allowedIpsText = (token.allowed_ips && token.allowed_ips.length > 0)
    ? token.allowed_ips.join(', ')
    : ''
  formData.expiresAt = token.expires_at
    ? toLocalDatetimeString(new Date(token.expires_at))
    : ''
  showCreateDialog.value = true
}

// 关闭对话框
function closeDialog() {
  showCreateDialog.value = false
  editingToken.value = null
  formData.name = ''
  formData.description = ''
  formData.allowedIpsText = ''
  formData.expiresAt = ''
}

// 保存 Token
async function saveToken() {
  if (!isFormValid.value) return

  saving.value = true
  try {
    const allowedIps = formData.allowedIpsText
      .split(',')
      .map(ip => ip.trim())
      .filter(ip => ip)

    // 将本地时间转换为 UTC ISO 字符串
    const expiresAtUtc = formData.expiresAt
      ? new Date(formData.expiresAt).toISOString()
      : null

    if (editingToken.value) {
      // 更新
      await managementTokenApi.updateToken(editingToken.value.id, {
        name: formData.name,
        description: formData.description.trim() || null,
        allowed_ips: allowedIps.length > 0 ? allowedIps : null,
        expires_at: expiresAtUtc
      })
      success('令牌更新成功')
    } else {
      // 创建
      const result = await managementTokenApi.createToken({
        name: formData.name,
        description: formData.description || undefined,
        allowed_ips: allowedIps.length > 0 ? allowedIps : undefined,
        expires_at: expiresAtUtc
      })
      newTokenValue.value = result.token
      isRegenerating.value = false
      showTokenDialog.value = true
      success('令牌创建成功')
    }

    closeDialog()
    await loadTokens()
  } catch (err: any) {
    log.error('保存 Token 失败:', err)
    const message = err.response?.data?.error?.message
      || err.response?.data?.detail
      || '保存失败'
    showError(message)
  } finally {
    saving.value = false
  }
}

// 切换状态
async function toggleToken(token: ManagementToken) {
  try {
    const result = await managementTokenApi.toggleToken(token.id)

    const index = tokens.value.findIndex(t => t.id === token.id)
    if (index !== -1) {
      tokens.value[index] = result.data
    }
    success(result.data.is_active ? '令牌已启用' : '令牌已禁用')
  } catch (err: any) {
    log.error('切换状态失败:', err)
    showError('操作失败')
  }
}

// 删除
function confirmDelete(token: ManagementToken) {
  tokenToDelete.value = token
  showDeleteDialog.value = true
}

async function deleteToken() {
  if (!tokenToDelete.value) return

  deleting.value = true
  try {
    await managementTokenApi.deleteToken(tokenToDelete.value.id)

    showDeleteDialog.value = false
    success('令牌已删除')
    await loadTokens()
  } catch (err: any) {
    log.error('删除 Token 失败:', err)
    showError('删除失败')
  } finally {
    deleting.value = false
    tokenToDelete.value = null
  }
}

// 重新生成
function confirmRegenerate(token: ManagementToken) {
  tokenToRegenerate.value = token
  showRegenerateDialog.value = true
}

async function regenerateToken() {
  if (!tokenToRegenerate.value) return

  regenerating.value = true
  try {
    const result = await managementTokenApi.regenerateToken(tokenToRegenerate.value.id)
    newTokenValue.value = result.token
    isRegenerating.value = true
    showRegenerateDialog.value = false
    showTokenDialog.value = true
    await loadTokens()
    success('令牌已重新生成')
  } catch (err: any) {
    log.error('重新生成失败:', err)
    showError('重新生成失败')
  } finally {
    regenerating.value = false
    tokenToRegenerate.value = null
  }
}

// 复制 Token
async function copyToken(text: string) {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      success('已复制到剪贴板')
    } else {
      const textArea = document.createElement('textarea')
      textArea.value = text
      textArea.style.position = 'fixed'
      textArea.style.left = '-999999px'
      document.body.appendChild(textArea)
      textArea.select()
      document.execCommand('copy')
      document.body.removeChild(textArea)
      success('已复制到剪贴板')
    }
  } catch (err) {
    log.error('复制失败:', err)
    showError('复制失败')
  }
}

// 格式化
function formatNumber(num: number): string {
  return num.toLocaleString('zh-CN')
}

function toLocalDatetimeString(date: Date): string {
  const year = date.getFullYear()
  const month = String(date.getMonth() + 1).padStart(2, '0')
  const day = String(date.getDate()).padStart(2, '0')
  const hours = String(date.getHours()).padStart(2, '0')
  const minutes = String(date.getMinutes()).padStart(2, '0')
  return `${year}-${month}-${day}T${hours}:${minutes}`
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
</script>
