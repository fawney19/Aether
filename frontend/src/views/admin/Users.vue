<template>
  <div class="space-y-6 pb-8">
    <!-- 用户表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和筛选器 -->
      <div class="px-4 sm:px-6 py-3.5 border-b border-border/60">
        <!-- 移动端：标题行 + 筛选器行 -->
        <div class="flex flex-col gap-3 sm:hidden">
          <div class="flex items-center justify-between">
            <h3 class="text-base font-semibold">
              用户管理
            </h3>
            <div class="flex items-center gap-2">
              <!-- 新增用户按钮 -->
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="新增用户"
                @click="openCreateDialog"
              >
                <Plus class="w-3.5 h-3.5" />
              </Button>
              <!-- 刷新按钮 -->
              <RefreshButton
                :loading="usersStore.loading || loadingStats"
                @click="refreshUsers"
              />
            </div>
          </div>
          <!-- 筛选器 -->
          <div class="flex items-center gap-2">
            <div class="relative flex-1">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                id="users-search-mobile"
                v-model="searchQuery"
                type="text"
                placeholder="搜索..."
                class="w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
              />
            </div>
            <Select
              v-model="filterRole"
              v-model:open="filterRoleOpenMobile"
            >
              <SelectTrigger class="w-24 h-8 text-xs border-border/60">
                <SelectValue placeholder="角色" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部
                </SelectItem>
                <SelectItem value="admin">
                  管理员
                </SelectItem>
                <SelectItem value="user">
                  用户
                </SelectItem>
              </SelectContent>
            </Select>
            <Select
              v-model="filterStatus"
              v-model:open="filterStatusOpenMobile"
            >
              <SelectTrigger class="w-20 h-8 text-xs border-border/60">
                <SelectValue placeholder="状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部
                </SelectItem>
                <SelectItem value="active">
                  活跃
                </SelectItem>
                <SelectItem value="inactive">
                  禁用
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <!-- 桌面端：单行布局 -->
        <div class="hidden sm:flex items-center justify-between gap-4">
          <h3 class="text-base font-semibold">
            用户管理
          </h3>

          <!-- 筛选器和操作按钮 -->
          <div class="flex items-center gap-2">
            <!-- 搜索框 -->
            <div class="relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                id="users-search"
                v-model="searchQuery"
                type="text"
                placeholder="搜索用户名或邮箱..."
                class="w-48 pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60 focus:border-primary/40 transition-colors"
              />
            </div>

            <!-- 分隔线 -->
            <div class="h-4 w-px bg-border" />

            <!-- 角色筛选 -->
            <Select
              v-model="filterRole"
              v-model:open="filterRoleOpen"
            >
              <SelectTrigger class="w-32 h-8 text-xs border-border/60">
                <SelectValue placeholder="全部角色" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部角色
                </SelectItem>
                <SelectItem value="admin">
                  管理员
                </SelectItem>
                <SelectItem value="user">
                  普通用户
                </SelectItem>
              </SelectContent>
            </Select>

            <!-- 状态筛选 -->
            <Select
              v-model="filterStatus"
              v-model:open="filterStatusOpen"
            >
              <SelectTrigger class="w-28 h-8 text-xs border-border/60">
                <SelectValue placeholder="全部状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部状态
                </SelectItem>
                <SelectItem value="active">
                  活跃
                </SelectItem>
                <SelectItem value="inactive">
                  禁用
                </SelectItem>
              </SelectContent>
            </Select>

            <!-- 分隔线 -->
            <div class="h-4 w-px bg-border" />

            <!-- 新增用户按钮 -->
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="新增用户"
              @click="openCreateDialog"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>

            <!-- 刷新按钮 -->
            <RefreshButton
              :loading="usersStore.loading || loadingStats"
              @click="refreshUsers"
            />
          </div>
        </div>
      </div>

      <!-- 桌面端表格 -->
      <div class="hidden xl:block overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow class="border-b border-border/60 hover:bg-transparent">
              <TableHead class="w-[200px] h-12 font-semibold">
                用户信息
              </TableHead>
              <TableHead class="w-[180px] h-12 font-semibold">
                邮箱
              </TableHead>
              <TableHead class="w-[180px] h-12 font-semibold">
                使用统计
              </TableHead>
              <TableHead class="w-[180px] h-12 font-semibold">
                配额(美元)
              </TableHead>
              <TableHead class="w-[110px] h-12 font-semibold">
                创建时间
              </TableHead>
              <TableHead class="w-[90px] h-12 font-semibold text-center">
                状态
              </TableHead>
              <TableHead class="w-[220px] h-12 font-semibold text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="user in paginatedUsers"
              :key="user.id"
              class="border-b border-border/40 hover:bg-muted/30 transition-colors"
            >
              <TableCell class="py-4">
                <div class="flex items-center gap-3">
                  <Avatar class="h-10 w-10 ring-2 ring-background shadow-md">
                    <AvatarFallback class="bg-primary text-sm font-bold text-white">
                      {{ user.username.charAt(0).toUpperCase() }}
                    </AvatarFallback>
                  </Avatar>
                  <div class="flex-1 min-w-0">
                    <div
                      class="truncate text-sm font-semibold mb-1"
                      :title="user.username"
                    >
                      {{ user.username }}
                    </div>
                    <Badge
                      :variant="user.role === 'admin' ? 'default' : 'secondary'"
                      class="text-xs px-2 py-0.5"
                    >
                      {{ user.role === 'admin' ? '管理员' : '普通用户' }}
                    </Badge>
                  </div>
                </div>
              </TableCell>
              <TableCell class="py-4">
                <span
                  class="block truncate text-sm text-muted-foreground"
                  :title="user.email || '-'"
                >
                  {{ user.email || '-' }}
                </span>
              </TableCell>
              <TableCell class="py-4">
                <div
                  v-if="userStats[user.id]"
                  class="space-y-1 text-xs"
                >
                  <div class="flex items-center text-muted-foreground">
                    <span class="w-14">请求:</span>
                    <span class="font-medium text-foreground">{{ formatNumber(userStats[user.id]?.request_count) }}</span>
                  </div>
                  <div class="flex items-center text-muted-foreground">
                    <span class="w-14">Tokens:</span>
                    <span class="font-medium text-foreground">{{ formatTokens(userStats[user.id]?.total_tokens ?? 0) }}</span>
                  </div>
                </div>
                <div
                  v-else
                  class="text-xs text-muted-foreground"
                >
                  <span v-if="loadingStats">加载中...</span>
                  <span v-else>无数据</span>
                </div>
              </TableCell>
              <TableCell class="py-4">
                <div class="space-y-1.5 text-xs">
                  <div
                    v-if="user.quota_usd != null"
                    class="text-muted-foreground"
                  >
                    当前: <span class="font-semibold text-foreground">${{ (user.used_usd || 0).toFixed(2) }}</span> / <span class="font-medium">${{ user.quota_usd.toFixed(2) }}</span>
                  </div>
                  <div
                    v-else
                    class="text-muted-foreground"
                  >
                    当前: <span class="font-semibold text-foreground">${{ (user.used_usd || 0).toFixed(2) }}</span> / <span class="font-medium text-amber-600">无限制</span>
                  </div>
                  <div class="text-muted-foreground">
                    累计: <span class="font-medium text-foreground">${{ (user.total_usd || 0).toFixed(2) }}</span>
                  </div>
                </div>
              </TableCell>
              <TableCell class="py-4 text-xs text-muted-foreground">
                {{ formatDate(user.created_at) }}
              </TableCell>
              <TableCell class="py-4 text-center">
                <Badge
                  :variant="user.is_active ? 'success' : 'destructive'"
                  class="font-medium px-3 py-1"
                >
                  {{ user.is_active ? '活跃' : '禁用' }}
                </Badge>
              </TableCell>
              <TableCell class="py-4">
                <div class="flex justify-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="编辑用户"
                    @click="editUser(user)"
                  >
                    <SquarePen class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="查看API Keys"
                    @click="manageApiKeys(user)"
                  >
                    <Key class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="user.is_active ? '禁用用户' : '启用用户'"
                    @click="toggleUserStatus(user)"
                  >
                    <PauseCircle
                      v-if="user.is_active"
                      class="h-4 w-4"
                    />
                    <PlayCircle
                      v-else
                      class="h-4 w-4"
                    />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="重置配额"
                    @click="resetQuota(user)"
                  >
                    <RotateCcw class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="删除用户"
                    @click="deleteUser(user)"
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
      <div class="xl:hidden divide-y divide-border/40">
        <div
          v-for="user in paginatedUsers"
          :key="user.id"
          class="p-4 sm:p-5 hover:bg-muted/30 transition-colors"
        >
          <!-- 用户头部 -->
          <div class="flex items-start justify-between mb-3 sm:mb-4">
            <div class="flex items-center gap-2 sm:gap-3">
              <Avatar class="h-10 w-10 sm:h-12 sm:w-12 ring-2 ring-background shadow-md flex-shrink-0">
                <AvatarFallback class="bg-primary text-sm sm:text-base font-bold text-white">
                  {{ user.username.charAt(0).toUpperCase() }}
                </AvatarFallback>
              </Avatar>
              <div class="min-w-0">
                <div class="font-semibold text-sm sm:text-base mb-1 truncate">
                  {{ user.username }}
                </div>
                <Badge
                  :variant="user.role === 'admin' ? 'default' : 'secondary'"
                  class="text-xs"
                >
                  {{ user.role === 'admin' ? '管理员' : '普通用户' }}
                </Badge>
              </div>
            </div>
            <Badge
              :variant="user.is_active ? 'success' : 'destructive'"
              class="font-medium text-xs flex-shrink-0"
            >
              {{ user.is_active ? '活跃' : '禁用' }}
            </Badge>
          </div>

          <!-- 用户信息 -->
          <div class="space-y-2 sm:space-y-3 mb-3 sm:mb-4">
            <div class="text-xs sm:text-sm">
              <span class="text-muted-foreground">邮箱:</span>
              <span class="ml-2 text-foreground truncate block sm:inline">{{ user.email || '-' }}</span>
            </div>

            <div
              v-if="userStats[user.id]"
              class="grid grid-cols-2 gap-2 p-2 sm:p-3 bg-muted/50 rounded-lg text-xs"
            >
              <div>
                <div class="text-muted-foreground mb-1">
                  请求次数
                </div>
                <div class="font-semibold text-sm text-foreground">
                  {{ formatNumber(userStats[user.id]?.request_count) }}
                </div>
              </div>
              <div>
                <div class="text-muted-foreground mb-1">
                  Tokens
                </div>
                <div class="font-semibold text-sm text-foreground">
                  {{ formatTokens(userStats[user.id]?.total_tokens ?? 0) }}
                </div>
              </div>
            </div>

            <div class="p-2 sm:p-3 bg-muted/50 rounded-lg text-xs space-y-1">
              <div v-if="user.quota_usd != null">
                <span class="text-muted-foreground">当前配额:</span>
                <span class="ml-2 font-semibold text-sm">${{ (user.used_usd || 0).toFixed(2) }}</span> / ${{ user.quota_usd.toFixed(2) }}
              </div>
              <div v-else>
                <span class="text-muted-foreground">当前配额:</span>
                <span class="ml-2 font-semibold text-sm">${{ (user.used_usd || 0).toFixed(2) }}</span> / <span class="text-amber-600">无限制</span>
              </div>
              <div>
                <span class="text-muted-foreground">累计消费:</span>
                <span class="ml-2 font-semibold text-sm">${{ (user.total_usd || 0).toFixed(2) }}</span>
              </div>
              <div>
                <span class="text-muted-foreground">创建时间:</span>
                <span class="ml-2 text-sm">{{ formatDate(user.created_at) }}</span>
              </div>
            </div>
          </div>

          <!-- 操作按钮 - 响应式布局 -->
          <div class="grid grid-cols-2 sm:flex sm:flex-wrap gap-1.5 sm:gap-2">
            <Button
              variant="outline"
              size="sm"
              class="text-xs sm:text-sm h-8 sm:h-9 sm:flex-1 sm:min-w-[90px]"
              @click="editUser(user)"
            >
              <SquarePen class="h-3 w-3 sm:h-3.5 sm:w-3.5 sm:mr-1.5" />
              <span class="hidden sm:inline">编辑</span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              class="text-xs sm:text-sm h-8 sm:h-9 sm:flex-1 sm:min-w-[100px]"
              @click="manageApiKeys(user)"
            >
              <Key class="h-3 w-3 sm:h-3.5 sm:w-3.5 sm:mr-1.5" />
              <span class="hidden sm:inline">API Keys</span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              class="text-xs sm:text-sm h-8 sm:h-9 sm:flex-1 sm:min-w-[90px]"
              :class="user.is_active ? 'text-amber-600' : 'text-emerald-600'"
              @click="toggleUserStatus(user)"
            >
              <PauseCircle
                v-if="user.is_active"
                class="h-3 w-3 sm:h-3.5 sm:w-3.5 sm:mr-1.5"
              />
              <PlayCircle
                v-else
                class="h-3 w-3 sm:h-3.5 sm:w-3.5 sm:mr-1.5"
              />
              <span class="hidden sm:inline">{{ user.is_active ? '禁用' : '启用' }}</span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              class="text-xs sm:text-sm h-8 sm:h-9"
              @click="resetQuota(user)"
            >
              <RotateCcw class="h-3 w-3 sm:h-3.5 sm:w-3.5 sm:mr-1.5" />
              <span class="hidden sm:inline">重置</span>
            </Button>
            <Button
              variant="outline"
              size="sm"
              class="col-span-2 text-xs sm:text-sm h-8 sm:h-9 text-rose-600 sm:col-span-1"
              @click="deleteUser(user)"
            >
              <Trash2 class="h-3 w-3 sm:h-3.5 sm:w-3.5 sm:mr-1.5" />
              <span class="hidden sm:inline">删除</span>
            </Button>
          </div>
        </div>
      </div>

      <!-- 分页控件 -->
      <Pagination
        :current="currentPage"
        :total="filteredUsers.length"
        :page-size="pageSize"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>

    <!-- 用户表单对话框（创建/编辑共用） -->
    <UserFormDialog
      ref="userFormDialogRef"
      :open="showUserFormDialog"
      :user="editingUser"
      @close="closeUserFormDialog"
      @submit="handleUserFormSubmit"
    />

    <!-- API Keys 管理对话框 -->
    <Dialog
      v-model="showApiKeysDialog"
      size="xl"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-kraft/10 flex-shrink-0">
              <Key class="h-5 w-5 text-kraft" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                管理 API Keys
              </h3>
              <p class="text-xs text-muted-foreground">
                查看和管理用户的 API 密钥
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="max-h-[60vh] overflow-y-auto space-y-3">
        <template v-if="userApiKeys.length > 0">
          <div
            v-for="apiKey in userApiKeys"
            :key="apiKey.id"
            class="rounded-lg border border-border bg-card p-4 hover:border-primary/30 transition-colors"
          >
            <div class="flex items-center justify-between gap-3">
              <!-- 左侧信息 -->
              <div class="flex items-center gap-3 min-w-0 flex-1">
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2 flex-wrap">
                    <span class="font-semibold text-foreground">
                      {{ apiKey.name || '未命名 API Key' }}
                    </span>
                    <Badge
                      :variant="apiKey.is_active ? 'success' : 'secondary'"
                      class="text-xs"
                    >
                      {{ apiKey.is_active ? '活跃' : '已禁用' }}
                    </Badge>
                    <Badge
                      v-if="apiKey.is_standalone"
                      variant="default"
                      class="text-xs bg-purple-500"
                    >
                      独立余额
                    </Badge>
                  </div>
                  <div class="flex items-center gap-1 mt-0.5">
                    <code class="text-xs font-mono text-muted-foreground">
                      {{ apiKey.key_display || 'sk-****' }}
                    </code>
                    <button
                      class="p-0.5 hover:bg-muted rounded transition-colors"
                      title="复制完整密钥"
                      @click="copyFullKey(apiKey)"
                    >
                      <Copy class="w-3 h-3 text-muted-foreground" />
                    </button>
                  </div>
                </div>
              </div>
              <!-- 右侧统计和操作 -->
              <div class="flex items-center gap-4 flex-shrink-0">
                <div class="text-right text-sm">
                  <div class="text-muted-foreground">
                    {{ (apiKey.total_requests || 0).toLocaleString() }} 次
                  </div>
                  <div class="font-semibold text-rose-600">
                    ${{ (apiKey.total_cost_usd || 0).toFixed(4) }}
                  </div>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="删除"
                  @click="deleteApiKey(apiKey)"
                >
                  <Trash2 class="h-4 w-4" />
                </Button>
              </div>
            </div>
          </div>
        </template>
        <div
          v-else
          class="rounded-lg border-2 border-dashed border-muted-foreground/20 bg-muted/20 px-4 py-12 text-center"
        >
          <div class="flex flex-col items-center gap-3">
            <div class="flex h-14 w-14 items-center justify-center rounded-full bg-muted">
              <Key class="h-6 w-6 text-muted-foreground/50" />
            </div>
            <div>
              <p class="mb-1 text-base font-semibold text-foreground">
                暂无 API Keys
              </p>
              <p class="text-sm text-muted-foreground">
                点击下方按钮创建
              </p>
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-10 px-5"
          @click="showApiKeysDialog = false"
        >
          取消
        </Button>
        <Button
          class="h-10 px-5"
          :disabled="creatingApiKey"
          @click="createApiKey"
        >
          {{ creatingApiKey ? '创建中...' : '创建' }}
        </Button>
      </template>
    </Dialog>

    <!-- 新 API Key 显示对话框 -->
    <Dialog
      v-model="showNewApiKeyDialog"
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
                请妥善保管, 切勿泄露给他人.
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label class="text-sm font-medium">API Key</Label>
          <div class="flex items-center gap-2">
            <Input
              ref="apiKeyInput"
              type="text"
              :value="newApiKey"
              readonly
              class="flex-1 font-mono text-sm bg-muted/50 h-11"
              @click="selectApiKey"
            />
            <Button
              class="h-11"
              @click="copyApiKey"
            >
              复制
            </Button>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          class="h-10 px-5"
          @click="closeNewApiKeyDialog"
        >
          确定
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useUsersStore } from '@/stores/users'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { usageApi, type UsageByUser } from '@/api/usage'
import { adminApi } from '@/api/admin'

