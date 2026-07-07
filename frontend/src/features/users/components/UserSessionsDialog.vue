<template>
  <Dialog
    :model-value="open"
    size="xl"
    @update:model-value="(value) => !value && $emit('close')"
  >
    <template #header>
      <div class="border-b border-border px-6 py-4">
        <div class="flex items-center gap-3">
          <div class="flex h-9 w-9 flex-shrink-0 items-center justify-center rounded-lg bg-primary/10">
            <MonitorSmartphone class="h-5 w-5 text-primary" />
          </div>
          <div class="min-w-0 flex-1">
            <h3 class="text-lg font-semibold leading-tight text-foreground">
              {{ legacyT('登录设备') }}
            </h3>
            <p class="text-xs text-muted-foreground">
              {{ legacyT('查看并强制下线该用户的设备会话') }}
            </p>
          </div>
        </div>
      </div>
    </template>

    <div class="max-h-[60vh] space-y-3 overflow-y-auto">
      <div
        v-if="loading"
        class="rounded-lg border border-dashed border-border/60 bg-muted/20 px-4 py-10 text-center text-sm text-muted-foreground"
      >
        {{ legacyT('正在加载设备会话...') }}
      </div>
      <div
        v-else-if="sessions.length === 0"
        class="rounded-lg border border-dashed border-border/60 bg-muted/20 px-4 py-10 text-center text-sm text-muted-foreground"
      >
        {{ legacyT('暂无在线设备') }}
      </div>
      <div
        v-else
        class="space-y-3"
      >
        <div
          v-for="session in sessions"
          :key="session.id"
          class="rounded-lg border border-border bg-card p-4 transition-colors hover:border-primary/30"
        >
          <div class="flex items-center justify-between gap-3">
            <div class="min-w-0 flex-1">
              <div class="font-semibold text-foreground">
                {{ session.device_label }}
              </div>
              <div class="mt-1 text-xs text-muted-foreground">
                {{ formatSessionMeta(session) }}
              </div>
              <div class="mt-1 text-xs text-muted-foreground">
                {{ legacyT('最近活跃') }} {{ formatDate(session.last_seen_at || session.created_at) }}
                <span v-if="session.ip_address"> · IP {{ session.ip_address }}</span>
              </div>
            </div>
            <Button
              variant="outline"
              size="sm"
              :disabled="actionLoading === session.id"
              @click="$emit('revoke-session', session.id)"
            >
              {{ actionLoading === session.id ? legacyT('处理中...') : legacyT('强制下线') }}
            </Button>
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
        {{ legacyT('关闭') }}
      </Button>
      <Button
        class="h-10 px-5"
        :disabled="loading || sessions.length === 0 || actionLoading === 'all'"
        @click="$emit('revoke-all')"
      >
        {{ actionLoading === 'all' ? legacyT('处理中...') : legacyT('全部下线') }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { MonitorSmartphone } from 'lucide-vue-next'
import { Button, Dialog } from '@/components/ui'
import { useI18n } from '@/i18n'
import type { UserSession } from '@/api/users'

defineProps<{
  open: boolean
  sessions: UserSession[]
  loading: boolean
  actionLoading: string | null
  formatDate: (dateString: string) => string
  formatSessionMeta: (session: UserSession) => string
}>()

defineEmits<{
  close: []
  'revoke-session': [sessionId: string]
  'revoke-all': []
}>()

const { legacyT } = useI18n()
</script>
