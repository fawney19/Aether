<template>
  <div class="space-y-6 pb-8">
    <!-- 统计卡片 -->
    <div class="grid grid-cols-2 lg:grid-cols-4 gap-4">
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-primary/10 flex items-center justify-center">
            <FileUp class="w-5 h-5 text-primary" />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ stats?.total_mappings ?? '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              总文件数
            </p>
          </div>
        </div>
      </Card>
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-green-500/10 flex items-center justify-center">
            <CheckCircle class="w-5 h-5 text-green-500" />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ stats?.active_mappings ?? '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              有效文件
            </p>
          </div>
        </div>
      </Card>
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-amber-500/10 flex items-center justify-center">
            <Clock class="w-5 h-5 text-amber-500" />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ stats?.expired_mappings ?? '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              已过期
            </p>
          </div>
        </div>
      </Card>
      <Card
        variant="default"
        class="p-4"
      >
        <div class="flex items-center gap-3">
          <div class="w-10 h-10 rounded-lg bg-blue-500/10 flex items-center justify-center">
            <Key class="w-5 h-5 text-blue-500" />
          </div>
          <div>
            <p class="text-2xl font-bold">
              {{ stats?.capable_keys_count ?? '-' }}
            </p>
            <p class="text-xs text-muted-foreground">
              支持的 Key
            </p>
          </div>
        </div>
      </Card>
    </div>

    <!-- 上传区域 -->
    <Card
      variant="default"
      class="p-4"
    >
      <div class="flex items-center justify-between mb-3">
        <h3 class="text-sm font-medium">
          上传文件
        </h3>
        <Button
          v-if="capableKeys.length > 0"
          variant="ghost"
          size="sm"
          class="h-7 text-xs"
          @click="toggleSelectAll"
        >
          {{ selectedKeyIds.length === capableKeys.length ? '取消全选' : '全选' }}
        </Button>
      </div>

      <!-- Key 选择器 -->
      <div
        v-if="capableKeys.length > 0"
        class="mb-4"
      >
        <p class="text-xs text-muted-foreground mb-2">
          选择要上传到的 Key（可多选）：
        </p>
        <div class="flex flex-wrap gap-2">
          <button
            v-for="key in capableKeys"
            :key="key.id"
            class="px-3 py-1.5 text-xs rounded-lg border transition-colors"
            :class="selectedKeyIds.includes(key.id)
              ? 'border-primary bg-primary/10 text-primary'
              : 'border-border hover:border-primary/50'"
            @click="toggleKeySelection(key.id)"
          >
            <span class="font-medium">{{ key.name }}</span>
            <span
              v-if="key.provider_name"
              class="text-muted-foreground ml-1"
            >({{ key.provider_name }})</span>
          </button>
        </div>
      </div>
      <div
        v-else
        class="mb-4 text-sm text-amber-600 bg-amber-50 dark:bg-amber-950/30 rounded-lg p-3"
      >
        暂无可用的 Key，请先配置具有「Gemini 文件 API」能力的 Key
      </div>

      <!-- 拖拽上传区 -->
      <div
        class="border-2 border-dashed border-border/60 rounded-lg p-6 text-center transition-colors"
        :class="{
          'border-primary bg-primary/5': isDragging,
          'hover:border-primary/50': !isDragging && !uploading && selectedKeyIds.length > 0,
          'opacity-50 cursor-not-allowed': selectedKeyIds.length === 0
        }"
        @dragover.prevent="isDragging = true"
        @dragleave.prevent="isDragging = false"
        @drop.prevent="handleDrop"
      >
        <input
          ref="fileInputRef"
          type="file"
          class="hidden"
          @change="handleFileSelect"
        >
        <div
          v-if="uploading"
          class="flex flex-col items-center gap-2"
        >
          <Loader2 class="w-8 h-8 animate-spin text-primary" />
          <p class="text-sm text-muted-foreground">
            正在上传到 {{ selectedKeyIds.length }} 个 Key...
          </p>
        </div>
        <div
          v-else
          class="flex flex-col items-center gap-2"
        >
          <Upload class="w-8 h-8 text-muted-foreground" />
          <p class="text-sm text-muted-foreground">
            <template v-if="selectedKeyIds.length > 0">
              拖拽文件到此处，或
              <button
                class="text-primary hover:underline"
                @click="fileInputRef?.click()"
              >
                点击选择
              </button>
            </template>
            <template v-else>
              请先选择至少一个 Key
            </template>
          </p>
          <p class="text-xs text-muted-foreground">
            支持视频、图片、音频、文档等，最大 2GB，有效期 48 小时
          </p>
        </div>
      </div>
    </Card>

    <!-- MIME 类型分布 -->
    <Card
      v-if="stats?.by_mime_type && Object.keys(stats.by_mime_type).length > 0"
      variant="default"
      class="p-4"
    >
      <h3 class="text-sm font-medium mb-3">
        文件类型分布
      </h3>
      <div class="flex flex-wrap gap-2">
        <Badge
          v-for="(count, mimeType) in stats.by_mime_type"
          :key="mimeType"
          variant="secondary"
          class="text-xs"
        >
          {{ mimeType }}: {{ count }}
        </Badge>
      </div>
    </Card>

    <!-- 文件映射表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和筛选器 -->
      <div class="px-4 sm:px-6 py-3.5 border-b border-border/60">
        <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <h3 class="text-base font-semibold">
            文件映射
          </h3>
          <div class="flex items-center gap-2">
            <!-- 搜索 -->
            <Input
              v-model="searchQuery"
              type="text"
              placeholder="搜索文件名..."
              class="w-40 h-8 text-xs"
            />
            <!-- 包含过期 -->
            <label class="flex items-center gap-1.5 text-xs text-muted-foreground cursor-pointer">
              <input
                v-model="includeExpired"
                type="checkbox"
                class="rounded border-border"
              >
              包含过期
            </label>
            <!-- 清理过期按钮 -->
            <Button
              variant="outline"
              size="sm"
              class="h-8 text-xs"
              :disabled="loading || (stats?.expired_mappings ?? 0) === 0"
              @click="cleanupExpired"
            >
              <Trash2 class="w-3 h-3 mr-1" />
              清理过期
            </Button>
            <!-- 刷新按钮 -->
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              :disabled="loading"
              @click="fetchData"
            >
              <RefreshCw
                class="w-3.5 h-3.5"
                :class="{ 'animate-spin': loading }"
              />
            </Button>
          </div>
        </div>
      </div>

      <!-- 加载状态 -->
      <div
        v-if="loading && !mappings.length"
        class="p-8 text-center"
      >
        <Loader2 class="w-8 h-8 animate-spin mx-auto text-muted-foreground" />
        <p class="mt-2 text-sm text-muted-foreground">
          加载中...
        </p>
      </div>

      <!-- 空状态 -->
      <div
        v-else-if="!mappings.length"
        class="p-8 text-center"
      >
        <FileUp class="w-12 h-12 mx-auto text-muted-foreground/50" />
        <p class="mt-2 text-sm text-muted-foreground">
          暂无文件映射
        </p>
        <p class="mt-1 text-xs text-muted-foreground">
          用户通过 Gemini Files API 上传文件后会在此显示
        </p>
      </div>

      <!-- 文件列表 -->
      <div
        v-else
        class="divide-y divide-border/60"
      >
        <div
          v-for="mapping in mappings"
          :key="mapping.id"
          class="px-4 sm:px-6 py-4 hover:bg-muted/30 transition-colors"
          :class="{ 'opacity-50': mapping.is_expired }"
        >
          <div class="flex items-start justify-between gap-4">
            <div class="flex-1 min-w-0">
              <!-- 文件名和状态 -->
              <div class="flex items-center gap-2 mb-1">
                <component
                  :is="getFileIcon(mapping.mime_type)"
                  class="w-4 h-4 text-muted-foreground"
                />
                <span class="font-mono text-sm font-medium">{{ mapping.file_name }}</span>
                <Badge
                  v-if="mapping.is_expired"
                  variant="secondary"
                  class="text-xs"
                >
                  已过期
                </Badge>
                <Badge
                  v-else
                  variant="outline"
                  class="text-xs text-green-600"
                >
                  有效
                </Badge>
              </div>
              <!-- 显示名 -->
              <p
                v-if="mapping.display_name"
                class="text-sm text-muted-foreground truncate"
              >
                {{ mapping.display_name }}
              </p>
              <!-- 元信息 -->
              <div class="flex items-center gap-4 mt-2 text-xs text-muted-foreground">
                <span
                  v-if="mapping.mime_type"
                  class="flex items-center gap-1"
                >
                  <File class="w-3 h-3" />
                  {{ mapping.mime_type }}
                </span>
                <span
                  v-if="mapping.username"
                  class="flex items-center gap-1"
                >
                  <User class="w-3 h-3" />
                  {{ mapping.username }}
                </span>
                <span
                  v-if="mapping.key_name"
                  class="flex items-center gap-1"
                >
                  <Key class="w-3 h-3" />
                  {{ mapping.key_name }}
                </span>
                <span class="flex items-center gap-1">
                  <Clock class="w-3 h-3" />
                  {{ formatDate(mapping.created_at) }}
                </span>
                <span
                  class="flex items-center gap-1"
                  :class="{ 'text-red-500': mapping.is_expired }"
                >
                  <Timer class="w-3 h-3" />
                  过期: {{ formatDate(mapping.expires_at) }}
                </span>
              </div>
            </div>
            <!-- 操作 -->
            <div class="flex items-center gap-2">
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8 text-muted-foreground hover:text-red-500"
                title="删除映射"
                @click.stop="deleteMapping(mapping)"
              >
                <Trash2 class="w-4 h-4" />
              </Button>
            </div>
          </div>
        </div>
      </div>

      <!-- 分页 -->
      <div
        v-if="totalPages > 1"
        class="px-4 sm:px-6 py-3 border-t border-border/60 flex items-center justify-between"
      >
        <p class="text-xs text-muted-foreground">
          共 {{ total }} 条记录
        </p>
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            size="sm"
            :disabled="currentPage <= 1"
            @click="currentPage--"
          >
            上一页
          </Button>
          <span class="text-sm text-muted-foreground">
            {{ currentPage }} / {{ totalPages }}
          </span>
          <Button
            variant="outline"
            size="sm"
            :disabled="currentPage >= totalPages"
            @click="currentPage++"
          >
            下一页
          </Button>
        </div>
      </div>
    </Card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, onMounted } from 'vue'