// UI 组件
import {
  Dialog,
  Card,
  Button,
  Badge,
  Input,
  Label,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Avatar,
  AvatarFallback,
  Pagination,
  RefreshButton
} from '@/components/ui'

import {
  Plus,
  SquarePen,
  Key,
  PauseCircle,
  PlayCircle,
  RotateCcw,
  Trash2,
  Copy,
  Search,
  CheckCircle
} from 'lucide-vue-next'

// 功能组件
import UserFormDialog, { type UserFormData } from '@/features/users/components/UserFormDialog.vue'
import { log } from '@/utils/logger'

const { success, error } = useToast()
const { confirmDanger, confirmWarning } = useConfirm()
const usersStore = useUsersStore()

// 用户表单对话框状态
const showUserFormDialog = ref(false)
const editingUser = ref<UserFormData | null>(null)
const userFormDialogRef = ref<InstanceType<typeof UserFormDialog>>()

// API Keys 对话框状态
const showApiKeysDialog = ref(false)
const showNewApiKeyDialog = ref(false)
const selectedUser = ref<any>(null)
const userApiKeys = ref<any[]>([])
const newApiKey = ref('')
const creatingApiKey = ref(false)
const apiKeyInput = ref<HTMLInputElement>()

// 用户统计
const userStats = ref<Record<string, UsageByUser>>({})
const loadingStats = ref(false)

