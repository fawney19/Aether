<template>
  <PageContainer>
    <PageHeader
      title="系统设置"
      description="管理系统级别的配置和参数"
    >
      <template #actions>
        <Button
          :disabled="loading"
          class="shadow-none hover:shadow-none"
          @click="saveSystemConfig"
        >
          {{ loading ? '保存中...' : '保存所有配置' }}
        </Button>
      </template>
    </PageHeader>

    <div class="mt-6 space-y-6">
      <!-- 配置导出/导入 -->
      <CardSection
        title="配置管理"
        description="导出或导入提供商和模型配置，便于备份或迁移"
      >
        <div class="flex flex-wrap gap-4">
          <div class="flex-1 min-w-[200px]">
            <p class="text-sm text-muted-foreground mb-3">
              导出当前所有提供商、端点、API Key 和模型配置到 JSON 文件
            </p>
            <Button
              variant="outline"
              :disabled="exportLoading"
              @click="handleExportConfig"
            >
              <Download class="w-4 h-4 mr-2" />
              {{ exportLoading ? '导出中...' : '导出配置' }}
            </Button>
          </div>
          <div class="flex-1 min-w-[200px]">
            <p class="text-sm text-muted-foreground mb-3">
              从 JSON 文件导入配置，支持跳过、覆盖或报错三种冲突处理模式
            </p>
            <div class="flex items-center gap-2">
              <input
                ref="configFileInput"
                type="file"
                accept=".json"
                class="hidden"
                @change="handleConfigFileSelect"
              >
              <Button
                variant="outline"
                :disabled="importLoading"
                @click="triggerConfigFileSelect"
              >
                <Upload class="w-4 h-4 mr-2" />
                {{ importLoading ? '导入中...' : '导入配置' }}
              </Button>
            </div>
          </div>
        </div>
      </CardSection>

      <!-- 用户数据导出/导入 -->
      <CardSection
        title="用户数据管理"
        description="导出或导入用户及其 API Keys 数据（不含管理员）"
      >
        <div class="flex flex-wrap gap-4">
          <div class="flex-1 min-w-[200px]">
            <p class="text-sm text-muted-foreground mb-3">
              导出所有普通用户及其 API Keys 到 JSON 文件
            </p>
            <Button
              variant="outline"
              :disabled="exportUsersLoading"
              @click="handleExportUsers"
            >
              <Download class="w-4 h-4 mr-2" />
              {{ exportUsersLoading ? '导出中...' : '导出用户数据' }}
            </Button>
          </div>
          <div class="flex-1 min-w-[200px]">
            <p class="text-sm text-muted-foreground mb-3">
              从 JSON 文件导入用户数据（需相同 ENCRYPTION_KEY）
            </p>
            <div class="flex items-center gap-2">
              <input
                ref="usersFileInput"
                type="file"
                accept=".json"
                class="hidden"
                @change="handleUsersFileSelect"
              >
              <Button
                variant="outline"
                :disabled="importUsersLoading"
                @click="triggerUsersFileSelect"
              >
                <Upload class="w-4 h-4 mr-2" />
                {{ importUsersLoading ? '导入中...' : '导入用户数据' }}
              </Button>
            </div>
          </div>
        </div>
      </CardSection>

      <!-- 基础配置 -->
      <CardSection
        title="基础配置"
        description="配置系统默认参数"
      >
        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
          <div>
            <Label
              for="default-quota"
              class="block text-sm font-medium"
            >
              默认用户配额(美元)
            </Label>
            <Input
              id="default-quota"
              v-model.number="systemConfig.default_user_quota_usd"
              type="number"
              step="0.01"
              placeholder="10.00"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              新用户注册时的默认配额
            </p>
          </div>

          <div>
            <Label
              for="rate-limit"
              class="block text-sm font-medium"
            >
              每分钟请求限制
            </Label>
            <Input
              id="rate-limit"
              v-model.number="systemConfig.rate_limit_per_minute"
              type="number"
              placeholder="0"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              0 表示不限制
            </p>
          </div>
        </div>
      </CardSection>

      <!-- 用户注册配置 -->
      <CardSection
        title="用户注册"
        description="控制用户注册和验证"
      >
        <div class="space-y-4">
          <div class="flex items-center space-x-2">
            <Checkbox
              id="enable-registration"
              v-model:checked="systemConfig.enable_registration"
            />
            <Label
              for="enable-registration"
              class="cursor-pointer"
            >
              开放用户注册
            </Label>
          </div>

          <div class="flex items-center space-x-2">
            <Checkbox
              id="require-email-verification"
              v-model:checked="systemConfig.require_email_verification"
            />
            <Label
              for="require-email-verification"
              class="cursor-pointer"
            >
              需要邮箱验证
            </Label>
          </div>
        </div>
      </CardSection>

      <!-- 独立余额 Key 过期管理 -->
      <CardSection
        title="独立余额 Key 过期管理"
        description="独立余额 Key 的过期处理策略（普通用户 Key 不会过期）"
      >
        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
          <div class="flex items-center h-full">
            <div class="flex items-center space-x-2">
              <Checkbox
                id="auto-delete-expired-keys"
                v-model:checked="systemConfig.auto_delete_expired_keys"
              />
              <div>
                <Label
                  for="auto-delete-expired-keys"
                  class="cursor-pointer"
                >
                  自动删除过期 Key
                </Label>
                <p class="text-xs text-muted-foreground">
                  关闭时仅禁用过期 Key，不会物理删除
                </p>
              </div>
            </div>
          </div>
        </div>
      </CardSection>

      <!-- 日志记录配置 -->
      <CardSection
        title="日志记录"
        description="控制请求日志的记录方式和内容"
      >
        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
          <div>
            <Label
              for="request-log-level"
              class="block text-sm font-medium mb-2"
            >
              记录详细程度
            </Label>
            <Select
              v-model="systemConfig.request_log_level"
              v-model:open="logLevelSelectOpen"
            >
              <SelectTrigger
                id="request-log-level"
                class="mt-1"
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="basic">
                  BASIC - 基本信息 (~1KB/条)
                </SelectItem>
                <SelectItem value="headers">
                  HEADERS - 含请求头 (~2-3KB/条)
                </SelectItem>
                <SelectItem value="full">
                  FULL - 完整请求响应 (~50KB/条)
                </SelectItem>
              </SelectContent>
            </Select>
            <p class="mt-1 text-xs text-muted-foreground">
              敏感信息会自动脱敏
            </p>
          </div>

          <div>
            <Label
              for="max-request-body-size"
              class="block text-sm font-medium"
            >
              最大请求体大小 (KB)
            </Label>
            <Input
              id="max-request-body-size"
              v-model.number="maxRequestBodySizeKB"
              type="number"
              placeholder="512"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              超过此大小的请求体将被截断记录
            </p>
          </div>

          <div>
            <Label
              for="max-response-body-size"
              class="block text-sm font-medium"
            >
              最大响应体大小 (KB)
            </Label>
            <Input
              id="max-response-body-size"
              v-model.number="maxResponseBodySizeKB"
              type="number"
              placeholder="512"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              超过此大小的响应体将被截断记录
            </p>
          </div>

          <div>
            <Label
              for="sensitive-headers"
              class="block text-sm font-medium"
            >
              敏感请求头
            </Label>
            <Input
              id="sensitive-headers"
              v-model="sensitiveHeadersStr"
              placeholder="authorization, x-api-key, cookie"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              逗号分隔，这些请求头会被脱敏处理
            </p>
          </div>
        </div>
      </CardSection>

      <!-- 日志清理策略 -->
      <CardSection
        title="日志清理策略"
        description="配置日志的分级保留和自动清理"
      >
        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
          <div class="md:col-span-2">
            <div class="flex items-center space-x-2 mb-4">
              <Checkbox
                id="enable-auto-cleanup"
                v-model:checked="systemConfig.enable_auto_cleanup"
              />
              <Label
                for="enable-auto-cleanup"
                class="cursor-pointer"
              >
                启用自动清理任务
              </Label>
              <span class="text-xs text-muted-foreground ml-2">
                (每天凌晨执行)
              </span>
            </div>
          </div>

          <div>
            <Label
              for="detail-log-retention-days"
              class="block text-sm font-medium"
            >
              详细日志保留天数
            </Label>
            <Input
              id="detail-log-retention-days"
              v-model.number="systemConfig.detail_log_retention_days"
              type="number"
              placeholder="7"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              超过后压缩 body 字段
            </p>
          </div>

          <div>
            <Label
              for="compressed-log-retention-days"
              class="block text-sm font-medium"
            >
              压缩日志保留天数
            </Label>
            <Input
              id="compressed-log-retention-days"
              v-model.number="systemConfig.compressed_log_retention_days"
              type="number"
              placeholder="90"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              超过后删除 body 字段
            </p>
          </div>

          <div>
            <Label
              for="header-retention-days"
              class="block text-sm font-medium"
            >
              请求头保留天数
            </Label>
            <Input
              id="header-retention-days"
              v-model.number="systemConfig.header_retention_days"
              type="number"
              placeholder="90"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              超过后清空 headers 字段
            </p>
          </div>

          <div>
            <Label
              for="log-retention-days"
              class="block text-sm font-medium"
            >
              完整日志保留天数
            </Label>
            <Input
              id="log-retention-days"
              v-model.number="systemConfig.log_retention_days"
              type="number"
              placeholder="365"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              超过后删除整条记录
            </p>
          </div>

          <div>
            <Label
              for="cleanup-batch-size"
              class="block text-sm font-medium"
            >
              每批次清理记录数
            </Label>
            <Input
              id="cleanup-batch-size"
              v-model.number="systemConfig.cleanup_batch_size"
              type="number"
              placeholder="1000"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              避免单次操作过大影响性能
            </p>
          </div>

          <div>
            <Label
              for="audit-log-retention-days"
              class="block text-sm font-medium"
            >
              审计日志保留天数
            </Label>
            <Input
              id="audit-log-retention-days"
              v-model.number="systemConfig.audit_log_retention_days"
              type="number"
              placeholder="30"
              class="mt-1"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              超过后删除审计日志记录
            </p>
          </div>
        </div>

        <!-- 清理策略说明 -->
        <div class="mt-4 p-4 bg-muted/50 rounded-lg">
          <h4 class="text-sm font-medium mb-2">
            清理策略说明
          </h4>
          <div class="text-xs text-muted-foreground space-y-1">
            <p>1. <strong>详细日志阶段</strong>: 保留完整的 request_body 和 response_body</p>
            <p>2. <strong>压缩日志阶段</strong>: body 字段被压缩存储，节省空间</p>
            <p>3. <strong>统计阶段</strong>: 仅保留 tokens、成本等统计信息</p>
            <p>4. <strong>归档删除</strong>: 超过保留期限后完全删除记录</p>
            <p>5. <strong>审计日志</strong>: 独立清理，记录用户登录、操作等安全事件</p>
          </div>
        </div>
      </CardSection>

      <!-- 流式输出配置 -->
      <CardSection
        title="流式输出"
        description="配置流式响应的输出效果"
      >
        <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
          <div class="md:col-span-2">
            <div class="flex items-center space-x-2">
              <Checkbox
                id="stream-smoothing-enabled"
                v-model:checked="systemConfig.stream_smoothing_enabled"
              />
              <div>
                <Label
                  for="stream-smoothing-enabled"
                  class="cursor-pointer"
                >
                  启用平滑输出
                </Label>
                <p class="text-xs text-muted-foreground">
                  将上游返回的大块内容拆分成小块，模拟打字效果
                </p>
              </div>
            </div>
          </div>

          <div>
            <Label
              for="stream-smoothing-chunk-size"
              class="block text-sm font-medium"
            >
              每块字符数
            </Label>
            <Input
              id="stream-smoothing-chunk-size"
              v-model.number="systemConfig.stream_smoothing_chunk_size"
              type="number"
              min="1"
              max="100"
              placeholder="20"
              class="mt-1"
              :disabled="!systemConfig.stream_smoothing_enabled"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              每次输出的字符数量（1-100）
            </p>
          </div>

          <div>
            <Label
              for="stream-smoothing-delay-ms"
              class="block text-sm font-medium"
            >
              输出间隔 (毫秒)
            </Label>
            <Input
              id="stream-smoothing-delay-ms"
              v-model.number="systemConfig.stream_smoothing_delay_ms"
              type="number"
              min="1"
              max="100"
              placeholder="8"
              class="mt-1"
              :disabled="!systemConfig.stream_smoothing_enabled"
            />
            <p class="mt-1 text-xs text-muted-foreground">
              每块之间的延迟毫秒数（1-100）
            </p>
          </div>
        </div>
      </CardSection>
    </div>

    <!-- 导入配置对话框 -->
    <Dialog
      v-model:open="importDialogOpen"
      title="导入配置"
      description="选择冲突处理模式并确认导入"
    >
      <div class="space-y-4">
        <div
          v-if="importPreview"
          class="p-3 bg-muted rounded-lg text-sm"
        >
          <p class="font-medium mb-2">
            配置预览
          </p>
          <ul class="space-y-1 text-muted-foreground">
            <li>全局模型: {{ importPreview.global_models?.length || 0 }} 个</li>
            <li>提供商: {{ importPreview.providers?.length || 0 }} 个</li>
            <li>
              端点: {{ importPreview.providers?.reduce((sum: number, p: any) => sum + (p.endpoints?.length || 0), 0) }} 个
            </li>
            <li>
              API Keys: {{ importPreview.providers?.reduce((sum: number, p: any) => sum + p.endpoints?.reduce((s: number, e: any) => s + (e.keys?.length || 0), 0), 0) }} 个
            </li>
          </ul>
        </div>

        <div>
          <Label class="block text-sm font-medium mb-2">冲突处理模式</Label>
          <Select
            v-model="mergeMode"
            v-model:open="mergeModeSelectOpen"
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
      </div>

      <template #footer>
        <Button
          variant="outline"
          @click="importDialogOpen = false; mergeModeSelectOpen = false"
        >
          取消
        </Button>
        <Button
          :disabled="importLoading"
          @click="confirmImport"
        >
          {{ importLoading ? '导入中...' : '确认导入' }}
        </Button>
      </template>
    </Dialog>

    <!-- 导入结果对话框 -->
    <Dialog
      v-model:open="importResultDialogOpen"
      title="导入完成"
    >
      <div
        v-if="importResult"
        class="space-y-4"
      >
        <div class="grid grid-cols-2 gap-4 text-sm">
          <div class="p-3 bg-muted rounded-lg">
            <p class="font-medium">
              全局模型
            </p>
            <p class="text-muted-foreground">
              创建: {{ importResult.stats.global_models.created }},
              更新: {{ importResult.stats.global_models.updated }},
              跳过: {{ importResult.stats.global_models.skipped }}
            </p>
          </div>
          <div class="p-3 bg-muted rounded-lg">
            <p class="font-medium">
              提供商
            </p>
            <p class="text-muted-foreground">
              创建: {{ importResult.stats.providers.created }},
              更新: {{ importResult.stats.providers.updated }},
              跳过: {{ importResult.stats.providers.skipped }}
            </p>
          </div>
          <div class="p-3 bg-muted rounded-lg">
            <p class="font-medium">
              端点
            </p>
            <p class="text-muted-foreground">
              创建: {{ importResult.stats.endpoints.created }},
              更新: {{ importResult.stats.endpoints.updated }},
              跳过: {{ importResult.stats.endpoints.skipped }}
            </p>
          </div>
          <div class="p-3 bg-muted rounded-lg">
            <p class="font-medium">
              API Keys
            </p>
            <p class="text-muted-foreground">
              创建: {{ importResult.stats.keys.created }},
              跳过: {{ importResult.stats.keys.skipped }}
            </p>
          </div>
          <div class="p-3 bg-muted rounded-lg col-span-2">
            <p class="font-medium">
              模型配置
            </p>
            <p class="text-muted-foreground">
              创建: {{ importResult.stats.models.created }},
              更新: {{ importResult.stats.models.updated }},
              跳过: {{ importResult.stats.models.skipped }}
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
        <Button @click="importResultDialogOpen = false">
          确定
        </Button>
      </template>
    </Dialog>

    <!-- 用户数据导入对话框 -->
    <Dialog
      v-model:open="importUsersDialogOpen"
      title="导入用户数据"
      description="选择冲突处理模式并确认导入"
    >
      <div class="space-y-4">
        <div
          v-if="importUsersPreview"
          class="p-3 bg-muted rounded-lg text-sm"
        >
          <p class="font-medium mb-2">
            数据预览
          </p>
          <ul class="space-y-1 text-muted-foreground">
            <li>用户: {{ importUsersPreview.users?.length || 0 }} 个</li>
            <li>
              API Keys: {{ importUsersPreview.users?.reduce((sum: number, u: any) => sum + (u.api_keys?.length || 0), 0) }} 个
            </li>
          </ul>
        </div>

        <div>
          <Label class="block text-sm font-medium mb-2">冲突处理模式</Label>
          <Select
            v-model="usersMergeMode"
            v-model:open="usersMergeModeSelectOpen"
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
      </div>

      <template #footer>
        <Button
          variant="outline"
          @click="importUsersDialogOpen = false; usersMergeModeSelectOpen = false"
        >
          取消
        </Button>
        <Button
          :disabled="importUsersLoading"
          @click="confirmImportUsers"
        >
          {{ importUsersLoading ? '导入中...' : '确认导入' }}
        </Button>
      </template>
    </Dialog>

    <!-- 用户数据导入结果对话框 -->
    <Dialog
      v-model:open="importUsersResultDialogOpen"
      title="用户数据导入完成"
    >
      <div
        v-if="importUsersResult"
        class="space-y-4"
      >
        <div class="grid grid-cols-2 gap-4 text-sm">
          <div class="p-3 bg-muted rounded-lg">
            <p class="font-medium">
              用户
            </p>
            <p class="text-muted-foreground">
              创建: {{ importUsersResult.stats.users.created }},
              更新: {{ importUsersResult.stats.users.updated }},
              跳过: {{ importUsersResult.stats.users.skipped }}
            </p>
          </div>
          <div class="p-3 bg-muted rounded-lg">
            <p class="font-medium">
              API Keys
            </p>
            <p class="text-muted-foreground">
              创建: {{ importUsersResult.stats.api_keys.created }},
              跳过: {{ importUsersResult.stats.api_keys.skipped }}
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
        <Button @click="importUsersResultDialogOpen = false">
          确定
        </Button>
      </template>
    </Dialog>
  </PageContainer>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { Download, Upload } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import {
  Dialog,
} from '@/components/ui'
import { PageHeader, PageContainer, CardSection } from '@/components/layout'
import { useToast } from '@/composables/useToast'
import { adminApi, type ConfigExportData, type ConfigImportResponse, type UsersExportData, type UsersImportResponse } from '@/api/admin'
import { log } from '@/utils/logger'

