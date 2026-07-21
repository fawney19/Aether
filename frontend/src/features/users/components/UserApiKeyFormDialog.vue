<template>
  <Dialog
    :model-value="open"
    size="lg"
    @update:model-value="(value) => !value && $emit('close')"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-kraft/10">
            <Key class="h-5 w-5 text-kraft" />
          </div>
          <div class="min-w-0 flex-1">
            <h3 class="text-lg font-semibold leading-tight text-foreground">
              {{ isEditing ? legacyT('编辑 API Key') : legacyT('创建 API Key') }}
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ isEditing ? legacyT('更新用户 API Key 的名称、速率限制和并发限制') : legacyT('为用户创建新的 API Key') }}
            </p>
          </div>
        </div>
      </div>
    </template>

    <div class="space-y-4">
      <div class="space-y-2">
        <Label
          for="admin-user-key-name"
          class="text-sm font-medium"
        >
          {{ legacyT('密钥名称') }}
        </Label>
        <Input
          id="admin-user-key-name"
          :model-value="form.name"
          class="h-10"
          :placeholder="legacyT('例如：生产环境 Key')"
          @update:model-value="updateField('name', String($event))"
        />
      </div>

      <div class="space-y-2">
        <Label
          for="admin-user-key-rate-limit"
          class="text-sm font-medium"
        >
          {{ legacyT('速率限制 (请求/分钟)') }}
        </Label>
        <Input
          id="admin-user-key-rate-limit"
          :model-value="form.rate_limit ?? ''"
          type="number"
          min="0"
          max="10000"
          class="h-10"
          :placeholder="legacyT('留空不限')"
          @update:model-value="updateField('rate_limit', parseNumberInput($event, { min: 0, max: 10000 }))"
        />
        <p class="text-xs text-muted-foreground">
          {{ legacyT('留空表示不限制') }}
        </p>
      </div>

      <div class="space-y-2">
        <Label
          for="admin-user-key-concurrent-limit"
          class="text-sm font-medium"
        >
          {{ legacyT('并发限制') }}
        </Label>
        <Input
          id="admin-user-key-concurrent-limit"
          :model-value="form.concurrent_limit ?? ''"
          type="number"
          min="0"
          max="10000"
          class="h-10"
          placeholder="0 = unlimited"
          @update:model-value="updateField('concurrent_limit', parseNumberInput($event, { min: 0, max: 10000 }))"
        />
        <p class="text-xs text-muted-foreground">
          {{ legacyT(isEditing ? '留空表示保持当前值，填 0 表示不限并发' : '留空表示不限并发，填 0 也表示不限并发') }}
        </p>
      </div>

      <div class="space-y-2">
        <Label
          for="admin-user-key-ip-rules"
          class="text-sm font-medium"
        >
          {{ legacyT('IP 限制') }}
        </Label>
        <Input
          id="admin-user-key-ip-rules"
          :model-value="form.ip_rules_text"
          class="h-10"
          placeholder="203.0.113.10, 10.0.0.0/24, !10.0.0.13"
          @update:model-value="updateField('ip_rules_text', String($event))"
        />
        <p class="text-xs text-muted-foreground">
          {{ legacyT('留空表示不限制；支持 IP、CIDR、IPv4 通配符、*，用 ! 前缀拒绝，多个规则用英文逗号分隔') }}
        </p>
      </div>

      <div class="space-y-3 rounded-lg border border-border bg-muted/30 p-3">
        <div class="flex items-center justify-between gap-3">
          <div>
            <Label class="text-sm font-medium">
              {{ legacyT('敏感信息保护') }}
            </Label>
            <p class="mt-1 text-xs text-muted-foreground">
              {{ legacyT(form.chat_pii_redaction_mode === 'inherit' ? '跟随目标用户设置' : '仅覆盖此 API Key') }}
            </p>
          </div>
          <div class="flex items-center gap-2">
            <Button
              size="sm"
              :variant="form.chat_pii_redaction_mode === 'inherit' ? 'default' : 'outline'"
              @click="updateField('chat_pii_redaction_mode', 'inherit')"
            >
              {{ legacyT('跟随用户') }}
            </Button>
            <Button
              size="sm"
              :variant="form.chat_pii_redaction_mode === 'custom' ? 'default' : 'outline'"
              @click="updateField('chat_pii_redaction_mode', 'custom')"
            >
              {{ legacyT('单独配置') }}
            </Button>
          </div>
        </div>
        <div
          v-if="form.chat_pii_redaction_mode === 'custom'"
          class="flex items-center justify-between gap-3 border-t border-border/50 pt-3"
        >
          <Label class="text-sm font-medium">
            {{ legacyT('启用保护') }}
          </Label>
          <Switch
            :model-value="form.chat_pii_redaction_enabled"
            @update:model-value="updateField('chat_pii_redaction_enabled', $event)"
          />
        </div>
        <div
          v-if="form.chat_pii_redaction_mode === 'custom' && form.chat_pii_redaction_enabled"
          class="flex items-center justify-between gap-3 border-t border-border/50 pt-3"
        >
          <Label class="text-sm font-medium">
            {{ legacyT('占位符说明') }}
          </Label>
          <Switch
            :model-value="form.chat_pii_redaction_placeholder_notice"
            :disabled="!form.chat_pii_redaction_enabled"
            @update:model-value="updateField('chat_pii_redaction_placeholder_notice', $event)"
          />
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        class="h-10 px-5"
        @click="$emit('close')"
      >
        {{ legacyT('取消') }}
      </Button>
      <Button
        class="h-10 px-5"
        :disabled="creating"
        @click="$emit('submit')"
      >
        {{ submitLabel }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Key } from 'lucide-vue-next'
import { Button, Dialog, Input, Label, Switch } from '@/components/ui'
import { useI18n } from '@/i18n'
import { parseNumberInput } from '@/utils/form'

export interface UserApiKeyFormState {
  name: string
  rate_limit?: number
  concurrent_limit?: number
  ip_rules_text: string
  chat_pii_redaction_mode: 'inherit' | 'custom'
  chat_pii_redaction_enabled: boolean
  chat_pii_redaction_placeholder_notice: boolean
}

const props = defineProps<{
  open: boolean
  form: UserApiKeyFormState
  isEditing: boolean
  creating: boolean
}>()

const emit = defineEmits<{
  close: []
  submit: []
  'update:form': [value: UserApiKeyFormState]
}>()

const { legacyT } = useI18n()

const submitLabel = computed(() => {
  if (props.creating) {
    return legacyT(props.isEditing ? '保存中...' : '创建中...')
  }
  return legacyT(props.isEditing ? '保存' : '创建')
})

function updateField<TKey extends keyof UserApiKeyFormState>(key: TKey, value: UserApiKeyFormState[TKey]): void {
  emit('update:form', {
    ...props.form,
    [key]: value,
  })
}
</script>
