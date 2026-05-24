<template>
  <!-- 导入配置对话框 -->
  <Dialog
    :open="importDialogOpen"
    title="导入配置"
    description="选择冲突处理模式并确认导入"
    @update:open="$emit('update:importDialogOpen', $event)"
  >
    <div class="space-y-4">
      <div
        v-if="importPreview"
        class="text-sm"
      >
        <p class="font-medium mb-2">
          配置预览
        </p>
        <ul class="space-y-1 text-muted-foreground">
          <li>全局模型: {{ importPreview.global_models?.length || 0 }} 个</li>
          <li>提供商: {{ importPreview.providers?.length || 0 }} 个</li>
          <li>
            端点: {{ importPreview.providers?.reduce((sum: number, p: { endpoints?: unknown[] }) => sum + (p.endpoints?.length || 0), 0) }} 个
          </li>
          <li>
            API Keys: {{ importPreview.providers?.reduce((sum: number, p: { api_keys?: unknown[] }) => sum + (p.api_keys?.length || 0), 0) }} 个
          </li>
          <li v-if="importPreview.proxy_nodes?.length">
            代理节点: {{ importPreview.proxy_nodes.length }} 个
          </li>
          <li v-if="importPreview.ldap_config">
            LDAP 配置: 1 个
          </li>
          <li v-if="importPreview.oauth_providers?.length">
            OAuth Providers: {{ importPreview.oauth_providers.length }} 个
          </li>
        </ul>
      </div>

      <div>
        <Label class="block text-sm font-medium mb-2">冲突处理模式</Label>
        <Select
          :model-value="mergeMode"
          :open="mergeModeSelectOpen"
          @update:model-value="$emit('update:mergeMode', $event)"
          @update:open="$emit('update:mergeModeSelectOpen', $event)"
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="skip">
              跳过 - 保留现有配置
            </SelectItem>
            <SelectItem value="overwrite">
              覆盖 - 用导入配置替换
            </SelectItem>
            <SelectItem value="error">
              报错 - 遇到冲突时中止
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="mt-1 text-xs text-muted-foreground">
          <template v-if="mergeMode === 'skip'">
            已存在的配置将被保留，仅导入新配置
          </template>
          <template v-else-if="mergeMode === 'overwrite'">
            已存在的配置将被导入的配置覆盖
          </template>
          <template v-else>
            如果发现任何冲突，导入将中止并回滚
          </template>
        </p>
      </div>

      <p class="text-xs text-muted-foreground">
        注意：相同的 API Keys 会自动跳过，不会创建重复记录。
      </p>

      <div
        v-if="importProgress"
        class="space-y-2 rounded-md border border-border p-3"
      >
        <div class="flex items-center justify-between gap-3 text-xs text-muted-foreground">
          <span>{{ importProgress.message }}</span>
          <span>{{ importProgress.percent }}%</span>
        </div>
        <div class="h-1.5 overflow-hidden rounded-full bg-muted">
          <div
            class="h-full bg-primary transition-all"
            :style="{ width: `${importProgress.percent}%` }"
          />
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="$emit('update:importDialogOpen', false); $emit('update:mergeModeSelectOpen', false)"
      >
        取消
      </Button>
      <Button
        :disabled="importLoading"
        @click="$emit('confirm')"
      >
        {{ importLoading ? '导入中...' : '确认导入' }}
      </Button>
    </template>
  </Dialog>

  <!-- 导入结果对话框 -->
  <Dialog
    :open="importResultDialogOpen"
    title="导入完成"
    @update:open="$emit('update:importResultDialogOpen', $event)"
  >
    <div
      v-if="importResult"
      class="space-y-4"
    >
      <div class="grid grid-cols-2 gap-4 text-sm">
        <div>
          <p class="font-medium">
            全局模型
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.global_models.created }},
            更新: {{ importResult.stats.global_models.updated }},
            跳过: {{ importResult.stats.global_models.skipped }}
          </p>
        </div>
        <div>
          <p class="font-medium">
            提供商
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.providers.created }},
            更新: {{ importResult.stats.providers.updated }},
            跳过: {{ importResult.stats.providers.skipped }}
          </p>
        </div>
        <div>
          <p class="font-medium">
            端点
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.endpoints.created }},
            更新: {{ importResult.stats.endpoints.updated }},
            跳过: {{ importResult.stats.endpoints.skipped }}
          </p>
        </div>
        <div>
          <p class="font-medium">
            API Keys
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.keys.created }},
            跳过: {{ importResult.stats.keys.skipped }}
          </p>
        </div>
        <div class="col-span-2">
          <p class="font-medium">
            模型配置
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.models.created }},
            更新: {{ importResult.stats.models.updated }},
            跳过: {{ importResult.stats.models.skipped }}
          </p>
        </div>
        <div v-if="importResult.stats.ldap">
          <p class="font-medium">
            LDAP 配置
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.ldap.created }},
            更新: {{ importResult.stats.ldap.updated }},
            跳过: {{ importResult.stats.ldap.skipped }}
          </p>
        </div>
        <div v-if="importResult.stats.oauth">
          <p class="font-medium">
            OAuth Providers
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.oauth.created }},
            更新: {{ importResult.stats.oauth.updated }},
            跳过: {{ importResult.stats.oauth.skipped }}
          </p>
        </div>
        <div v-if="importResult.stats.proxy_nodes">
          <p class="font-medium">
            代理节点
          </p>
          <p class="text-muted-foreground">
            创建: {{ importResult.stats.proxy_nodes.created }},
            更新: {{ importResult.stats.proxy_nodes.updated }},
            跳过: {{ importResult.stats.proxy_nodes.skipped }}
          </p>
        </div>
      </div>

      <div
        v-if="importResult.stats.errors.length > 0"
        class="p-3 bg-destructive/10 rounded-lg"
      >
        <p class="font-medium text-destructive mb-2">
          警告信息
        </p>
        <ul class="text-sm text-destructive space-y-1">
          <li
            v-for="(err, index) in importResult.stats.errors"
            :key="index"
          >
            {{ err }}
          </li>
        </ul>
      </div>
    </div>

    <template #footer>
      <Button @click="$emit('update:importResultDialogOpen', false)">
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
import type { ConfigExportData, ConfigImportResponse } from '@/api/admin'
import type { ImportProgressState } from './composables/useConfigExportImport'

defineProps<{
  importDialogOpen: boolean
  importResultDialogOpen: boolean
  importPreview: ConfigExportData | null
  importResult: ConfigImportResponse | null
  mergeMode: 'skip' | 'overwrite' | 'error'
  mergeModeSelectOpen: boolean
  importLoading: boolean
  importProgress: ImportProgressState | null
}>()

defineEmits<{
  confirm: []
  'update:importDialogOpen': [value: boolean]
  'update:importResultDialogOpen': [value: boolean]
  'update:mergeMode': [value: 'skip' | 'overwrite' | 'error']
  'update:mergeModeSelectOpen': [value: boolean]
}>()
</script>