const { success, error } = useToast()

interface SystemConfig {
  // 基础配置
  default_user_quota_usd: number
  rate_limit_per_minute: number
  // 用户注册
  enable_registration: boolean
  require_email_verification: boolean
  // 独立余额 Key 过期管理
  auto_delete_expired_keys: boolean
  // 日志记录
  request_log_level: string
  max_request_body_size: number
  max_response_body_size: number
  sensitive_headers: string[]
  // 日志清理
  enable_auto_cleanup: boolean
  detail_log_retention_days: number
  compressed_log_retention_days: number
  header_retention_days: number
  log_retention_days: number
  cleanup_batch_size: number
  audit_log_retention_days: number
  // 流式输出
  stream_smoothing_enabled: boolean
  stream_smoothing_chunk_size: number
  stream_smoothing_delay_ms: number
}

const loading = ref(false)
const logLevelSelectOpen = ref(false)

// 导出/导入相关
const exportLoading = ref(false)
const importLoading = ref(false)
const importDialogOpen = ref(false)
const importResultDialogOpen = ref(false)
const configFileInput = ref<HTMLInputElement | null>(null)
const importPreview = ref<ConfigExportData | null>(null)
const importResult = ref<ConfigImportResponse | null>(null)
const mergeMode = ref<'skip' | 'overwrite' | 'error'>('skip')
const mergeModeSelectOpen = ref(false)

