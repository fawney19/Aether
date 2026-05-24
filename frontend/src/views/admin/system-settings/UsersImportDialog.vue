<template>
  <!-- 用户数据导入对话框 -->
  <Dialog
    :open="importUsersDialogOpen"
    title="导入用户数据"
    description="选择冲突处理模式并确认导入"
    @update:open="$emit('update:importUsersDialogOpen', $event)"
  >
    <div class="space-y-4">
      <div
        v-if="importUsersPreview"
        class="text-sm"
      >
        <p class="font-medium mb-2">
          数据预览
        </p>
        <ul class="space-y-1 text-muted-foreground">
          <li v-if="importUsersPreview.user_groups?.length">
            用户组: {{ importUsersPreview.user_groups.length }} 个
          </li>
          <li>用户: {{ importUsersPreview.users?.length || 0 }} 个</li>
          <li>
            API Keys: {{ importUsersPreview.users?.reduce((sum: number, u: { api_keys?: unknown[] }) => sum + (u.api_keys?.length || 0), 0) }} 个
          </li>
          <li v-if="importUsersPreview.standalone_keys?.length">
            独立余额 Keys: {{ importUsersPreview.standalone_keys.length }} 个
          </li>
        </ul>
      </div>

      <div>
        <Label class="block text-sm font-medium mb-2">冲突处理模式</Label>
        <Select
          :model-value="usersMergeMode"
          :open="usersMergeModeSelectOpen"
          @update:model-value="$emit('update:usersMergeMode', $event)"
          @update:open="$emit('update:usersMergeModeSelectOpen', $event)"
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="skip">
              跳过 - 保留现有用户
            </SelectItem>
            <SelectItem value="overwrite">
              覆盖 - 用导入数据替换
            </SelectItem>
            <SelectItem value="error">
              报错 - 遇到冲突时中止
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="mt-1 text-xs text-muted-foreground">
          <template v-if="usersMergeMode === 'skip'">
            已存在的用户将被保留，仅导入新用户
          </template>
          <template v-else-if="usersMergeMode === 'overwrite'">
            已存在的用户将被导入的数据覆盖
          </template>
          <template v-else>
            如果发现任何冲突，导入将中止并回滚
          </template>
        </p>
      </div>

      <p class="text-xs text-muted-foreground">
        注意：用户 API Keys 需要目标系统使用相同的 ENCRYPTION_KEY 环境变量才能正常工作。
      </p>

      <div
        v-if="importUsersProgress"
        class="space-y-2 rounded-md border border-border p-3"
      >
        <div class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
          <span>{{ importUsersProgress.message }}</span>
          <span>{{ importUsersProgress.percent }}%</span>
        </div>
        <div class="h-1.5 overflow-hidden rounded-full bg-muted">
          <div
            class="h-full bg-primary transition-all"
            :style="{ width: `${importUsersProgress.percent}%` }"
          />
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="$emit('update:importUsersDialogOpen', false); $emit('update:usersMergeModeSelectOpen', false)"
      >
        取消
      </Button>
      <Button
        :disabled="importUsersLoading"
        @click="$emit('confirm')"
      >
        {{ importUsersLoading ? '导入中...' : '确认导入' }}
      </Button>
    </template>
  </Dialog>

  <!-- 用户数据导入结果对话框 -->
  <Dialog
    :open="importUsersResultDialogOpen"
    title="用户数据导入完成"
    @update:open="$emit('update:importUsersResultDialogOpen', $event)"
  >
    <div
      v-if="importUsersResult"
      class="space-y-4"
    >
      <div class="grid grid-cols-2 gap-4 text-sm">
        <div v-if="importUsersResult.stats.user_groups">
          <p class="font-medium">
            用户组
          </p>
          <p class="text-muted-foreground">
            创建: {{ importUsersResult.stats.user_groups.created }},
            更新: {{ importUsersResult.stats.user_groups.updated }},
            跳过: {{ importUsersResult.stats.user_groups.skipped }}
          </p>
        </div>
        <div>
          <p class="font-medium">
            用户
          </p>
          <p class="text-muted-foreground">
            创建: {{ importUsersResult.stats.users.created }},
            更新: {{ importUsersResult.stats.users.updated }},
            跳过: {{ importUsersResult.stats.users.skipped }}
          </p>
        </div>
        <div>
          <p class="font-medium">
            API Keys
          </p>
          <p class="text-muted-foreground">
            创建: {{ importUsersResult.stats.api_keys.created }},
            跳过: {{ importUsersResult.stats.api_keys.skipped }}
          </p>
        </div>
        <div
          v-if="importUsersResult.stats.standalone_keys"
          class="col-span-2"
        >
          <p class="font-medium">
            独立余额 Keys
          </p>
          <p class="text-muted-foreground">
            创建: {{ importUsersResult.stats.standalone_keys.created }},
            跳过: {{ importUsersResult.stats.standalone_keys.skipped }}
          </p>
        </div>
      </div>

      <div
        v-if="importUsersResult.stats.errors.length > 0"
        class="p-3 bg-destructive/10 rounded-lg"
      >
        <p class="font-medium text-destructive mb-2">
          警告信息
        </p>
        <ul class="text-sm text-destructive space-y-1">
          <li
            v-for="(err, index) in importUsersResult.stats.errors"
            :key="index"
          >
            {{ err }}
          </li>
        </ul>
      </div>
    </div>

    <template #footer>
      <Button @click="$emit('update:importUsersResultDialogOpen', false)">
        确定
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import Button from '@/components/ui/button.vue'
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { Dialog } from '@/components/ui'
import type { UsersExportData, UsersImportResponse } from '@/api/admin'
import type { ImportProgressState } from './composables/useConfigExportImport'

defineProps<{
  importUsersDialogOpen: boolean
  importUsersResultDialogOpen: boolean
  importUsersPreview: UsersExportData | null
  importUsersResult: UsersImportResponse | null
  usersMergeMode: 'skip' | 'overwrite' | 'error'
  usersMergeModeSelectOpen: boolean
  importUsersLoading: boolean
  importUsersProgress: ImportProgressState | null
}>()

defineEmits<{
  confirm: []
  'update:importUsersDialogOpen': [value: boolean]
  'update:importUsersResultDialogOpen': [value: boolean]
  'update:usersMergeMode': [value: 'skip' | 'overwrite' | 'error']
  'update:usersMergeModeSelectOpen': [value: boolean]
}>()
</script>