import { useToast } from '@/composables/useToast'
import { useI18n } from '@/i18n'
import Card from '@/components/ui/card.vue'
import Badge from '@/components/ui/badge.vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import {
  FileUp,
  CheckCircle,
  Clock,
  Key,
  RefreshCw,
  Loader2,
  Trash2,
  File,
  User,
  Timer,
  Video,
  Image,
  FileText,
  Music,
  Upload
} from 'lucide-vue-next'
import { geminiFilesApi, type FileMappingStatsResponse, type FileMappingResponse, type CapableKeyResponse } from '@/api/gemini-files'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

const { toast } = useToast()
const { legacyT } = useI18n()

// 状态
const loading = ref(false)
const stats = ref<FileMappingStatsResponse | null>(null)
const mappings = ref<FileMappingResponse[]>([])
const total = ref(0)
const currentPage = ref(1)
const pageSize = 20
const searchQuery = ref('')
const includeExpired = ref(false)

// 上传状态
const uploading = ref(false)
const isDragging = ref(false)
const fileInputRef = ref<HTMLInputElement | null>(null)
const capableKeys = ref<CapableKeyResponse[]>([])
const selectedKeyIds = ref<string[]>([])

// 计算属性
const totalPages = computed(() => Math.ceil(total.value / pageSize))

