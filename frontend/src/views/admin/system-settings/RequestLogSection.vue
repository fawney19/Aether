<template>
  <CardSection
    title="请求记录"
    description="控制请求/响应详情的入库方式和内容"
  >
    <template #actions>
      <Button
        size="sm"
        :disabled="loading || !hasChanges"
        @click="$emit('save')"
      >
        {{ loading ? '保存中...' : '保存' }}
      </Button>
    </template>
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <div>
        <Label
          for="request-log-level"
          class="block text-sm font-medium mb-2"
        >
          记录详细程度
        </Label>
        <Select
          :model-value="requestRecordLevel"
          @update:model-value="$emit('update:requestRecordLevel', $event)"
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
              FULL - 完整请求响应
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="mt-1 text-xs text-muted-foreground">
          敏感信息会自动脱敏
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
          :model-value="sensitiveHeadersStr"
          placeholder="authorization, x-api-key, cookie"
          class="mt-1"
          @update:model-value="$emit('update:sensitiveHeadersStr', $event)"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          逗号分隔，这些请求头会被脱敏处理
        </p>
      </div>

      <div class="flex items-start gap-3 pt-1">
        <Switch
          id="compact-base64-images"
          :model-value="compactBase64Images"
          :disabled="loading"
          @update:model-value="$emit('update:compactBase64Images', $event)"
        />
        <div>
          <Label
            for="compact-base64-images"
            class="text-sm font-medium cursor-pointer"
          >
            精简 Base64 图片
          </Label>
          <p class="mt-1 text-xs text-muted-foreground">
            将 data:image/...;base64 图片替换为包含原始大小的占位符
          </p>
        </div>
      </div>

      <div>
        <Label
          for="request-record-max-body-size-kb"
          class="block text-sm font-medium"
        >
          Body 大小上限（KB）
        </Label>
        <div class="relative mt-1">
          <Input
            id="request-record-max-body-size-kb"
            :model-value="maxBodySizeKb"
            type="number"
            min="0"
            max="1048576"
            step="1"
            class="pr-12"
            @update:model-value="$emit('update:maxBodySizeKb', Math.min(1048576, Math.max(0, Math.trunc(Number($event) || 0))))"
          />
          <span class="absolute inset-y-0 right-3 flex items-center text-xs text-muted-foreground pointer-events-none">
            KB
          </span>
        </div>
        <p class="mt-1 text-xs text-muted-foreground">
          每个请求/响应 Body 独立限制，0 表示不限制
        </p>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Switch from '@/components/ui/switch.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { CardSection } from '@/components/layout'

defineProps<{
  requestRecordLevel: string
  sensitiveHeadersStr: string
  compactBase64Images: boolean
  maxBodySizeKb: number
  loading: boolean
  hasChanges: boolean
}>()

defineEmits<{
  save: []
  'update:requestRecordLevel': [value: string]
  'update:sensitiveHeadersStr': [value: string]
  'update:compactBase64Images': [value: boolean]
  'update:maxBodySizeKb': [value: number]
}>()
</script>
