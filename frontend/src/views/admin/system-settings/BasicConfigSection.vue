<template>
  <CardSection
    title="基础配置"
    description="配置系统默认参数"
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
          for="default-quota"
          class="block text-sm font-medium"
        >
          默认用户初始赠款(美元)
        </Label>
        <Input
          id="default-quota"
          :model-value="defaultUserInitialGiftUsd"
          type="number"
          step="0.01"
          placeholder="10.00"
          class="mt-1"
          @update:model-value="$emit('update:defaultUserInitialGiftUsd', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          新用户注册时的默认初始赠款
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
          :model-value="rateLimitPerMinute"
          type="number"
          placeholder="0"
          class="mt-1"
          @update:model-value="$emit('update:rateLimitPerMinute', Number($event))"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          0 表示不限制
        </p>
      </div>

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="enable-registration"
            :checked="enableRegistration"
            @update:checked="$emit('update:enableRegistration', $event)"
          />
          <div>
            <Label
              for="enable-registration"
              class="cursor-pointer"
            >
              开放用户注册
            </Label>
            <p class="text-xs text-muted-foreground">
              允许新用户自助注册账户
            </p>
          </div>
        </div>
      </div>

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="auto-delete-expired-keys"
            :checked="autoDeleteExpiredKeys"
            @update:checked="$emit('update:autoDeleteExpiredKeys', $event)"
          />
          <div>
            <Label
              for="auto-delete-expired-keys"
              class="cursor-pointer"
            >
              自动删除过期 Key
            </Label>
            <p class="text-xs text-muted-foreground">
              关闭时仅禁用过期的独立余额 Key
            </p>
          </div>
        </div>
      </div>

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="enable-format-conversion"
            :checked="enableFormatConversion"
            @update:checked="$emit('update:enableFormatConversion', $event)"
          />
          <div>
            <Label
              for="enable-format-conversion"
              class="cursor-pointer"
            >
              全局格式转换
            </Label>
            <p class="text-xs text-muted-foreground">
              开启后强制允许所有提供商接受跨格式请求
            </p>
          </div>
        </div>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import { CardSection } from '@/components/layout'

defineProps<{
  defaultUserInitialGiftUsd: number
  rateLimitPerMinute: number
  enableRegistration: boolean
  autoDeleteExpiredKeys: boolean
  enableFormatConversion: boolean
  loading: boolean
  hasChanges: boolean
}>()

defineEmits<{
  save: []
  'update:defaultUserInitialGiftUsd': [value: number]
  'update:rateLimitPerMinute': [value: number]
  'update:enableRegistration': [value: boolean]
  'update:autoDeleteExpiredKeys': [value: boolean]
  'update:enableFormatConversion': [value: boolean]
}>()
</script>