// 监听筛选条件变化
watch([searchQuery, includeExpired], () => {
  currentPage.value = 1
  fetchMappings()
})

watch(currentPage, () => {
  fetchMappings()
})

// 获取数据
async function fetchData() {
  await Promise.all([fetchStats(), fetchMappings(), fetchCapableKeys()])
}

async function fetchCapableKeys() {
  try {
    const keys = await geminiFilesApi.getCapableKeys()
    capableKeys.value = keys
    // 默认全选
    if (selectedKeyIds.value.length === 0 && keys.length > 0) {
      selectedKeyIds.value = keys.map(k => k.id)
    }
  } catch (error: unknown) {
    log.error('Failed to fetch capable keys', error)
  }
}

function toggleKeySelection(keyId: string) {
  const index = selectedKeyIds.value.indexOf(keyId)
  if (index === -1) {
    selectedKeyIds.value.push(keyId)
  } else {
    selectedKeyIds.value.splice(index, 1)
  }
}

function toggleSelectAll() {
  if (selectedKeyIds.value.length === capableKeys.value.length) {
    selectedKeyIds.value = []
  } else {
    selectedKeyIds.value = capableKeys.value.map(k => k.id)
  }
}

async function fetchStats() {
  try {
    const data = await geminiFilesApi.getStats()
    stats.value = data
  } catch (error: unknown) {
    toast({
      title: '获取统计失败',
      description: error instanceof Error ? error.message : String(error),
      variant: 'destructive'
    })
  }
}

