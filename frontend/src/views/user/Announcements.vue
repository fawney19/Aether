<template>
  <div class="space-y-6 pb-8">
    <!-- 公告列表卡片 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和操作栏 -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
          <div class="shrink-0">
            <h3 class="text-sm sm:text-base font-semibold">
              公告管理
            </h3>
            <p class="text-xs text-muted-foreground mt-0.5">
              {{ isAdmin ? '管理系统公告和通知' : '查看系统公告和通知' }}
            </p>
          </div>
          <div class="flex flex-wrap items-center gap-2">
            <Badge
              v-if="unreadCount > 0"
              variant="default"
              class="px-3 py-1"
            >
              {{ unreadCount }} 条未读
            </Badge>
            <div class="hidden sm:block h-4 w-px bg-border" />
            <Button
              v-if="isAdmin"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="新建公告"
              @click="openCreateDialog"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>
            <RefreshButton
              :loading="loading"
              @click="loadAnnouncements(currentPage)"
            />
          </div>
        </div>
      </div>

      <!-- 内容区域 -->
      <div
        v-if="loading"
        class="flex items-center justify-center py-12"
      >
        <Loader2 class="w-8 h-8 animate-spin text-primary" />
      </div>

      <div
        v-else-if="announcements.length === 0"
        class="flex flex-col items-center justify-center py-12 text-center"
      >
        <Bell class="h-12 w-12 text-muted-foreground mb-3" />
        <h3 class="text-sm font-medium text-foreground">
          暂无公告
        </h3>
        <p class="text-xs text-muted-foreground mt-1">
          系统暂时没有发布任何公告
        </p>
      </div>

      <div
        v-else
        class="overflow-x-auto"
      >
        <Table class="hidden xl:table">
          <TableHeader>
            <TableRow class="border-b border-border/60 hover:bg-transparent">
              <TableHead class="w-[80px] h-12 font-semibold text-center">
                类型
              </TableHead>
              <TableHead class="h-12 font-semibold">
                概要
              </TableHead>
              <TableHead class="w-[120px] h-12 font-semibold">
                发布者
              </TableHead>
              <TableHead class="w-[140px] h-12 font-semibold">
                发布时间
              </TableHead>
              <TableHead class="w-[80px] h-12 font-semibold text-center">
                状态
              </TableHead>
              <TableHead
                v-if="isAdmin"
                class="w-[80px] h-12 font-semibold text-center"
              >
                置顶
              </TableHead>
              <TableHead
                v-if="isAdmin"
                class="w-[80px] h-12 font-semibold text-center"
              >
                启用
              </TableHead>
              <TableHead
                v-if="isAdmin"
                class="w-[100px] h-12 font-semibold text-center"
              >
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="announcement in announcements"
              :key="announcement.id"
              :class="'border-b border-border/40 transition-colors cursor-pointer ' + (announcement.is_read ? 'hover:bg-muted/30' : 'bg-primary/5 hover:bg-primary/10')"
              @click="viewAnnouncementDetail(announcement)"
            >
              <TableCell class="py-4 text-center">
                <div class="flex flex-col items-center gap-1">
                  <component
                    :is="getAnnouncementIcon(announcement.type)"
                    class="w-5 h-5"
                    :class="getIconColor(announcement.type)"
                  />
                  <span
                    class="text-xs font-medium"
                    :class="[getTypeTextColor(announcement.type)]"
                  >
                    {{ getTypeLabel(announcement.type) }}
                  </span>
                </div>
              </TableCell>
              <TableCell class="py-4">
                <div class="flex-1 min-w-0">
                  <div class="flex items-center gap-2 mb-1">
                    <span class="text-sm font-medium text-foreground">{{ announcement.title }}</span>
                    <Pin
                      v-if="announcement.is_pinned"
                      class="w-3.5 h-3.5 text-muted-foreground flex-shrink-0"
                    />
                  </div>
                  <p class="text-xs text-muted-foreground line-clamp-1">
                    {{ getPlainText(announcement.content) }}
                  </p>
                </div>
              </TableCell>
              <TableCell class="py-4 text-sm text-muted-foreground">
                {{ announcement.author.username }}
              </TableCell>
              <TableCell class="py-4 text-xs text-muted-foreground">
                {{ formatDate(announcement.created_at) }}
              </TableCell>
              <TableCell class="py-4 text-center">
                <Badge
                  v-if="announcement.is_read"
                  variant="secondary"
                  class="text-xs px-2.5 py-0.5"
                >
                  已读
                </Badge>
                <Badge
                  v-else
                  variant="default"
                  class="text-xs px-2.5 py-0.5"
                >
                  未读
                </Badge>
              </TableCell>
              <TableCell
                v-if="isAdmin"
                class="py-4"
                @click.stop
              >
                <div class="flex items-center justify-center">
                  <Switch
                    :model-value="announcement.is_pinned"
                    class="data-[state=checked]:bg-emerald-500"
                    @update:model-value="toggleAnnouncementPin(announcement, $event)"
                  />
                </div>
              </TableCell>
              <TableCell
                v-if="isAdmin"
                class="py-4"
                @click.stop
              >
                <div class="flex items-center justify-center">
                  <Switch
                    :model-value="announcement.is_active"
                    class="data-[state=checked]:bg-primary"
                    @update:model-value="toggleAnnouncementActive(announcement, $event)"
                  />
                </div>
              </TableCell>
              <TableCell
                v-if="isAdmin"
                class="py-4"
                @click.stop
              >
                <div class="flex items-center justify-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    @click="openEditDialog(announcement)"
                  >
                    <SquarePen class="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-9 w-9 hover:bg-rose-500/10 hover:text-rose-600"
                    @click="confirmDelete(announcement)"
                  >
                    <Trash2 class="w-4 h-4" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>

        <!-- 移动端卡片列表 -->
        <div
          v-if="announcements.length > 0"
          class="xl:hidden divide-y divide-border/40"
        >
          <div
            v-for="announcement in announcements"
            :key="announcement.id"
            class="p-4 space-y-2 cursor-pointer transition-colors"
            :class="[
              announcement.is_read ? 'hover:bg-muted/30' : 'bg-primary/5 hover:bg-primary/10'
            ]"
            @click="viewAnnouncementDetail(announcement)"
          >
            <div class="flex items-start justify-between gap-3">
              <div class="flex items-center gap-2">
                <component
                  :is="getAnnouncementIcon(announcement.type)"
                  class="w-4 h-4 shrink-0"
                  :class="getIconColor(announcement.type)"
                />
                <span class="font-medium text-sm">{{ announcement.title }}</span>
                <Pin
                  v-if="announcement.is_pinned"
                  class="w-3.5 h-3.5 text-muted-foreground shrink-0"
                />
              </div>
              <Badge
                :variant="announcement.is_read ? 'secondary' : 'default'"
                class="text-xs shrink-0"
              >
                {{ announcement.is_read ? '已读' : '未读' }}
              </Badge>
            </div>
            <p class="text-xs text-muted-foreground line-clamp-2">
              {{ getPlainText(announcement.content) }}
            </p>
            <div class="flex items-center gap-2 text-xs text-muted-foreground">
              <span>{{ announcement.author.username }}</span>
              <span>·</span>
              <span>{{ formatDate(announcement.created_at) }}</span>
            </div>
            <div
              v-if="isAdmin"
              class="flex items-center gap-4 pt-2"
              @click.stop
            >
              <div class="flex items-center gap-2">
                <span class="text-xs text-muted-foreground">置顶</span>
                <Switch
                  :model-value="announcement.is_pinned"
                  class="data-[state=checked]:bg-emerald-500 scale-75"
                  @update:model-value="toggleAnnouncementPin(announcement, $event)"
                />
              </div>
              <div class="flex items-center gap-2">
                <span class="text-xs text-muted-foreground">启用</span>
                <Switch
                  :model-value="announcement.is_active"
                  class="data-[state=checked]:bg-primary scale-75"
                  @update:model-value="toggleAnnouncementActive(announcement, $event)"
                />
              </div>
              <div class="flex items-center gap-1 ml-auto">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  @click="openEditDialog(announcement)"
                >
                  <SquarePen class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 hover:text-destructive"
                  @click="confirmDelete(announcement)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </Button>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="!loading && total > 0"
        :current="currentPage"
        :total="total"
        :page-size="pageSize"
        cache-key="announcements-page-size"
        @update:current="loadAnnouncements($event)"
        @update:page-size="pageSize = $event; loadAnnouncements(1)"
      />
    </Card>

    <!-- 创建/编辑公告对话框 -->
    <Dialog
      v-model="dialogOpen"
      size="xl"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
              <Bell class="h-5 w-5 text-primary" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                {{ editingAnnouncement ? '编辑公告' : '新建公告' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                {{ editingAnnouncement ? '修改公告内容和设置' : '发布新的系统公告' }}
              </p>
            </div>
          </div>
        </div>
      </template>

      <form
        class="space-y-4"
        @submit.prevent="saveAnnouncement"
      >
        <div class="space-y-2">
          <Label
            for="title"
            class="text-sm font-medium"
          >标题 *</Label>
          <Input
            id="title"
            v-model="formData.title"
            placeholder="输入公告标题"
            class="h-11"
            required
          />
        </div>

        <div class="space-y-2">
          <Label
            for="content"
            class="text-sm font-medium"
          >内容 * (支持 Markdown)</Label>
          <Textarea
            id="content"
            v-model="formData.content"
            placeholder="输入公告内容，支持 Markdown 格式"
            rows="10"
            required
          />
        </div>

        <div class="grid grid-cols-2 gap-4">
          <div class="space-y-2">
            <Label
              for="type"
              class="text-sm font-medium"
            >类型</Label>
            <Select
              v-model="formData.type"
              v-model:open="typeSelectOpen"
            >
              <SelectTrigger
                id="type"
                class="h-11"
              >
                <SelectValue placeholder="选择类型" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="info">
                  信息
                </SelectItem>
                <SelectItem value="warning">
                  警告
                </SelectItem>
                <SelectItem value="maintenance">
                  维护
                </SelectItem>
                <SelectItem value="important">
                  重要
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          <div class="space-y-2">
            <Label
              for="priority"
              class="text-sm font-medium"
            >优先级</Label>
            <Input
              id="priority"
              v-model.number="formData.priority"
              type="number"
              placeholder="0"
              class="h-11"
              min="0"
              max="10"
            />
          </div>
        </div>

        <div class="flex items-center gap-6 p-3 border rounded-lg bg-muted/50">
          <div class="flex items-center gap-2">
            <input
              id="pinned"
              v-model="formData.is_pinned"
              type="checkbox"
              class="h-4 w-4 rounded border-gray-300 cursor-pointer"
            >
            <Label
              for="pinned"
              class="cursor-pointer text-sm"
            >置顶公告</Label>
          </div>
          <div
            v-if="editingAnnouncement"
            class="flex items-center gap-2"
          >
            <input
              id="active"
              v-model="formData.is_active"
              type="checkbox"
              class="h-4 w-4 rounded border-gray-300 cursor-pointer"
            >
            <Label
              for="active"
              class="cursor-pointer text-sm"
            >启用</Label>
          </div>
        </div>
      </form>

      <template #footer>
        <Button
          :disabled="saving"
          class="h-10 px-5"
          @click="saveAnnouncement"
        >
          <Loader2
            v-if="saving"
            class="animate-spin h-4 w-4 mr-2"
          />
          {{ editingAnnouncement ? '保存' : '创建' }}
        </Button>
        <Button
          variant="outline"
          type="button"
          class="h-10 px-5"
          @click="dialogOpen = false"
        >
          取消
        </Button>
      </template>
    </Dialog>

    <!-- 删除确认对话框 -->
    <AlertDialog
      v-model="deleteDialogOpen"
      type="danger"
      title="确认删除"
      :description="`确定要删除公告「${deletingAnnouncement?.title}」吗？此操作无法撤销。`"
      confirm-text="删除"
      :loading="deleting"
      @confirm="deleteAnnouncement"
      @cancel="deleteDialogOpen = false"
    />

    <!-- 公告详情对话框 -->
    <Dialog
      v-model="detailDialogOpen"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div
              class="flex h-9 w-9 items-center justify-center rounded-lg flex-shrink-0"
              :class="getDialogIconClass(viewingAnnouncement?.type)"
            >
              <component
                :is="getAnnouncementIcon(viewingAnnouncement.type)"
                v-if="viewingAnnouncement"
                class="h-5 w-5"
                :class="getIconColor(viewingAnnouncement.type)"
              />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight truncate">
                {{ viewingAnnouncement?.title || '公告详情' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                系统公告
              </p>
            </div>
          </div>
        </div>
      </template>

      <div
        v-if="viewingAnnouncement"
        class="space-y-4"
      >
        <div class="flex items-center gap-3 text-xs text-gray-500 dark:text-muted-foreground">
          <span>{{ viewingAnnouncement.author.username }}</span>
          <span>·</span>
          <span>{{ formatFullDate(viewingAnnouncement.created_at) }}</span>
        </div>

        <div
          class="prose prose-sm dark:prose-invert max-w-none"
          v-html="renderMarkdown(viewingAnnouncement.content)"
        />
      </div>

      <template #footer>
        <Button
          variant="outline"
          type="button"
          class="h-10 px-5"
          @click="detailDialogOpen = false"
        >
          关闭
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { announcementApi, type Announcement } from '@/api/announcements'
import { useAuthStore } from '@/stores/auth'
import {
  Card,
  Button,
  Badge,
  Input,
  Label,
  Textarea,
  Dialog,
  Pagination,
  RefreshButton,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Switch,
  getPageSizeCache
} from '@/components/ui'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { AlertDialog } from '@/components/common'
import { Bell, AlertCircle, AlertTriangle, Info, Pin, Wrench, Loader2, Plus, SquarePen, Trash2 } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { log } from '@/utils/logger'
import { marked } from 'marked'
import { sanitizeMarkdown } from '@/utils/sanitize'

const { success, error: showError } = useToast()
const authStore = useAuthStore()
const isAdmin = computed(() => authStore.isAdmin)

const announcements = ref<Announcement[]>([])
const loading = ref(false)
const total = ref(0)
const unreadCount = ref(0)
const currentPage = ref(1)
const pageSize = ref(getPageSizeCache('announcements-page-size', 20))

// 对话框状态
const dialogOpen = ref(false)
const deleteDialogOpen = ref(false)
const detailDialogOpen = ref(false)
const typeSelectOpen = ref(false)
const editingAnnouncement = ref<Announcement | null>(null)
const deletingAnnouncement = ref<Announcement | null>(null)
const viewingAnnouncement = ref<Announcement | null>(null)
const saving = ref(false)
const deleting = ref(false)

// 表单数据
const formData = ref({
  title: '',
  content: '',
  type: 'info' as 'info' | 'warning' | 'maintenance' | 'important',
  priority: 0,
  is_pinned: false,
  is_active: true
})

onMounted(() => {
  loadAnnouncements()
})

async function loadAnnouncements(page = 1) {
  loading.value = true
  currentPage.value = page
  try {
    const response = await announcementApi.getAnnouncements({
      active_only: !isAdmin.value, // 管理员可以看到所有公告
      limit: pageSize.value,
      offset: (page - 1) * pageSize.value
    })
    announcements.value = response.items
    total.value = response.total
    unreadCount.value = response.unread_count || 0
  } catch (error) {
    log.error('加载公告失败:', error)
    showError('加载公告失败')
  } finally {
    loading.value = false
  }
}

async function viewAnnouncementDetail(announcement: Announcement) {
  // 标记为已读
  if (!announcement.is_read && !isAdmin.value) {
    try {
      await announcementApi.markAsRead(announcement.id)
      announcement.is_read = true
      unreadCount.value = Math.max(0, unreadCount.value - 1)
    } catch (error) {
      log.error('标记已读失败:', error)
    }
  }

  // 显示详情对话框
  viewingAnnouncement.value = announcement
  detailDialogOpen.value = true
}

function openCreateDialog() {
  editingAnnouncement.value = null
  formData.value = {
    title: '',
    content: '',
    type: 'info',
    priority: 0,
    is_pinned: false,
    is_active: true
  }
  dialogOpen.value = true
}

function openEditDialog(announcement: Announcement) {
  editingAnnouncement.value = announcement
  formData.value = {
    title: announcement.title,
    content: announcement.content,
    type: announcement.type,
    priority: announcement.priority,
    is_pinned: announcement.is_pinned,
    is_active: announcement.is_active
  }
  dialogOpen.value = true
}

async function toggleAnnouncementPin(announcement: Announcement, newStatus: boolean) {
  try {
    await announcementApi.updateAnnouncement(announcement.id, {
      is_pinned: newStatus
    })
    announcement.is_pinned = newStatus
    success(newStatus ? '已置顶' : '已取消置顶')
  } catch (error) {
    log.error('更新置顶状态失败:', error)
    showError('更新置顶状态失败')
  }
}

async function toggleAnnouncementActive(announcement: Announcement, newStatus: boolean) {
  try {
    await announcementApi.updateAnnouncement(announcement.id, {
      is_active: newStatus
    })
    announcement.is_active = newStatus
    success(newStatus ? '已启用' : '已禁用')
  } catch (error) {
    log.error('更新启用状态失败:', error)
    showError('更新启用状态失败')
  }
}

async function saveAnnouncement() {
  if (!formData.value.title || !formData.value.content) {
    showError('请填写标题和内容')
    return
  }

  saving.value = true
  try {
    if (editingAnnouncement.value) {
      // 更新
      await announcementApi.updateAnnouncement(editingAnnouncement.value.id, formData.value)
      success('公告更新成功')
    } else {
      // 创建
      await announcementApi.createAnnouncement(formData.value)
      success('公告创建成功')
    }
    dialogOpen.value = false
    loadAnnouncements(currentPage.value)
  } catch (error) {
    log.error('保存失败:', error)
    showError('保存失败')
  } finally {
    saving.value = false
  }
}

function confirmDelete(announcement: Announcement) {
  deletingAnnouncement.value = announcement
  deleteDialogOpen.value = true
}

async function deleteAnnouncement() {
  if (!deletingAnnouncement.value) return

  deleting.value = true
  try {
    await announcementApi.deleteAnnouncement(deletingAnnouncement.value.id)
    success('公告已删除')
    deleteDialogOpen.value = false
    loadAnnouncements(currentPage.value)
  } catch (error) {
    log.error('删除失败:', error)
    showError('删除失败')
  } finally {
    deleting.value = false
  }
}

function getAnnouncementIcon(type: string) {
  switch (type) {
    case 'important':
      return AlertCircle
    case 'warning':
      return AlertTriangle
    case 'maintenance':
      return Wrench
    default:
      return Info
  }
}

function getIconColor(type: string) {
  switch (type) {
    case 'important':
      return 'text-red-500'
    case 'warning':
      return 'text-yellow-500'
    case 'maintenance':
      return 'text-orange-500'
    default:
      return 'text-primary'
  }
}

function getTypeTextColor(type: string): string {
  switch (type) {
    case 'important':
      return 'text-red-600 dark:text-red-400'
    case 'warning':
      return 'text-yellow-600 dark:text-yellow-400'
    case 'maintenance':
      return 'text-orange-600 dark:text-orange-400'
    default:
      return 'text-primary'
  }
}

function getTypeLabel(type: string): string {
  switch (type) {
    case 'important':
      return '重要'
    case 'warning':
      return '警告'
    case 'maintenance':
      return '维护'
    default:
      return '信息'
  }
}

function getDialogIconClass(type?: string) {
  switch (type) {
    case 'important':
      return 'bg-rose-100 dark:bg-rose-900/30'
    case 'warning':
      return 'bg-amber-100 dark:bg-amber-900/30'
    case 'maintenance':
      return 'bg-orange-100 dark:bg-orange-900/30'
    default:
      return 'bg-primary/10 dark:bg-primary/20'
  }
}

function formatFullDate(dateString: string): string {
  const date = new Date(dateString)
  return date.toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
}

function renderMarkdown(content: string): string {
  const rawHtml = marked(content) as string
  return sanitizeMarkdown(rawHtml)
}

function getPlainText(content: string): string {
  // 简单地移除 Markdown 标记，用于预览
  return content
    .replace(/[#*_`~[\]()]/g, '')
    .replace(/\n+/g, ' ')
    .trim()
    .substring(0, 200)
}

function formatDate(dateString: string): string {
  const date = new Date(dateString)
  const now = new Date()
  const diff = now.getTime() - date.getTime()
  const days = Math.floor(diff / (1000 * 60 * 60 * 24))
  const hours = Math.floor(diff / (1000 * 60 * 60))
  const minutes = Math.floor(diff / (1000 * 60))

  if (minutes < 60) {
    return `${minutes} 分钟前`
  } else if (hours < 24) {
    return `${hours} 小时前`
  } else if (days < 7) {
    return `${days} 天前`
  } else {
    return date.toLocaleDateString('zh-CN', {
      year: 'numeric',
      month: '2-digit',
      day: '2-digit'
    })
  }
}
</script>

<style scoped>
/* Markdown 内容样式 */
:deep(.prose) {
  max-width: none;
}

:deep(.prose p) {
  margin-top: 0.5em;
  margin-bottom: 0.5em;
}

:deep(.prose ul) {
  margin-top: 0.5em;
  margin-bottom: 0.5em;
}

:deep(.prose li) {
  margin-top: 0.25em;
  margin-bottom: 0.25em;
}

:deep(.prose h1),
:deep(.prose h2),
:deep(.prose h3) {
  margin-top: 1em;
  margin-bottom: 0.5em;
}

:deep(.prose code) {
  @apply bg-gray-100 dark:bg-muted px-1 py-0.5 rounded text-sm;
}

:deep(.prose pre) {
  @apply bg-gray-100 dark:bg-card p-3 rounded-lg overflow-x-auto;
}

.line-clamp-2 {
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
</style>