// 用户数据导出/导入相关
const exportUsersLoading = ref(false)
const importUsersLoading = ref(false)
const importUsersDialogOpen = ref(false)
const importUsersResultDialogOpen = ref(false)
const usersFileInput = ref<HTMLInputElement | null>(null)
const importUsersPreview = ref<UsersExportData | null>(null)
const importUsersResult = ref<UsersImportResponse | null>(null)
const usersMergeMode = ref<'skip' | 'overwrite' | 'error'>('skip')
const usersMergeModeSelectOpen = ref(false)

const systemConfig = ref<SystemConfig>({
  // 基础配置
  default_user_quota_usd: 10.0,
  rate_limit_per_minute: 0,
  // 用户注册
  enable_registration: false,
  require_email_verification: false,
  // 独立余额 Key 过期管理
  auto_delete_expired_keys: false,
  // 日志记录
  request_log_level: 'basic',
  max_request_body_size: 1048576,
  max_response_body_size: 1048576,
  sensitive_headers: ['authorization', 'x-api-key', 'api-key', 'cookie', 'set-cookie'],
  // 日志清理
  enable_auto_cleanup: true,
  detail_log_retention_days: 7,
  compressed_log_retention_days: 90,
  header_retention_days: 90,
  log_retention_days: 365,
  cleanup_batch_size: 1000,
  audit_log_retention_days: 30,
  // 流式输出
  stream_smoothing_enabled: false,
  stream_smoothing_chunk_size: 20,
  stream_smoothing_delay_ms: 8,
})

