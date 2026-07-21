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
          默认速率限制 (请求/分钟)
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
          0 表示默认不限制；未单独配置的用户和独立 Key 会跟随这里
        </p>
      </div>

      <div>
        <Label
          for="password-policy-level"
          class="block text-sm font-medium mb-2"
        >
          密码策略
        </Label>
        <Select
          :model-value="passwordPolicyLevel"
          @update:model-value="$emit('update:passwordPolicyLevel', $event)"
        >
          <SelectTrigger
            id="password-policy-level"
            class="mt-1"
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="weak">
              弱密码 - 至少 6 个字符
            </SelectItem>
            <SelectItem value="medium">
              中等密码 - 至少 8 位，含字母和数字
            </SelectItem>
            <SelectItem value="strong">
              强密码 - 至少 8 位，含大小写字母、数字和特殊字符
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="mt-1 text-xs text-muted-foreground">
          影响注册、创建用户、重置/修改密码的校验规则
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

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="enable-openai-image-sync-heartbeat"
            :checked="enableOpenaiImageSyncHeartbeat"
            @update:checked="$emit('update:enableOpenaiImageSyncHeartbeat', $event)"
          />
          <div>
            <Label
              for="enable-openai-image-sync-heartbeat"
              class="cursor-pointer"
            >
              同步生图心跳
            </Label>
            <p class="text-xs text-muted-foreground">
              开启后同步生图外层 HTTP 状态固定为 200，上游失败需读取响应体 error.upstream_status
            </p>
          </div>
        </div>
      </div>

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="enable-standard-text-sync-heartbeat"
            :checked="enableStandardTextSyncHeartbeat"
            @update:checked="$emit('update:enableStandardTextSyncHeartbeat', $event)"
          />
          <div>
            <Label
              for="enable-standard-text-sync-heartbeat"
              class="cursor-pointer"
            >
              标准文本非流式心跳
            </Label>
            <p class="text-xs text-muted-foreground">
              开启后标准文本非流式接口外层 HTTP 状态固定为 200，上游失败需读取响应体 error.upstream_status
            </p>
          </div>
        </div>
      </div>

      <div class="flex items-center h-full">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="cyber-continue-failover"
            :checked="cyberContinueFailover"
            @update:checked="$emit('update:cyberContinueFailover', $event)"
          />
          <div>
            <Label
              for="cyber-continue-failover"
              class="cursor-pointer"
            >
              Cyber继续转移
            </Label>
            <p class="text-xs text-muted-foreground">
              关闭时Cyber Policy错误直接返回客户端；开启后在响应内容开始前按普通错误继续故障转移，可能增加首字等待时间
            </p>
          </div>
        </div>
      </div>

      <div class="md:col-span-2 grid grid-cols-1 md:grid-cols-2 gap-4 border-t pt-5">
        <div class="flex items-center h-full">
          <div class="flex items-center space-x-2">
            <Checkbox
              id="turnstile-enabled"
              :checked="turnstileEnabled"
              @update:checked="$emit('update:turnstileEnabled', $event)"
            />
            <div>
              <Label
                for="turnstile-enabled"
                class="cursor-pointer"
              >
                注册人机验证
              </Label>
              <p class="text-xs text-muted-foreground">
                开启后注册与发送邮箱验证码前需要通过 Cloudflare Turnstile
              </p>
            </div>
          </div>
        </div>

        <div>
          <Label
            for="turnstile-site-key"
            class="block text-sm font-medium"
          >
            Turnstile Site Key
          </Label>
          <Input
            id="turnstile-site-key"
            :model-value="turnstileSiteKey || ''"
            type="text"
            placeholder="0x4AAAA..."
            class="mt-1"
            @update:model-value="$emit('update:turnstileSiteKey', String($event || '').trim() || null)"
          />
        </div>

        <div>
          <div class="flex items-center justify-between">
            <Label
              for="turnstile-secret-key"
              class="block text-sm font-medium"
            >
              Turnstile Secret Key
            </Label>
            <Button
              v-if="turnstileSecretConfigured"
              type="button"
              variant="link"
              size="sm"
              class="h-auto p-0 text-xs"
              :disabled="loading"
              @click="$emit('clearTurnstileSecret')"
            >
              清空
            </Button>
          </div>
          <Input
            id="turnstile-secret-key"
            :model-value="turnstileSecretKey"
            type="password"
            :placeholder="turnstileSecretConfigured ? '已配置，留空不修改' : '输入 Secret Key'"
            class="mt-1"
            autocomplete="new-password"
            @update:model-value="$emit('update:turnstileSecretKey', String($event || ''))"
          />
        </div>

        <div>
          <Label
            for="turnstile-hostnames"
            class="block text-sm font-medium"
          >
            允许的 Hostname
          </Label>
          <Input
            id="turnstile-hostnames"
            :model-value="turnstileAllowedHostnamesStr"
            type="text"
            placeholder="example.com, app.example.com"
            class="mt-1"
            @update:model-value="$emit('update:turnstileAllowedHostnamesStr', String($event || ''))"
          />
          <p class="mt-1 text-xs text-muted-foreground">
            留空则不额外校验 Cloudflare 返回的 hostname
          </p>
        </div>
      </div>

      <div class="md:col-span-2 grid grid-cols-1 md:grid-cols-2 gap-4 border-t pt-5">
        <div class="flex items-center h-full">
          <div class="flex items-center space-x-2">
            <Checkbox
              id="referral-enabled"
              :checked="referralEnabled"
              @update:checked="$emit('update:referralEnabled', $event)"
            />
            <div>
              <Label
                for="referral-enabled"
                class="cursor-pointer"
              >
                邀请返利
              </Label>
              <p class="text-xs text-muted-foreground">
                开启后可按充值比例、人头或两者同时发放赠款返利
              </p>
            </div>
          </div>
        </div>

        <div>
          <Label
            for="referral-reward-mode"
            class="block text-sm font-medium mb-2"
          >
            返利方式
          </Label>
          <Select
            :model-value="referralRewardMode"
            @update:model-value="$emit('update:referralRewardMode', $event)"
          >
            <SelectTrigger id="referral-reward-mode">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="percent">
                按充值比例
              </SelectItem>
              <SelectItem value="headcount">
                按邀请人头
              </SelectItem>
              <SelectItem value="both">
                两者同时启用
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div>
          <Label
            for="referral-recharge-percent"
            class="block text-sm font-medium"
          >
            充值返利比例 (%)
          </Label>
          <Input
            id="referral-recharge-percent"
            :model-value="referralRechargePercent"
            type="number"
            min="0"
            step="0.01"
            class="mt-1"
            @update:model-value="$emit('update:referralRechargePercent', Number($event))"
          />
        </div>

        <div>
          <Label
            for="referral-headcount-amount"
            class="block text-sm font-medium"
          >
            人头返利金额 (美元)
          </Label>
          <Input
            id="referral-headcount-amount"
            :model-value="referralHeadcountAmountUsd"
            type="number"
            min="0"
            step="0.01"
            class="mt-1"
            @update:model-value="$emit('update:referralHeadcountAmountUsd', Number($event))"
          />
        </div>

        <div>
          <Label
            for="referral-headcount-trigger"
            class="block text-sm font-medium mb-2"
          >
            人头返利触发时机
          </Label>
          <Select
            :model-value="referralHeadcountTrigger"
            @update:model-value="$emit('update:referralHeadcountTrigger', $event)"
          >
            <SelectTrigger id="referral-headcount-trigger">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="registration">
                注册成功
              </SelectItem>
              <SelectItem value="email_verified">
                邮箱验证完成
              </SelectItem>
              <SelectItem value="first_paid_order">
                首笔真实支付完成
              </SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>

      <div class="md:col-span-2 grid grid-cols-1 md:grid-cols-2 gap-4 border-t pt-5">
        <div class="flex items-center h-full">
          <div class="flex items-center space-x-2">
            <Checkbox
              id="privacy-policy-enabled"
              :checked="registrationPrivacyPolicyEnabled"
              @update:checked="$emit('update:registrationPrivacyPolicyEnabled', $event)"
            />
            <div>
              <Label
                for="privacy-policy-enabled"
                class="cursor-pointer"
              >
                注册隐私政策确认
              </Label>
              <p class="text-xs text-muted-foreground">
                开启后注册时必须确认当前版本
              </p>
            </div>
          </div>
        </div>

        <div>
          <Label
            for="privacy-policy-version"
            class="block text-sm font-medium"
          >
            隐私政策版本
          </Label>
          <Input
            id="privacy-policy-version"
            :model-value="registrationPrivacyPolicyVersion"
            type="text"
            placeholder="2026-05-16"
            class="mt-1"
            @update:model-value="$emit('update:registrationPrivacyPolicyVersion', String($event || '').trim())"
          />
        </div>

        <div>
          <Label
            for="privacy-policy-format"
            class="block text-sm font-medium mb-2"
          >
            隐私政策格式
          </Label>
          <Select
            :model-value="registrationPrivacyPolicyFormat"
            @update:model-value="$emit('update:registrationPrivacyPolicyFormat', $event)"
          >
            <SelectTrigger id="privacy-policy-format">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="markdown">
                Markdown
              </SelectItem>
              <SelectItem value="html">
                HTML
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        <div class="md:col-span-2">
          <Label
            for="privacy-policy-content"
            class="block text-sm font-medium"
          >
            隐私政策内容
          </Label>
          <Textarea
            id="privacy-policy-content"
            :model-value="registrationPrivacyPolicyContent"
            rows="8"
            class="mt-1"
            placeholder="填写 Markdown 或 HTML 内容"
            @update:model-value="$emit('update:registrationPrivacyPolicyContent', $event)"
          />
        </div>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Textarea from '@/components/ui/textarea.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { CardSection } from '@/components/layout'

