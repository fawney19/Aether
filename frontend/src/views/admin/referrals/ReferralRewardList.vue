<template>
  <Card
    class="overflow-visible"
    :data-list="kind"
  >
    <div class="flex flex-wrap items-center gap-3 border-b px-4 py-3 sm:px-5">
      <h2 class="shrink-0 text-sm font-medium">
        {{ title }} <span class="ml-2 text-xs font-normal text-muted-foreground">{{ total }} 条记录</span>
      </h2>
      <div class="order-last min-w-0 basis-full lg:order-none lg:flex-1 lg:basis-0">
        <slot name="filters" />
      </div>
      <Button
        variant="ghost"
        size="sm"
        class="ml-auto h-7 w-7 shrink-0 p-0 text-muted-foreground lg:ml-0"
        :aria-busy="loading"
        aria-label="刷新列表"
        @click="$emit('reload')"
      >
        <RefreshCw
          class="h-3.5 w-3.5"
          :class="{ 'animate-spin': loading }"
        /><span class="sr-only">刷新列表</span>
      </Button>
    </div>
    <div
      v-if="error"
      role="alert"
      class="flex flex-wrap items-center gap-2 p-5 text-sm text-destructive"
    >
      <AlertCircle class="h-4 w-4" />{{ error }}<Button
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
      <LoaderCircle class="h-4 w-4 animate-spin" />正在加载{{ title }}…
    </div>
    <div
      v-else-if="items.length === 0"
      class="flex flex-col items-center gap-3 px-5 py-14 text-center"
    >
      <span class="flex h-11 w-11 items-center justify-center rounded-2xl bg-muted/60 text-muted-foreground"><Inbox class="h-5 w-5" /></span><p class="text-sm text-muted-foreground">
        {{ kind === 'failed' ? '没有发放失败的返利' : kind === 'debt' ? '没有待冲回款项' : '暂无符合条件的返利记录' }}
      </p>
    </div>
    <template v-else>
      <div class="hidden overflow-x-auto xl:block">
        <Table class="min-w-[920px]">
          <TableHeader>
            <TableRow class="bg-muted/25">
              <TableHead class="pl-5">
                邀请双方
              </TableHead><TableHead>奖励来源</TableHead><TableHead>金额与冲回</TableHead><TableHead>状态</TableHead><TableHead>{{ kind === 'failed' ? '更新时间' : '创建时间' }}</TableHead><TableHead class="pr-5 text-right">
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
                <div class="flex items-center gap-3">
                  <span
                    class="flex h-9 w-9 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-semibold text-primary"
                    aria-hidden="true"
                  >{{ initial(item.inviter_username || item.inviter_user_id) }}</span><div class="min-w-0">
                    <p
                      class="max-w-44 truncate font-medium"
                      :title="item.inviter_username || item.inviter_user_id"
                    >
                      {{ item.inviter_username || item.inviter_user_id }}
                    </p><p class="mt-1 flex items-center gap-1 text-xs text-muted-foreground">
                      <CornerDownRight class="h-3 w-3 shrink-0" /><span
                        class="max-w-40 truncate"
                        :title="item.invitee_username || item.invitee_user_id"
                      >{{ item.invitee_username || item.invitee_user_id }}</span>
                    </p>
                  </div>
                </div>
              </TableCell>
              <TableCell>
                <p class="text-sm">
                  {{ rewardType(item.reward_type) }}<span class="ml-1.5 text-xs text-muted-foreground">{{ trigger(item.trigger_point) }}</span>
                </p><p
                  class="mt-1 max-w-48 truncate text-xs text-muted-foreground"
                  :title="source(item)"
                >
                  {{ source(item) }}
                </p>
              </TableCell>
              <TableCell class="whitespace-nowrap">
                <p class="font-semibold tabular-nums">
                  <span class="mr-1 text-xs font-normal text-muted-foreground">原额</span> {{ usd(item.amount_usd) }}
                </p><p class="mt-1 text-xs text-muted-foreground">
                  已冲回 {{ usd(item.reversed_amount_usd) }}
                </p><p
                  v-if="item.pending_reversal_amount_usd > 0"
                  class="mt-0.5 text-xs text-amber-700 dark:text-amber-400"
                >
                  待冲回 {{ usd(item.pending_reversal_amount_usd) }}
                </p>
              </TableCell>
              <TableCell>
                <Badge :variant="statusVariant(item.status)">
                  {{ rewardStatus(item.status) }}
                </Badge><p
                  v-if="reversalStatus(item) !== '无冲回'"
                  class="mt-1.5 text-xs text-muted-foreground"
                >
                  {{ reversalStatus(item) }}
                </p>
              </TableCell>
              <TableCell class="max-w-36 text-xs leading-5 text-muted-foreground">
                {{ date(kind === 'failed' ? item.updated_at_unix_secs : item.created_at_unix_secs) }}
              </TableCell>
              <TableCell class="pr-5">
                <div class="flex justify-end gap-1 whitespace-nowrap [&>button]:shrink-0">
                  <Button
                    variant="ghost"
                    size="sm"
                    class="h-8 px-2 text-primary"
                    @click="$emit('detail', item.id)"
                  >
                    详情<ChevronRight class="ml-0.5 h-3.5 w-3.5" />
                  </Button><Button
                    v-if="item.status === 'failed'"
                    variant="outline"
                    size="sm"
                    class="h-8 px-2 text-xs"
                    :disabled="!!mutating"
                    @click="$emit('operate', 'retry', item)"
                  >
                    重试发放
                  </Button><Button
                    v-if="item.status === 'failed' || item.status === 'pending'"
                    variant="ghost"
                    size="sm"
                    class="h-8 px-2 text-xs text-muted-foreground hover:text-destructive"
                    :disabled="!!mutating"
                    @click="$emit('operate', 'void', item)"
                  >
                    作废
                  </Button>
                </div>
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
          <div class="flex items-start justify-between gap-3">
            <div class="min-w-0">
              <p class="text-xs text-muted-foreground">
                {{ rewardType(item.reward_type) }} · {{ trigger(item.trigger_point) }}
              </p><p class="mt-1 break-all text-xl font-semibold tabular-nums">
                <span class="mr-1 text-xs font-normal text-muted-foreground">原额</span> {{ usd(item.amount_usd) }}
              </p>
            </div><div class="shrink-0 text-right">
              <Badge :variant="statusVariant(item.status)">
                {{ rewardStatus(item.status) }}
              </Badge><p
                v-if="reversalStatus(item) !== '无冲回'"
                class="mt-1 text-xs text-muted-foreground"
              >
                {{ reversalStatus(item) }}
              </p>
            </div>
          </div>
          <div class="flex items-center gap-2 rounded-lg bg-muted/35 p-3">
            <span
              class="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary/10 text-xs font-semibold text-primary"
              aria-hidden="true"
            >{{ initial(item.inviter_username || item.inviter_user_id) }}</span><p class="min-w-0 break-all text-sm">
              <span class="font-medium">{{ item.inviter_username || item.inviter_user_id }}</span><span class="mx-1.5 text-muted-foreground">→</span>{{ item.invitee_username || item.invitee_user_id }}
            </p>
          </div>
          <div class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
            <p>已冲回 {{ usd(item.reversed_amount_usd) }}</p><p
              v-if="item.pending_reversal_amount_usd > 0"
              class="text-amber-700 dark:text-amber-400"
            >
              待冲回 {{ usd(item.pending_reversal_amount_usd) }}
            </p>
          </div>
          <p class="break-all text-xs text-muted-foreground">
            {{ source(item) }}
          </p>
          <div class="flex flex-wrap items-center justify-between gap-2 border-t pt-3">
            <p class="text-xs text-muted-foreground">
              {{ date(kind === 'failed' ? item.updated_at_unix_secs : item.created_at_unix_secs) }}
            </p><div class="flex flex-wrap gap-1">
              <Button
                variant="outline"
                size="sm"
                class="h-8 px-2.5"
                @click="$emit('detail', item.id)"
              >
                详情
              </Button><Button
                v-if="item.status === 'failed'"
                variant="outline"
                size="sm"
                class="h-8 px-2.5"
                :disabled="!!mutating"
                @click="$emit('operate', 'retry', item)"
              >
                重试发放
              </Button><Button
                v-if="item.status === 'failed' || item.status === 'pending'"
                variant="ghost"
                size="sm"
                class="h-8 px-2 text-muted-foreground hover:text-destructive"
                :disabled="!!mutating"
                @click="$emit('operate', 'void', item)"
              >
                作废
              </Button>
            </div>
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
import { AlertCircle, ChevronRight, CornerDownRight, Inbox, LoaderCircle, RefreshCw } from 'lucide-vue-next'
import type { ReferralRewardRecord } from '@/api/referrals'
import { Badge, Button, Card, Pagination, Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui'
import { date, reversalStatus, rewardStatus, rewardType, source, statusVariant, trigger, usd } from './presentation'

defineProps<{ title: string; kind: string; items: ReferralRewardRecord[]; total: number; page: number; pageSize: number; loading: boolean; error: string; mutating: string | null }>()
defineEmits<{ reload: []; page: [value: number]; size: [value: number]; detail: [id: string]; operate: [action: 'retry' | 'void', reward: ReferralRewardRecord] }>()
const initial = (name: string) => Array.from(name)[0]?.toLocaleUpperCase() || '?'
</script>