// 计算属性：KB 和 字节 之间的转换
const maxRequestBodySizeKB = computed({
  get: () => Math.round(systemConfig.value.max_request_body_size / 1024),
  set: (val: number) => {
    systemConfig.value.max_request_body_size = val * 1024
  }
})

const maxResponseBodySizeKB = computed({
  get: () => Math.round(systemConfig.value.max_response_body_size / 1024),
  set: (val: number) => {
    systemConfig.value.max_response_body_size = val * 1024
  }
})

// 计算属性：敏感请求头数组和字符串之间的转换
const sensitiveHeadersStr = computed({
  get: () => systemConfig.value.sensitive_headers.join(', '),
  set: (val: string) => {
    systemConfig.value.sensitive_headers = val
      .split(',')
      .map(s => s.trim().toLowerCase())
      .filter(s => s.length > 0)
  }
})

onMounted(async () => {
  await loadSystemConfig()
})

async function loadSystemConfig() {
  try {
    const configs = [
      // 基础配置
      'default_user_quota_usd',
      'rate_limit_per_minute',
      // 用户注册
      'enable_registration',
      'require_email_verification',
      // 独立余额 Key 过期管理
      'auto_delete_expired_keys',
      // 日志记录
      'request_log_level',
      'max_request_body_size',
      'max_response_body_size',
      'sensitive_headers',
      // 日志清理
      'enable_auto_cleanup',
      'detail_log_retention_days',
      'compressed_log_retention_days',
      'header_retention_days',
      'log_retention_days',
      'cleanup_batch_size',
      'audit_log_retention_days',
      // 流式输出
      'stream_smoothing_enabled',
      'stream_smoothing_chunk_size',
      'stream_smoothing_delay_ms',
    ]

    for (const key of configs) {
      try {
        const response = await adminApi.getSystemConfig(key)
        if (response.value !== null && response.value !== undefined) {
          (systemConfig.value as any)[key] = response.value
        }
      } catch {
        // 配置不存在时使用默认值，无需处理
      }
    }
  } catch (err) {
    error('加载系统配置失败')
    log.error('加载系统配置失败:', err)
  }
}