defineProps<{
  defaultUserInitialGiftUsd: number
  rateLimitPerMinute: number
  enableRegistration: boolean
  passwordPolicyLevel: string
  turnstileEnabled: boolean
  turnstileSiteKey: string | null
  turnstileSecretKey: string
  turnstileSecretConfigured: boolean
  turnstileAllowedHostnamesStr: string
  referralEnabled: boolean
  referralRewardMode: string
  referralRechargePercent: number
  referralHeadcountAmountUsd: number
  referralHeadcountTrigger: string
  registrationPrivacyPolicyEnabled: boolean
  registrationPrivacyPolicyFormat: string
  registrationPrivacyPolicyContent: string
  registrationPrivacyPolicyVersion: string
  autoDeleteExpiredKeys: boolean
  enableFormatConversion: boolean
  enableOpenaiImageSyncHeartbeat: boolean
  enableStandardTextSyncHeartbeat: boolean
  cyberContinueFailover: boolean
  loading: boolean
  hasChanges: boolean
}>()

defineEmits<{
  save: []
  'update:defaultUserInitialGiftUsd': [value: number]
  'update:rateLimitPerMinute': [value: number]
  'update:enableRegistration': [value: boolean]
  'update:passwordPolicyLevel': [value: string]
  'update:turnstileEnabled': [value: boolean]
  'update:turnstileSiteKey': [value: string | null]
  'update:turnstileSecretKey': [value: string]
  'update:turnstileAllowedHostnamesStr': [value: string]
  clearTurnstileSecret: []
  'update:referralEnabled': [value: boolean]
  'update:referralRewardMode': [value: string]
  'update:referralRechargePercent': [value: number]
  'update:referralHeadcountAmountUsd': [value: number]
  'update:referralHeadcountTrigger': [value: string]
  'update:registrationPrivacyPolicyEnabled': [value: boolean]
  'update:registrationPrivacyPolicyFormat': [value: string]
  'update:registrationPrivacyPolicyContent': [value: string]
  'update:registrationPrivacyPolicyVersion': [value: string]
  'update:autoDeleteExpiredKeys': [value: boolean]
  'update:enableFormatConversion': [value: boolean]
  'update:enableOpenaiImageSyncHeartbeat': [value: boolean]
  'update:enableStandardTextSyncHeartbeat': [value: boolean]
  'update:cyberContinueFailover': [value: boolean]
}>()
</script>
