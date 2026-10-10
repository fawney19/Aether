<template>
  <Card
    class="overflow-visible"
    data-list="relationships"
  >
    <div class="flex flex-wrap items-center gap-3 border-b px-4 py-3 sm:px-5">
      <h2 class="shrink-0 text-sm font-medium">
        邀请关系<span class="ml-2 text-xs font-normal text-muted-foreground">{{ total }} 条记录</span>
      </h2>
      <form
        class="flex min-w-0 basis-full flex-wrap items-center gap-2 lg:flex-1 lg:basis-0"
        @submit.prevent="$emit('query')"
      >
        <ReferralUserSelect
          v-model="filters.inviter"
          class="w-full sm:w-36"
          label="邀请人"
        /><ReferralUserSelect
          v-model="filters.invitee"
          class="w-full sm:w-36"
          label="被邀请人"
        /><Input
          v-model="filters.invite_code"
          class="h-9 w-full sm:w-36"
          placeholder="邀请码"
          aria-label="邀请码"
        />
        <select
          v-model="filters.first_paid"
          class="h-9 w-full min-w-0 rounded-lg border border-border bg-background px-2 text-sm sm:w-auto"
          aria-label="首付状态"
        >
          <option value="all">
            全部首付状态
          </option><option value="true">
            已首付
          </option><option value="false">
            未首付
          </option>
        </select>
        <div class="flex gap-1 sm:ml-auto">
          <Button
            type="submit"
            variant="outline"
            size="sm"
            class="h-9"
            :aria-busy="loading"
          >
            <Search class="mr-1.5 h-3.5 w-3.5" />查询
          </Button><Button
            type="button"
            variant="ghost"
            size="sm"
            class="h-9 text-muted-foreground"
            @click="$emit('reset')"
          >
            重置
          </Button>
        </div>
      </form>
    </div>
    <div
      v-if="error"
      role="alert"
      class="flex items-center gap-2 p-5 text-sm text-destructive"
    >
      {{ error }}<Button
        variant="outline"
        size="sm"
        @click="$emit('reload')"
      >
        重试
      </Button>
    </div>
    <div
      v-else-if="loading"
      role="status"
      class="flex items-center justify-center gap-2 py-14 text-sm text-muted-foreground"
    >
      <LoaderCircle class="h-4 w-4 animate-spin" />正在加载邀请关系…
    </div>
    <div
      v-else-if="!items.length"
      class="flex flex-col items-center gap-3 px-5 py-14 text-center"
    >
      <span class="flex h-11 w-11 items-center justify-center rounded-2xl bg-muted/60 text-muted-foreground"><Users class="h-5 w-5" /></span><p class="text-sm text-muted-foreground">
        暂无符合条件的邀请关系
      </p>
    </div>
    <template v-else>
      <div class="hidden overflow-x-auto xl:block">
        <Table class="min-w-[920px]">
          <TableHeader>
            <TableRow class="bg-muted/25">
              <TableHead class="pl-5">
                邀请人
              </TableHead><TableHead>被邀请人</TableHead><TableHead>邀请码</TableHead><TableHead>绑定时间</TableHead><TableHead>首次实际付款</TableHead><TableHead class="pr-5 text-right">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="item in items"
              :key="item.id"
            >
              <TableCell class="py-4 pl-5">
                <div class="flex items-center gap-2.5">
                  <span
                    class="avatar bg-primary/10 text-primary"
                    aria-hidden="true"
                  >{{ initial(item.inviter_username || item.inviter_user_id) }}</span><span
                    class="max-w-40 truncate font-medium"
                    :title="item.inviter_username || item.inviter_user_id"
                  >{{ item.inviter_username || item.inviter_user_id }}</span><Button
                    variant="ghost"
                    size="sm"
                    class="copy-button"
                    aria-label="复制邀请人 ID"
                    @click="$emit('copy', item.inviter_user_id)"
                  >
                    <Copy class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </TableCell>
              <TableCell>
                <div class="flex items-center gap-2.5">
                  <span
                    class="avatar bg-muted text-muted-foreground"
                    aria-hidden="true"
                  >{{ initial(item.invitee_username || item.invitee_user_id) }}</span><span
                    class="max-w-40 truncate"
                    :title="item.invitee_username || item.invitee_user_id"
                  >{{ item.invitee_username || item.invitee_user_id }}</span><Button
                    variant="ghost"
                    size="sm"
                    class="copy-button"
                    aria-label="复制被邀请人 ID"
                    @click="$emit('copy', item.invitee_user_id)"
                  >
                    <Copy class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </TableCell>
              <TableCell>
                <div class="flex items-center gap-1">
                  <code class="rounded-md bg-muted/50 px-2 py-1 text-xs">{{ item.invite_code_snapshot }}</code><Button
                    variant="ghost"
                    size="sm"
                    class="copy-button"
                    aria-label="复制邀请码"
                    @click="$emit('copy', item.invite_code_snapshot)"
                  >
                    <Copy class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </TableCell>
              <TableCell class="max-w-36 text-xs leading-5 text-muted-foreground">
                {{ date(item.created_at_unix_secs) }}
              </TableCell>
              <TableCell>
                <div class="flex items-center gap-1">
                  <Badge :variant="item.first_paid_order_id ? 'success' : 'secondary'">
                    {{ item.first_paid_order_id ? '已首付' : '未首付' }}
                  </Badge><Button
                    v-if="item.first_paid_order_id"
                    variant="ghost"
                    size="sm"
                    class="copy-button"
                    aria-label="复制首付订单 ID"
                    @click="$emit('copy', item.first_paid_order_id)"
                  >
                    <Copy class="h-3.5 w-3.5" />
                  </Button>
                </div><p
                  v-if="item.first_paid_order_id"
                  class="mt-1 text-xs text-muted-foreground"
                >
                  {{ date(item.first_paid_at_unix_secs) }}
                </p>
              </TableCell>
              <TableCell class="pr-5 text-right">
                <Button
                  size="sm"
                  variant="ghost"
                  class="h-8 px-2 text-primary"
                  @click="$emit('rewards', item.id)"
                >
                  查看返利<ChevronRight class="ml-1 h-3.5 w-3.5" />
                </Button>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>
      <div class="divide-y xl:hidden">
        <article
          v-for="item in items"
          :key="item.id"
          class="space-y-4 p-4 sm:p-5"
        >
          <div class="flex items-start justify-between gap-2">
            <div class="min-w-0 space-y-2">
              <div class="flex items-center gap-2">
                <span
                  class="avatar bg-primary/10 text-primary"
                  aria-hidden="true"
                >{{ initial(item.inviter_username || item.inviter_user_id) }}</span><span class="min-w-0 break-all text-sm font-medium">{{ item.inviter_username || item.inviter_user_id }}</span><Button
                  variant="ghost"
                  size="sm"
                  class="copy-button"
                  aria-label="复制邀请人 ID"
                  @click="$emit('copy', item.inviter_user_id)"
                >
                  <Copy class="h-3.5 w-3.5" />
                </Button>
              </div><div class="flex items-center gap-2 pl-2">
                <CornerDownRight class="h-4 w-4 shrink-0 text-muted-foreground" /><span class="min-w-0 break-all text-sm">{{ item.invitee_username || item.invitee_user_id }}</span><Button
                  variant="ghost"
                  size="sm"
                  class="copy-button"
                  aria-label="复制被邀请人 ID"
                  @click="$emit('copy', item.invitee_user_id)"
                >
                  <Copy class="h-3.5 w-3.5" />
                </Button>
              </div>
            </div><Badge
              class="shrink-0"
              :variant="item.first_paid_order_id ? 'success' : 'secondary'"
            >
              {{ item.first_paid_order_id ? '已首付' : '未首付' }}
            </Badge>
          </div>
          <div class="flex flex-wrap items-center gap-1 text-xs text-muted-foreground">
            <span>邀请码</span><code class="rounded-md bg-muted/50 px-2 py-1 text-foreground">{{ item.invite_code_snapshot }}</code><Button
              variant="ghost"
              size="sm"
              class="copy-button"
              aria-label="复制邀请码"
              @click="$emit('copy', item.invite_code_snapshot)"
            >
              <Copy class="h-3.5 w-3.5" />
            </Button>
          </div>
          <p class="text-xs text-muted-foreground">
            绑定于 {{ date(item.created_at_unix_secs) }}
          </p>
          <div
            v-if="item.first_paid_order_id"
            class="flex items-center gap-1 text-xs text-muted-foreground"
          >
            <p>首付于 {{ date(item.first_paid_at_unix_secs) }}</p><Button
              variant="ghost"
              size="sm"
              class="copy-button"
              aria-label="复制首付订单 ID"
              @click="$emit('copy', item.first_paid_order_id)"
            >
              <Copy class="h-3.5 w-3.5" />
            </Button>
          </div>
          <div class="flex justify-end border-t pt-3">
            <Button
              size="sm"
              variant="outline"
              class="h-8"
              @click="$emit('rewards', item.id)"
            >
              查看返利<ChevronRight class="ml-1 h-3.5 w-3.5" />
            </Button>
          </div>
        </article>
      </div>
    </template>
    <Pagination
      :current="page"
      :total="total"
      :page-size="pageSize"
      @update:current="$emit('page', $event)"
      @update:page-size="$emit('size', $event)"
    />
  </Card>
</template>

<script setup lang="ts">
import { ChevronRight, Copy, CornerDownRight, LoaderCircle, Search, Users } from 'lucide-vue-next'
import type { ReferralRelationshipRecord } from '@/api/referrals'
import { Badge, Button, Card, Input, Pagination, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui'
import { date } from './presentation'
import ReferralUserSelect from './ReferralUserSelect.vue'

defineProps<{ items: ReferralRelationshipRecord[]; total: number; page: number; pageSize: number; loading: boolean; error: string }>()
defineEmits<{ query: []; reset: []; reload: []; page: [value: number]; size: [value: number]; rewards: [id: string]; copy: [value: string] }>()
const filters = defineModel<{ inviter: string; invitee: string; invite_code: string; first_paid: string }>('filters', { required: true })
const initial = (name: string) => Array.from(name)[0]?.toLocaleUpperCase() || '?'
</script>

<style scoped>
.avatar { @apply flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold; }
.copy-button { @apply h-7 w-7 shrink-0 rounded-md p-0 text-muted-foreground; }
</style>