async function saveSystemConfig() {
  loading.value = true
  try {
    const configItems = [
      // 基础配置
      {
        key: 'default_user_quota_usd',
        value: systemConfig.value.default_user_quota_usd,
        description: '默认用户配额（美元）'
      },
      {
        key: 'rate_limit_per_minute',
        value: systemConfig.value.rate_limit_per_minute,
        description: '每分钟请求限制'
      },
      // 用户注册
      {
        key: 'enable_registration',
        value: systemConfig.value.enable_registration,
        description: '是否开放用户注册'
      },
      {
        key: 'require_email_verification',
        value: systemConfig.value.require_email_verification,
        description: '是否需要邮箱验证'
      },
      // 独立余额 Key 过期管理
      {
        key: 'auto_delete_expired_keys',
        value: systemConfig.value.auto_delete_expired_keys,
        description: '是否自动删除过期的API Key'
      },
      // 日志记录
      {
        key: 'request_log_level',
        value: systemConfig.value.request_log_level,
        description: '请求记录级别'
      },
      {
        key: 'max_request_body_size',
        value: systemConfig.value.max_request_body_size,
        description: '最大请求体记录大小（字节）'
      },
      {
        key: 'max_response_body_size',
        value: systemConfig.value.max_response_body_size,
        description: '最大响应体记录大小（字节）'
      },
      {
        key: 'sensitive_headers',
        value: systemConfig.value.sensitive_headers,
        description: '敏感请求头列表'
      },
      // 日志清理
      {
        key: 'enable_auto_cleanup',
        value: systemConfig.value.enable_auto_cleanup,
        description: '是否启用自动清理任务'
      },
      {
        key: 'detail_log_retention_days',
        value: systemConfig.value.detail_log_retention_days,
        description: '详细日志保留天数'
      },
      {
        key: 'compressed_log_retention_days',
        value: systemConfig.value.compressed_log_retention_days,
        description: '压缩日志保留天数'
      },
      {
        key: 'header_retention_days',
        value: systemConfig.value.header_retention_days,
        description: '请求头保留天数'
      },
      {
        key: 'log_retention_days',
        value: systemConfig.value.log_retention_days,
        description: '完整日志保留天数'
      },
      {
        key: 'cleanup_batch_size',
        value: systemConfig.value.cleanup_batch_size,
        description: '每批次清理的记录数'
      },
      {
        key: 'audit_log_retention_days',
        value: systemConfig.value.audit_log_retention_days,
        description: '审计日志保留天数'
      },
      // 流式输出
      {
        key: 'stream_smoothing_enabled',
        value: systemConfig.value.stream_smoothing_enabled,
        description: '是否启用流式平滑输出'
      },
      {
        key: 'stream_smoothing_chunk_size',
        value: systemConfig.value.stream_smoothing_chunk_size,
        description: '流式平滑输出每个小块的字符数'
      },
      {
        key: 'stream_smoothing_delay_ms',
        value: systemConfig.value.stream_smoothing_delay_ms,
        description: '流式平滑输出每个小块之间的延迟毫秒数'
      },
    ]

    const promises = configItems.map(item =>
      adminApi.updateSystemConfig(item.key, item.value, item.description)
    )

    await Promise.all(promises)
    success('系统配置已保存')
  } catch (err) {
    error('保存配置失败')
    log.error('保存配置失败:', err)
  } finally {
    loading.value = false
  }
}