async function fetchMappings() {
  loading.value = true
  try {
    const data = await geminiFilesApi.listMappings({
      page: currentPage.value,
      page_size: pageSize,
      include_expired: includeExpired.value,
      search: searchQuery.value || undefined
    })
    mappings.value = data.items
    total.value = data.total
  } catch (error: unknown) {
    toast({
      title: '获取文件列表失败',
      description: error instanceof Error ? error.message : String(error),
      variant: 'destructive'
    })
  } finally {
    loading.value = false
  }
}

async function deleteMapping(mapping: FileMappingResponse) {
  if (!confirm(legacyT(`确定要删除映射 "${mapping.file_name}" 吗？\n\n注意：这只会删除映射记录，不会删除 Google 上的实际文件。`))) {
    return
  }

  try {
    await geminiFilesApi.deleteMapping(mapping.id)
    toast({
      title: '删除成功',
      description: `已删除映射 ${mapping.file_name}`
    })
    await fetchData()
  } catch (error: unknown) {
    toast({
      title: '删除失败',
      description: error instanceof Error ? error.message : String(error),
      variant: 'destructive'
    })
  }
}

async function cleanupExpired() {
  if (!confirm(legacyT('确定要清理所有过期的文件映射吗？'))) {
    return
  }

  try {
    const result = await geminiFilesApi.cleanupExpired()
    toast({
      title: '清理完成',
      description: `已清理 ${result.deleted_count} 条过期映射`
    })
    await fetchData()
  } catch (error: unknown) {
    toast({
      title: '清理失败',
      description: error instanceof Error ? error.message : String(error),
      variant: 'destructive'
    })
  }
}

// 上传相关
async function uploadFile(file: globalThis.File) {
  if (selectedKeyIds.value.length === 0) {
    toast({
      title: '请选择 Key',
      description: '请至少选择一个 Key 来上传文件',
      variant: 'destructive'
    })
    return
  }

  uploading.value = true
  let hasSuccess = false
  try {
    const result = await geminiFilesApi.uploadFile(file, selectedKeyIds.value)
    if (result.fail_count === 0) {
      toast({
        title: '上传成功',
        description: `文件 ${result.display_name} 已上传到 ${result.success_count} 个 Key`
      })
      hasSuccess = true
    } else if (result.success_count > 0) {
      toast({
        title: '部分成功',
        description: `成功 ${result.success_count} 个，失败 ${result.fail_count} 个`
      })
      hasSuccess = true
    } else {
      const errors = result.results.map(r => r.error).filter(Boolean).join('; ')
      toast({
        title: '上传失败',
        description: errors || '所有 Key 上传都失败了',
        variant: 'destructive'
      })
    }
  } catch (error: unknown) {
    toast({
      title: '上传失败',
      description: parseApiError(error, '上传失败'),
      variant: 'destructive'
    })
  } finally {
    uploading.value = false
    isDragging.value = false
    // 有成功上传时刷新列表，并重置到第一页
    if (hasSuccess) {
      currentPage.value = 1
      await fetchData()
    }
  }
}

function handleDrop(e: DragEvent) {
  isDragging.value = false
  const files = e.dataTransfer?.files
  if (files && files.length > 0) {
    uploadFile(files[0])
  }
}

function handleFileSelect(e: Event) {
  const input = e.target as HTMLInputElement
  if (input.files && input.files.length > 0) {
    uploadFile(input.files[0])
    input.value = '' // 清空以便重复选择同一文件
  }
}

// 工具函数
function formatDate(dateStr: string) {
  if (!dateStr) return '-'
  const date = new Date(dateStr)
  return date.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit'
  })
}

function getFileIcon(mimeType: string | null) {
  if (!mimeType) return File
  if (mimeType.startsWith('video/')) return Video
  if (mimeType.startsWith('image/')) return Image
  if (mimeType.startsWith('audio/')) return Music
  if (mimeType.startsWith('text/') || mimeType.includes('pdf')) return FileText
  return File
}

// 初始化
onMounted(() => {
  fetchData()
})
</script>