const searchQuery = ref('')
const filterRole = ref('all')
const filterStatus = ref('all')
const filterRoleOpen = ref(false)
const filterStatusOpen = ref(false)
const filterRoleOpenMobile = ref(false)
const filterStatusOpenMobile = ref(false)

const currentPage = ref(1)
const pageSize = ref(20)

const filteredUsers = computed(() => {
  let filtered = [...usersStore.users]

  // 先排序：管理员优先，然后按创建时间倒序
  filtered.sort((a, b) => {
    // 管理员优先
    if (a.role === 'admin' && b.role !== 'admin') return -1
    if (a.role !== 'admin' && b.role === 'admin') return 1
    // 同角色按创建时间倒序（新用户在前）
    return new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
  })

  // 搜索（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    filtered = filtered.filter(u => {
      const searchableText = `${u.username} ${u.email || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  if (filterRole.value !== 'all') {
    filtered = filtered.filter(u => u.role === filterRole.value)
  }

  if (filterStatus.value !== 'all') {
    filtered = filtered.filter(u =>
      filterStatus.value === 'active' ? u.is_active : !u.is_active
    )
  }

  return filtered
})

const paginatedUsers = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredUsers.value.slice(start, start + pageSize.value)
})

// Watch filter changes and reset to first page
watch([searchQuery, filterRole, filterStatus], () => {
  currentPage.value = 1
})

onMounted(async () => {
  await usersStore.fetchUsers()
  await loadUserStats()
})

async function refreshUsers() {
  await usersStore.fetchUsers()
  await loadUserStats()
}

function formatDate(dateString: string) {
  return new Date(dateString).toLocaleDateString('zh-CN')
}

async function loadUserStats() {
  loadingStats.value = true
  try {
    const data = await usageApi.getUsageByUser()
    userStats.value = data.reduce((acc: any, stat: any) => {
      acc[stat.user_id] = stat
      return acc
    }, {})
  } catch (err) {
    log.error('加载用户统计失败:', err)
  } finally {
    loadingStats.value = false
  }
}

function formatTokens(tokens: number): string {
  if (tokens >= 1000000) {
    return `${(tokens / 1000000).toFixed(1)}M`
  } else if (tokens >= 1000) {
    return `${(tokens / 1000).toFixed(1)}K`
  }
  return tokens.toString()
}

function formatNumber(value?: number | null): string {
  const numericValue = typeof value === 'number' && Number.isFinite(value) ? value : 0
  return numericValue.toLocaleString()
}

function formatCurrency(value?: number | null, fractionDigits = 4): string {
  const numericValue = typeof value === 'number' && Number.isFinite(value) ? value : 0
  return numericValue.toFixed(fractionDigits)
}

async function toggleUserStatus(user: any) {
  const action = user.is_active ? '禁用' : '启用'
  const confirmed = await confirmDanger(
    `确定要${action}用户 ${user.username} 吗？`,
    `${action}用户`,
    action
  )

  if (!confirmed) return

  try {
    await usersStore.updateUser(user.id, { is_active: !user.is_active })
    success(`用户已${action}`)
  } catch (err: any) {
    error(err.response?.data?.error?.message || err.response?.data?.detail || '未知错误', `${action}用户失败`)
  }
}

// ========== 用户表单对话框方法 ==========

function openCreateDialog() {
  editingUser.value = null
  showUserFormDialog.value = true
}

function editUser(user: any) {
  editingUser.value = {
    id: user.id,
    username: user.username,
    email: user.email,
    quota_usd: user.quota_usd,
    role: user.role,
    is_active: user.is_active,
    allowed_providers: user.allowed_providers || [],
    allowed_endpoints: user.allowed_endpoints || [],
    allowed_models: user.allowed_models || []
  }
  showUserFormDialog.value = true
}

function closeUserFormDialog() {
  showUserFormDialog.value = false
  editingUser.value = null
}

async function handleUserFormSubmit(data: UserFormData & { password?: string }) {
  userFormDialogRef.value?.setSaving(true)
  try {
    if (data.id) {
      // 更新用户
      const updateData: any = {
        username: data.username,
        email: data.email || undefined,
        quota_usd: data.quota_usd,
        role: data.role,
        allowed_providers: data.allowed_providers,
        allowed_endpoints: data.allowed_endpoints,
        allowed_models: data.allowed_models
      }
      if (data.password) {
        updateData.password = data.password
      }
      await usersStore.updateUser(data.id, updateData)
      success('用户信息已更新')
    } else {
      // 创建用户
      const newUser = await usersStore.createUser({
        username: data.username,
        password: data.password!,
        email: data.email || undefined,
        quota_usd: data.quota_usd,
        role: data.role,
        allowed_providers: data.allowed_providers,
        allowed_endpoints: data.allowed_endpoints,
        allowed_models: data.allowed_models
      })
      // 如果创建时指定为禁用，则更新状态
      if (data.is_active === false && newUser) {
        await usersStore.updateUser(newUser.id, { is_active: false })
      }
      success('用户创建成功')
    }
    closeUserFormDialog()
  } catch (err: any) {
    const title = data.id ? '更新用户失败' : '创建用户失败'
    error(err.response?.data?.error?.message || err.response?.data?.detail || '未知错误', title)
  } finally {
    userFormDialogRef.value?.setSaving(false)
  }
}

async function manageApiKeys(user: any) {
  selectedUser.value = user
  showApiKeysDialog.value = true
  await loadUserApiKeys(user.id)
}

async function loadUserApiKeys(userId: string) {
  try {
    userApiKeys.value = await usersStore.getUserApiKeys(userId)
  } catch (err) {
    log.error('加载API Keys失败:', err)
    userApiKeys.value = []
  }
}

async function createApiKey() {
  if (!selectedUser.value) return

  creatingApiKey.value = true
  try {
    const response = await usersStore.createApiKey(
      selectedUser.value.id,
      `Key-${new Date().toISOString().split('T')[0]}`
    )
    newApiKey.value = response.key || ''
    showNewApiKeyDialog.value = true
    await loadUserApiKeys(selectedUser.value.id)
  } catch (err: any) {
    error(err.response?.data?.error?.message || err.response?.data?.detail || '未知错误', '创建 API Key 失败')
  } finally {
    creatingApiKey.value = false
  }
}

function selectApiKey() {
  apiKeyInput.value?.select()
}

async function copyApiKey() {
  try {
    await navigator.clipboard.writeText(newApiKey.value)
    success('API Key已复制到剪贴板')
  } catch {
    error('复制失败，请手动复制')
  }
}

async function closeNewApiKeyDialog() {
  showNewApiKeyDialog.value = false
  newApiKey.value = ''
}

async function deleteApiKey(apiKey: any) {
  const confirmed = await confirmDanger(
    `确定要删除这个API Key吗？\n\n${apiKey.key_display || 'sk-****'}\n\n此操作无法撤销。`,
    '删除 API Key'
  )

  if (!confirmed) return

  try {
    await usersStore.deleteApiKey(selectedUser.value.id, apiKey.id)
    await loadUserApiKeys(selectedUser.value.id)
    success('API Key已删除')
  } catch (err: any) {
    error(err.response?.data?.error?.message || err.response?.data?.detail || '未知错误', '删除 API Key 失败')
  }
}

async function copyFullKey(apiKey: any) {
  try {
    // 调用后端 API 获取完整密钥
    const response = await adminApi.getFullApiKey(apiKey.id)
    await navigator.clipboard.writeText(response.key)
    success('完整密钥已复制到剪贴板')
  } catch (err: any) {
    log.error('复制密钥失败:', err)
    error(err.response?.data?.error?.message || err.response?.data?.detail || '未知错误', '复制密钥失败')
  }
}

async function resetQuota(user: any) {
  const confirmed = await confirmWarning(
    `确定要重置用户 ${user.username} 的配额使用量吗？\n\n这将把已使用金额重置为0。`,
    '重置配额'
  )

  if (!confirmed) return

  try {
    await usersStore.resetUserQuota(user.id)
    success('配额已重置')
  } catch (err: any) {
    error(err.response?.data?.error?.message || err.response?.data?.detail || '未知错误', '重置配额失败')
  }
}

async function deleteUser(user: any) {
  const confirmed = await confirmDanger(
    `确定要删除用户 ${user.username} 吗？\n\n此操作将删除：\n• 用户账户\n• 所有API密钥\n• 所有使用记录\n\n此操作无法撤销！`,
    '删除用户'
  )

  if (!confirmed) return

  try {
    await usersStore.deleteUser(user.id)
    success('用户已删除')
  } catch (err: any) {
    error(err.response?.data?.error?.message || err.response?.data?.detail || '未知错误', '删除用户失败')
  }
}
</script>