// 导出配置
async function handleExportConfig() {
  exportLoading.value = true
  try {
    const data = await adminApi.exportConfig()
    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `aether-config-${new Date().toISOString().slice(0, 10)}.json`
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
    URL.revokeObjectURL(url)
    success('配置已导出')
  } catch (err) {
    error('导出配置失败')
    log.error('导出配置失败:', err)
  } finally {
    exportLoading.value = false
  }
}

// 触发文件选择
function triggerConfigFileSelect() {
  configFileInput.value?.click()
}

// 文件大小限制 (10MB)
const MAX_FILE_SIZE = 10 * 1024 * 1024

// 处理文件选择
function handleConfigFileSelect(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return

  if (file.size > MAX_FILE_SIZE) {
    error('文件大小不能超过 10MB')
    input.value = ''
    return
  }

  const reader = new FileReader()
  reader.onload = (e) => {
    try {
      const content = e.target?.result as string
      const data = JSON.parse(content) as ConfigExportData

      // 验证版本
      if (data.version !== '1.0') {
        error(`不支持的配置版本: ${data.version}`)
        return
      }

      importPreview.value = data
      mergeMode.value = 'skip'
      importDialogOpen.value = true
    } catch (err) {
      error('解析配置文件失败，请确保是有效的 JSON 文件')
      log.error('解析配置文件失败:', err)
    }
  }
  reader.readAsText(file)

  // 重置 input 以便能再次选择同一文件
  input.value = ''
}

