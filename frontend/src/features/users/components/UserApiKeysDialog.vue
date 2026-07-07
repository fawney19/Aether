<template>
  <Dialog
    :model-value="open"
    size="xl"
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
              {{ legacyT('管理 API Keys') }}
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ legacyT('查看和管理用户的 API 密钥') }}
            </p>
          </div>
        </div>
      </div>
    </template>

    <div class="max-h-[60vh] space-y-3 overflow-y-auto">
      <template v-if="apiKeys.length > 0">
        <div
          v-for="apiKey in apiKeys"
          :key="apiKey.id"
          class="rounded-lg border border-border bg-card p-4 transition-colors hover:border-primary/30"
        >
          <div class="flex items-center justify-between gap-3">
            <div class="flex min-w-0 flex-1 items-center gap-3">
              <div class="min-w-0 flex-1">
                <div class="flex flex-wrap items-center gap-2">
                  <span class="font-semibold text-foreground">
                    {{ apiKey.name || legacyT('未命名 API Key') }}
                  </span>
                  <Badge
                    :variant="apiKey.is_active ? 'success' : 'secondary'"
                    class="text-xs"
                  >
                    {{ legacyT(apiKey.is_active ? '活跃' : '禁用') }}
                  </Badge>
                  <Badge
                    v-if="apiKey.is_locked"
                    variant="secondary"
                    class="text-xs"
                  >
                    {{ legacyT('已锁定') }}
                  </Badge>
                  <Badge
                    v-if="apiKey.is_standalone"
                    variant="default"
                    class="bg-purple-500 text-xs"
                  >
                    {{ legacyT('独立余额') }}
                  </Badge>
                  <Badge
                    variant="secondary"
                    class="text-xs"
                  >
                    {{ legacyT(formatRateLimit(apiKey.rate_limit)) }}
                  </Badge>
                  <Badge
                    variant="secondary"
                    class="text-xs"
                  >
                    {{ legacyT(formatConcurrentLimit(apiKey.concurrent_limit)) }}
                  </Badge>
                </div>
                <div class="mt-0.5 flex items-center gap-1">
                  <code class="font-mono text-xs text-muted-foreground">
                    {{ apiKey.key_display || '****' }}
                  </code>
                  <span class="text-xs text-muted-foreground">
                    {{ legacyT('IP 限制：') }}{{ legacyT(formatIpRules(apiKey.ip_rules)) }}
                  </span>
                  <button
                    class="rounded p-0.5 transition-colors hover:bg-muted"
                    :title="legacyT('复制完整密钥')"
                    @click="$emit('copy-full-key', apiKey)"
                  >
                    <Copy class="h-3 w-3 text-muted-foreground" />
                  </button>
                </div>
              </div>
            </div>
            <div class="flex flex-shrink-0 items-center gap-4">
              <div class="text-right text-sm">
                <div class="text-muted-foreground">
                  {{ (apiKey.total_requests || 0).toLocaleString() }} {{ legacyT('次') }}
                </div>
                <div class="font-semibold text-rose-600">
                  ${{ (apiKey.total_cost_usd || 0).toFixed(4) }}
                </div>
              </div>
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                :title="legacyT('编辑')"
                @click="$emit('edit-key', apiKey)"
              >
                <SquarePen class="h-4 w-4" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                :title="legacyT(apiKey.is_locked ? '解锁' : '锁定')"
                @click="$emit('toggle-lock', apiKey)"
              >
                <Lock
                  v-if="apiKey.is_locked"
                  class="h-4 w-4"
                />
                <LockOpen
                  v-else
                  class="h-4 w-4"
                />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                :title="legacyT('删除')"
                @click="$emit('delete-key', apiKey)"
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
              {{ legacyT('暂无 API Keys') }}
            </p>
            <p class="text-sm text-muted-foreground">
              {{ legacyT('点击下方按钮创建') }}
            </p>
          </div>
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
        @click="$emit('create-key')"
      >
        {{ creating ? legacyT('创建中...') : legacyT('创建') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { Copy, Key, Lock, LockOpen, SquarePen, Trash2 } from 'lucide-vue-next'
import { Badge, Button, Dialog } from '@/components/ui'
import { useI18n } from '@/i18n'
import type { ApiKey } from '@/api/users'

defineProps<{
  open: boolean
  apiKeys: ApiKey[]
  creating: boolean
  formatRateLimit: (rateLimit?: number | null) => string
  formatConcurrentLimit: (concurrentLimit?: number | null) => string
  formatIpRules: (ipRules?: string[] | null) => string
}>()

defineEmits<{
  close: []
  'create-key': []
  'edit-key': [apiKey: ApiKey]
  'toggle-lock': [apiKey: ApiKey]
  'delete-key': [apiKey: ApiKey]
  'copy-full-key': [apiKey: ApiKey]
}>()

const { legacyT } = useI18n()
</script>