// 确认导入
async function confirmImport() {
  if (!importPreview.value) return

  importLoading.value = true
  try {
    const result = await adminApi.importConfig({
      ...importPreview.value,
      merge_mode: mergeMode.value
    })
    importResult.value = result
    importDialogOpen.value = false
    mergeModeSelectOpen.value = false
    importResultDialogOpen.value = true
    success('配置导入成功')
  } catch (err: any) {
    error(err.response?.data?.detail || '导入配置失败')
    log.error('导入配置失败:', err)
  } finally {
    importLoading.value = false
  }
}

// 导出用户数据
async function handleExportUsers() {
  exportUsersLoading.value = true
  try {
    const data = await adminApi.exportUsers()
    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `aether-users-${new Date().toISOString().slice(0, 10)}.json`
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
    URL.revokeObjectURL(url)
    success('用户数据已导出')
  } catch (err) {
    error('导出用户数据失败')
    log.error('导出用户数据失败:', err)
  } finally {
    exportUsersLoading.value = false
  }
}

// 触发用户数据文件选择
function triggerUsersFileSelect() {
  usersFileInput.value?.click()
}

// 处理用户数据文件选择
function handleUsersFileSelect(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return

  if (file.size > MAX_FILE_SIZE) {
    error('文件大小不能超过 10MB')
    input.value = ''
    return
  }

  const reader = new FileReader()
  reader.onload = (e) => {
    try {
      const content = e.target?.result as string
      const data = JSON.parse(content) as UsersExportData

      // 验证版本
      if (data.version !== '1.0') {
        error(`不支持的配置版本: ${data.version}`)
        return
      }

      importUsersPreview.value = data
      usersMergeMode.value = 'skip'
      importUsersDialogOpen.value = true
    } catch (err) {
      error('解析用户数据文件失败，请确保是有效的 JSON 文件')
      log.error('解析用户数据文件失败:', err)
    }
  }
  reader.readAsText(file)

  // 重置 input 以便能再次选择同一文件
  input.value = ''
}

// 确认导入用户数据
async function confirmImportUsers() {
  if (!importUsersPreview.value) return

  importUsersLoading.value = true
  try {
    const result = await adminApi.importUsers({
      ...importUsersPreview.value,
      merge_mode: usersMergeMode.value
    })
    importUsersResult.value = result
    importUsersDialogOpen.value = false
    usersMergeModeSelectOpen.value = false
    importUsersResultDialogOpen.value = true
    success('用户数据导入成功')
  } catch (err: any) {
    error(err.response?.data?.detail || '导入用户数据失败')
    log.error('导入用户数据失败:', err)
  } finally {
    importUsersLoading.value = false
  }
}
</script>
