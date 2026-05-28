<template>
  <div class="space-y-6">
    <CardSection
      title="Webhook 出站通知"
      description="把 Aether 事件投递到外部系统，支持签名、重试、测试发送和投递日志。"
    >
      <template #actions>
        <div class="flex w-full flex-wrap gap-2 sm:w-auto sm:justify-end">
          <Button
            size="sm"
            variant="outline"
            class="whitespace-nowrap"
            :disabled="loading"
            @click="loadAll"
          >
            <RefreshCw
              class="mr-1.5 h-4 w-4"
              :class="{ 'animate-spin': loading }"
            />
            刷新
          </Button>
          <Button
            size="sm"
            class="whitespace-nowrap"
            @click="openCreateEndpointDialog"
          >
            <Plus class="mr-1.5 h-4 w-4" />
            <span class="sm:hidden">新增</span>
            <span class="hidden sm:inline">新增 Webhook</span>
          </Button>
        </div>
      </template>

      <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <div class="rounded-xl border border-border bg-muted/30 px-4 py-3">
          <p class="text-xs text-muted-foreground">
            Endpoint
          </p>
          <p class="mt-1 text-2xl font-semibold">
            {{ endpoints.length }}
          </p>
        </div>
        <div class="rounded-xl border border-border bg-muted/30 px-4 py-3">
          <p class="text-xs text-muted-foreground">
            已启用
          </p>
          <p class="mt-1 text-2xl font-semibold">
            {{ enabledEndpointCount }}
          </p>
        </div>
        <div class="rounded-xl border border-border bg-muted/30 px-4 py-3">
          <p class="text-xs text-muted-foreground">
            待重试
          </p>
          <p class="mt-1 text-2xl font-semibold">
            {{ retryableDeliveryCount }}
          </p>
        </div>
        <div class="rounded-xl border border-border bg-muted/30 px-4 py-3">
          <p class="text-xs text-muted-foreground">
            最近投递
          </p>
          <p class="mt-1 truncate text-sm font-medium">
            {{ latestDeliveryText }}
          </p>
        </div>
      </div>
    </CardSection>

    <div class="grid gap-6 xl:grid-cols-[minmax(0,1fr)_360px]">
      <CardSection
        title="Endpoint 管理"
        description="每个 Endpoint 可独立启停、订阅事件并配置重试预算。"
      >
        <div
          v-if="endpoints.length === 0"
          class="rounded-xl border border-dashed border-border bg-card px-6 py-12 text-center"
        >
          <Webhook class="mx-auto h-8 w-8 text-muted-foreground" />
          <p class="mt-3 text-sm font-medium text-foreground">
            还没有 Webhook Endpoint
          </p>
          <Button
            class="mt-4"
            variant="outline"
            @click="openCreateEndpointDialog"
          >
            <Plus class="mr-1.5 h-4 w-4" />
            新增 Endpoint
          </Button>
        </div>

        <div
          v-else
          class="space-y-3"
        >
          <section
            v-for="endpoint in endpoints"
            :key="endpoint.id"
            class="rounded-xl border border-border bg-card p-4 shadow-sm"
          >
            <div class="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
              <div class="min-w-0">
                <div class="flex flex-wrap items-center gap-2">
                  <h3 class="truncate text-base font-semibold text-foreground">
                    {{ endpoint.name }}
                  </h3>
                  <Badge :variant="endpoint.enabled ? 'success' : 'outline'">
                    {{ endpoint.enabled ? '已启用' : '已停用' }}
                  </Badge>
                  <Badge
                    v-if="endpoint.secret_set"
                    variant="outline"
                  >
                    HMAC 签名
                  </Badge>
                  <Badge
                    v-if="endpoint.last_delivery_status"
                    :variant="deliveryStatusBadgeVariant(endpoint.last_delivery_status)"
                  >
                    {{ deliveryStatusLabel(endpoint.last_delivery_status) }}
                  </Badge>
                </div>
                <p class="mt-1 truncate text-xs text-muted-foreground">
                  {{ endpoint.url }}
                </p>
                <div class="mt-3 flex flex-wrap gap-2">
                  <span
                    v-for="eventType in endpoint.subscribed_events"
                    :key="eventType"
                    class="rounded-full border border-border bg-muted/40 px-2.5 py-1 text-xs text-muted-foreground"
                  >
                    {{ eventName(eventType) }}
                  </span>
                </div>
              </div>

              <div class="flex flex-wrap items-center gap-2">
                <Switch
                  :model-value="endpoint.enabled"
                  :disabled="busyEndpointId === endpoint.id"
                  @update:model-value="value => updateEndpointEnabled(endpoint, value)"
                />
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-9 w-9"
                  title="编辑"
                  @click="openEditEndpointDialog(endpoint)"
                >
                  <Pencil class="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-9 w-9"
                  title="发送测试"
                  @click="openTestDialog(endpoint)"
                >
                  <Send class="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-9 w-9 text-destructive"
                  title="删除"
                  :disabled="busyEndpointId === endpoint.id"
                  @click="deleteEndpoint(endpoint)"
                >
                  <Trash2 class="h-4 w-4" />
                </Button>
              </div>
            </div>

            <div class="mt-4 grid gap-3 text-xs text-muted-foreground sm:grid-cols-3">
              <div class="rounded-lg border border-border/70 bg-muted/20 px-3 py-2">
                超时 <span class="font-medium text-foreground">{{ endpoint.timeout_ms }}ms</span>
              </div>
              <div class="rounded-lg border border-border/70 bg-muted/20 px-3 py-2">
                重试 <span class="font-medium text-foreground">{{ endpoint.max_retries }}</span> 次
              </div>
              <div class="rounded-lg border border-border/70 bg-muted/20 px-3 py-2">
                失败累计 <span class="font-medium text-foreground">{{ endpoint.failure_count }}</span>
              </div>
            </div>
          </section>
        </div>
      </CardSection>

      <CardSection
        title="事件目录"
        description="可订阅事件和对应的投递语义。"
      >
        <div class="space-y-3">
          <div
            v-for="definition in eventDefinitions"
            :key="definition.type"
            class="rounded-lg border border-border/70 px-3 py-3"
          >
            <div class="flex items-center justify-between gap-3">
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <p class="truncate text-sm font-medium text-foreground">
                    {{ definition.name }}
                  </p>
                  <Badge
                    v-if="definition.high_priority"
                    variant="warning"
                  >
                    高优先级
                  </Badge>
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  {{ definition.description }}
                </p>
              </div>
            </div>
            <p class="mt-2 font-mono text-[11px] text-muted-foreground">
              {{ definition.type }}
            </p>
          </div>
        </div>
      </CardSection>
    </div>

    <CardSection
      title="投递日志"
      description="展示最近 Webhook 投递状态、失败原因和下一次重试时间。"
    >
      <template #actions>
        <div class="flex w-full flex-wrap items-center gap-2 sm:w-auto sm:justify-end">
          <Select v-model="deliveryEndpointFilter">
            <SelectTrigger class="h-9 w-full sm:w-44">
              <SelectValue placeholder="Endpoint" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">
                全部 Endpoint
              </SelectItem>
              <SelectItem
                v-for="endpoint in endpoints"
                :key="endpoint.id"
                :value="endpoint.id"
              >
                {{ endpoint.name }}
              </SelectItem>
            </SelectContent>
          </Select>
          <Select v-model="deliveryStatusFilter">
            <SelectTrigger class="h-9 w-full sm:w-32">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">
                全部状态
              </SelectItem>
              <SelectItem value="succeeded">
                成功
              </SelectItem>
              <SelectItem value="failed">
                失败
              </SelectItem>
              <SelectItem value="retrying">
                重试中
              </SelectItem>
              <SelectItem value="dead">
                已终止
              </SelectItem>
            </SelectContent>
          </Select>
          <Select v-model="deliveryEventFilter">
            <SelectTrigger class="h-9 w-full sm:w-40">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">
                全部事件
              </SelectItem>
              <SelectItem
                v-for="definition in eventDefinitions"
                :key="definition.type"
                :value="definition.type"
              >
                {{ definition.name }}
              </SelectItem>
            </SelectContent>
          </Select>
          <Button
            size="sm"
            variant="outline"
            class="whitespace-nowrap"
            :disabled="loadingDeliveries"
            @click="loadDeliveries"
          >
            <RefreshCw
              class="mr-1.5 h-4 w-4"
              :class="{ 'animate-spin': loadingDeliveries }"
            />
            刷新日志
          </Button>
        </div>
      </template>

      <div
        v-if="deliveries.length === 0"
        class="rounded-xl border border-dashed border-border bg-card px-6 py-10 text-center"
      >
        <ListChecks class="mx-auto h-8 w-8 text-muted-foreground" />
        <p class="mt-3 text-sm font-medium text-foreground">
          暂无投递记录
        </p>
      </div>

      <div v-else>
        <Table class="hidden lg:table">
          <TableHeader>
            <TableRow>
              <TableHead>事件</TableHead>
              <TableHead>Endpoint</TableHead>
              <TableHead>状态</TableHead>
              <TableHead>尝试</TableHead>
              <TableHead>耗时</TableHead>
              <TableHead>时间</TableHead>
              <TableHead class="text-right">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="delivery in deliveries"
              :key="delivery.id"
            >
              <TableCell>
                <div class="min-w-0">
                  <p class="truncate text-sm font-medium">
                    {{ eventName(delivery.event_type) }}
                  </p>
                  <p class="font-mono text-xs text-muted-foreground">
                    {{ delivery.request_id || delivery.id }}
                  </p>
                </div>
              </TableCell>
              <TableCell class="max-w-[220px] truncate">
                {{ delivery.endpoint_name }}
              </TableCell>
              <TableCell>
                <Badge :variant="deliveryStatusBadgeVariant(delivery.status)">
                  {{ deliveryStatusLabel(delivery.status) }}
                </Badge>
              </TableCell>
              <TableCell class="tabular-nums">
                {{ delivery.attempt_count }} / {{ delivery.max_attempts }}
              </TableCell>
              <TableCell class="tabular-nums">
                {{ delivery.duration_ms === null ? '-' : `${delivery.duration_ms}ms` }}
              </TableCell>
              <TableCell class="text-xs text-muted-foreground">
                {{ formatDateTime(delivery.created_at) }}
              </TableCell>
              <TableCell>
                <div class="flex justify-end gap-1">
                  <Button
                    size="sm"
                    variant="ghost"
                    @click="openDeliveryDialog(delivery)"
                  >
                    查看
                  </Button>
                  <Button
                    v-if="canRetryDelivery(delivery)"
                    size="sm"
                    variant="outline"
                    :disabled="busyDeliveryId === delivery.id"
                    @click="retryDelivery(delivery)"
                  >
                    <RotateCcw class="mr-1.5 h-4 w-4" />
                    重试
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>

        <div class="space-y-3 lg:hidden">
          <section
            v-for="delivery in deliveries"
            :key="delivery.id"
            class="rounded-xl border border-border bg-card p-4"
          >
            <div class="flex items-start justify-between gap-3">
              <div class="min-w-0">
                <p class="truncate text-sm font-semibold">
                  {{ eventName(delivery.event_type) }}
                </p>
                <p class="mt-1 truncate text-xs text-muted-foreground">
                  {{ delivery.endpoint_name }}
                </p>
              </div>
              <Badge :variant="deliveryStatusBadgeVariant(delivery.status)">
                {{ deliveryStatusLabel(delivery.status) }}
              </Badge>
            </div>
            <div class="mt-3 grid grid-cols-2 gap-2 text-xs text-muted-foreground">
              <div>尝试 {{ delivery.attempt_count }} / {{ delivery.max_attempts }}</div>
              <div>{{ delivery.duration_ms === null ? '-' : `${delivery.duration_ms}ms` }}</div>
              <div class="col-span-2">
                {{ formatDateTime(delivery.created_at) }}
              </div>
            </div>
            <p
              v-if="delivery.last_error"
              class="mt-3 rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-2 text-xs text-destructive"
            >
              {{ delivery.last_error }}
            </p>
            <div class="mt-3 flex flex-wrap gap-2">
              <Button
                size="sm"
                variant="ghost"
                @click="openDeliveryDialog(delivery)"
              >
                查看详情
              </Button>
              <Button
                v-if="canRetryDelivery(delivery)"
                size="sm"
                variant="outline"
                :disabled="busyDeliveryId === delivery.id"
                @click="retryDelivery(delivery)"
              >
                <RotateCcw class="mr-1.5 h-4 w-4" />
                重试
              </Button>
            </div>
          </section>
        </div>
      </div>
    </CardSection>

    <Dialog
      :open="showEndpointDialog"
      :title="editingEndpointId ? '编辑 Webhook Endpoint' : '新增 Webhook Endpoint'"
      description="配置目标地址、签名密钥、订阅事件和投递重试预算。"
      size="3xl"
      @update:open="showEndpointDialog = $event"
    >
      <div class="space-y-5">
        <div class="flex items-center justify-between gap-4 rounded-lg border border-border/70 px-4 py-3">
          <div>
            <Label class="text-sm font-medium">
              启用 Endpoint
            </Label>
            <p class="mt-1 text-xs text-muted-foreground">
              关闭后保留配置但不再投递新事件。
            </p>
          </div>
          <Switch v-model="endpointForm.enabled" />
        </div>

        <div class="grid gap-4 md:grid-cols-2">
          <div class="space-y-2">
            <Label for="webhook-name">
              名称
            </Label>
            <Input
              id="webhook-name"
              v-model="endpointForm.name"
              maxlength="64"
              placeholder="飞书工单系统"
            />
          </div>
          <div class="space-y-2">
            <Label for="webhook-secret">
              Secret
            </Label>
            <Input
              id="webhook-secret"
              v-model="endpointForm.secret"
              masked
              :placeholder="editingEndpointId ? '留空保持不变' : '留空由后端生成'"
            />
          </div>
          <div class="space-y-2 md:col-span-2">
            <Label for="webhook-url">
              Webhook URL
            </Label>
            <Input
              id="webhook-url"
              v-model="endpointForm.url"
              placeholder="https://ops.example.com/aether/webhook"
            />
          </div>
          <div class="space-y-2">
            <Label for="webhook-timeout">
              超时时间（ms）
            </Label>
            <Input
              id="webhook-timeout"
              v-model.number="endpointForm.timeout_ms"
              type="number"
              min="1000"
              max="30000"
              step="500"
            />
          </div>
          <div class="space-y-2">
            <Label for="webhook-retries">
              最大重试次数
            </Label>
            <Input
              id="webhook-retries"
              v-model.number="endpointForm.max_retries"
              type="number"
              min="0"
              max="10"
            />
          </div>
        </div>

        <div class="space-y-3">
          <div class="flex flex-wrap items-center justify-between gap-2">
            <Label class="text-sm font-medium">
              订阅事件
            </Label>
            <div class="flex gap-2">
              <Button
                size="sm"
                variant="ghost"
                @click="selectAllEvents"
              >
                全选
              </Button>
              <Button
                size="sm"
                variant="ghost"
                @click="selectHighPriorityEvents"
              >
                高优先级
              </Button>
            </div>
          </div>
          <div class="grid gap-3 md:grid-cols-2">
            <label
              v-for="definition in subscribableEventDefinitions"
              :key="definition.type"
              class="flex cursor-pointer gap-3 rounded-lg border border-border/70 px-3 py-3 transition hover:border-primary/40 hover:bg-primary/5"
            >
              <Checkbox
                class="mt-0.5 shrink-0"
                :checked="endpointForm.subscribed_events.includes(definition.type)"
                @update:checked="checked => toggleFormEvent(definition.type, checked)"
              />
              <span class="min-w-0">
                <span class="flex items-center gap-2 text-sm font-medium text-foreground">
                  {{ definition.name }}
                  <Badge
                    v-if="definition.high_priority"
                    variant="warning"
                  >
                    高
                  </Badge>
                </span>
                <span class="mt-1 block text-xs text-muted-foreground">
                  {{ definition.description }}
                </span>
              </span>
            </label>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          :disabled="savingEndpoint"
          @click="saveEndpoint"
        >
          {{ savingEndpoint ? '保存中...' : '保存 Endpoint' }}
        </Button>
        <Button
          variant="outline"
          :disabled="savingEndpoint"
          @click="showEndpointDialog = false"
        >
          取消
        </Button>
      </template>
    </Dialog>

    <Dialog
      :open="showTestDialog"
      title="发送测试投递"
      description="使用已保存配置向目标 Endpoint 投递一条测试事件。"
      size="2xl"
      @update:open="showTestDialog = $event"
    >
      <div class="space-y-4">
        <div class="grid gap-4 md:grid-cols-2">
          <div class="space-y-2">
            <Label>Endpoint</Label>
            <Select v-model="testForm.endpoint_id">
              <SelectTrigger>
                <SelectValue placeholder="选择 Endpoint" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="endpoint in endpoints"
                  :key="endpoint.id"
                  :value="endpoint.id"
                >
                  {{ endpoint.name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div class="space-y-2">
            <Label>事件类型</Label>
            <Select v-model="testForm.event_type">
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="definition in eventDefinitions"
                  :key="definition.type"
                  :value="definition.type"
                >
                  {{ definition.name }}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>
        <div class="space-y-2">
          <Label for="webhook-test-payload">
            测试载荷 JSON
          </Label>
          <Textarea
            id="webhook-test-payload"
            v-model="testForm.payload"
            rows="8"
            class="font-mono text-sm"
            spellcheck="false"
          />
        </div>
      </div>

      <template #footer>
        <Button
          :disabled="testingDelivery || !testForm.endpoint_id"
          @click="sendTestDelivery"
        >
          <Send class="mr-1.5 h-4 w-4" />
          {{ testingDelivery ? '发送中...' : '发送测试' }}
        </Button>
        <Button
          variant="outline"
          :disabled="testingDelivery"
          @click="showTestDialog = false"
        >
          取消
        </Button>
      </template>
    </Dialog>

    <Dialog
      :open="showDeliveryDialog"
      title="投递详情"
      description="用于排查签名、重试和外部系统响应问题。"
      size="2xl"
      @update:open="showDeliveryDialog = $event"
    >
      <div
        v-if="selectedDelivery"
        class="space-y-4"
      >
        <div class="grid gap-3 sm:grid-cols-2">
          <div class="rounded-lg border border-border/70 px-3 py-2">
            <p class="text-xs text-muted-foreground">
              状态
            </p>
            <Badge
              class="mt-1"
              :variant="deliveryStatusBadgeVariant(selectedDelivery.status)"
            >
              {{ deliveryStatusLabel(selectedDelivery.status) }}
            </Badge>
          </div>
          <div class="rounded-lg border border-border/70 px-3 py-2">
            <p class="text-xs text-muted-foreground">
              HTTP 状态码
            </p>
            <p class="mt-1 text-sm font-medium">
              {{ selectedDelivery.status_code ?? '-' }}
            </p>
          </div>
          <div class="rounded-lg border border-border/70 px-3 py-2">
            <p class="text-xs text-muted-foreground">
              下一次重试
            </p>
            <p class="mt-1 text-sm font-medium">
              {{ selectedDelivery.next_retry_at ? formatDateTime(selectedDelivery.next_retry_at) : '-' }}
            </p>
          </div>
          <div class="rounded-lg border border-border/70 px-3 py-2">
            <p class="text-xs text-muted-foreground">
              Request ID
            </p>
            <p class="mt-1 truncate font-mono text-xs">
              {{ selectedDelivery.request_id || selectedDelivery.id }}
            </p>
          </div>
        </div>

        <div
          v-if="selectedDelivery.last_error"
          class="rounded-lg border border-destructive/20 bg-destructive/10 px-3 py-3 text-sm text-destructive"
        >
          {{ selectedDelivery.last_error }}
        </div>

        <div>
          <Label class="text-sm font-medium">
            响应摘要
          </Label>
          <pre class="mt-2 max-h-56 overflow-auto rounded-lg border border-border bg-muted/30 p-3 text-xs text-muted-foreground">{{ selectedDelivery.response_excerpt || '无响应摘要' }}</pre>
        </div>
      </div>

      <template #footer>
        <Button
          v-if="selectedDelivery && canRetryDelivery(selectedDelivery)"
          variant="outline"
          :disabled="busyDeliveryId === selectedDelivery.id"
          @click="retryDelivery(selectedDelivery)"
        >
          <RotateCcw class="mr-1.5 h-4 w-4" />
          重试
        </Button>
        <Button @click="showDeliveryDialog = false">
          关闭
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import {
  ListChecks,
  Pencil,
  Plus,
  RefreshCw,
  RotateCcw,
  Send,
  Trash2,
  Webhook,
} from 'lucide-vue-next'
import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
} from '@/components/ui'
import { CardSection } from '@/components/layout'
import {
  WEBHOOK_EVENT_DEFINITIONS,
  outboundWebhooksApi,
  validateWebhookEndpointUrl,
  type WebhookDeliveryLog,
  type WebhookDeliveryStatus,
  type WebhookEndpoint,
  type WebhookEndpointUpsertRequest,
  type WebhookEventType,
} from '@/api/outboundWebhooks'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

type BadgeVariant = 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark'

interface EndpointForm {
  name: string
  url: string
  enabled: boolean
  subscribed_events: WebhookEventType[]
  secret: string
  timeout_ms: number
  max_retries: number
}

interface TestForm {
  endpoint_id: string
  event_type: WebhookEventType
  payload: string
}

const { success, error } = useToast()

const endpoints = ref<WebhookEndpoint[]>([])
const deliveries = ref<WebhookDeliveryLog[]>([])
const loadingEndpoints = ref(false)
const loadingDeliveries = ref(false)
const savingEndpoint = ref(false)
const testingDelivery = ref(false)
const busyEndpointId = ref<string | null>(null)
const busyDeliveryId = ref<string | null>(null)

const deliveryEndpointFilter = ref('all')
const deliveryStatusFilter = ref<WebhookDeliveryStatus | 'all'>('all')
const deliveryEventFilter = ref<WebhookEventType | 'all'>('all')
const editingEndpointId = ref<string | null>(null)
const showEndpointDialog = ref(false)
const showTestDialog = ref(false)
const showDeliveryDialog = ref(false)
const selectedDelivery = ref<WebhookDeliveryLog | null>(null)

const eventDefinitions = WEBHOOK_EVENT_DEFINITIONS
const subscribableEventDefinitions = WEBHOOK_EVENT_DEFINITIONS.filter(item => item.type !== 'webhook.test')

const endpointForm = ref<EndpointForm>(createDefaultEndpointForm())
const testForm = ref<TestForm>(createDefaultTestForm())

const loading = computed(() => loadingEndpoints.value || loadingDeliveries.value)
const enabledEndpointCount = computed(() => endpoints.value.filter(endpoint => endpoint.enabled).length)
const retryableDeliveryCount = computed(() => deliveries.value.filter(canRetryDelivery).length)
const latestDeliveryText = computed(() => {
  const item = deliveries.value[0]
  if (!item) return '暂无记录'
  return `${eventName(item.event_type)} / ${deliveryStatusLabel(item.status)}`
})

onMounted(() => {
  void loadAll()
})

watch([deliveryEndpointFilter, deliveryStatusFilter, deliveryEventFilter], () => {
  void loadDeliveries()
})

async function loadAll() {
  await Promise.all([loadEndpoints(), loadDeliveries()])
}

async function loadEndpoints() {
  loadingEndpoints.value = true
  try {
    endpoints.value = await outboundWebhooksApi.listEndpoints()
  } catch (err) {
    error(parseApiError(err, '加载 Webhook Endpoint 失败'))
    log.error('加载 Webhook Endpoint 失败:', err)
  } finally {
    loadingEndpoints.value = false
  }
}

async function loadDeliveries() {
  loadingDeliveries.value = true
  try {
    const result = await outboundWebhooksApi.listDeliveries({
      endpoint_id: deliveryEndpointFilter.value === 'all' ? undefined : deliveryEndpointFilter.value,
      status: deliveryStatusFilter.value,
      event_type: deliveryEventFilter.value,
      limit: 50,
    })
    deliveries.value = result.items
  } catch (err) {
    error(parseApiError(err, '加载 Webhook 投递日志失败'))
    log.error('加载 Webhook 投递日志失败:', err)
  } finally {
    loadingDeliveries.value = false
  }
}

function createDefaultEndpointForm(): EndpointForm {
  return {
    name: '',
    url: '',
    enabled: true,
    subscribed_events: ['risk_control.hit', 'provider.error', 'balance.low'],
    secret: '',
    timeout_ms: 5000,
    max_retries: 5,
  }
}

function createDefaultTestForm(endpointId = ''): TestForm {
  return {
    endpoint_id: endpointId,
    event_type: 'webhook.test',
    payload: JSON.stringify({
      source: 'aether',
      event: 'webhook.test',
      message: 'Webhook 连通性测试',
    }, null, 2),
  }
}

function openCreateEndpointDialog() {
  editingEndpointId.value = null
  endpointForm.value = createDefaultEndpointForm()
  showEndpointDialog.value = true
}

function openEditEndpointDialog(endpoint: WebhookEndpoint) {
  editingEndpointId.value = endpoint.id
  endpointForm.value = {
    name: endpoint.name,
    url: endpoint.url,
    enabled: endpoint.enabled,
    subscribed_events: [...endpoint.subscribed_events],
    secret: '',
    timeout_ms: endpoint.timeout_ms,
    max_retries: endpoint.max_retries,
  }
  showEndpointDialog.value = true
}

function openTestDialog(endpoint?: WebhookEndpoint) {
  testForm.value = createDefaultTestForm(endpoint?.id || endpoints.value[0]?.id || '')
  showTestDialog.value = true
}

function openDeliveryDialog(delivery: WebhookDeliveryLog) {
  selectedDelivery.value = delivery
  showDeliveryDialog.value = true
}

function selectAllEvents() {
  endpointForm.value.subscribed_events = subscribableEventDefinitions.map(item => item.type)
}

function selectHighPriorityEvents() {
  endpointForm.value.subscribed_events = subscribableEventDefinitions
    .filter(item => item.high_priority)
    .map(item => item.type)
}

function toggleFormEvent(eventType: WebhookEventType, checked: boolean) {
  const events = new Set(endpointForm.value.subscribed_events)
  if (checked) {
    events.add(eventType)
  } else {
    events.delete(eventType)
  }
  endpointForm.value.subscribed_events = Array.from(events)
}

async function saveEndpoint() {
  const payload = buildEndpointPayload()
  if (!payload) return

  savingEndpoint.value = true
  try {
    if (editingEndpointId.value) {
      const updated = await outboundWebhooksApi.updateEndpoint(editingEndpointId.value, payload)
      replaceEndpoint(updated)
      success('Webhook Endpoint 已更新')
    } else {
      const created = await outboundWebhooksApi.createEndpoint(payload)
      endpoints.value = [created, ...endpoints.value]
      success('Webhook Endpoint 已创建')
    }
    showEndpointDialog.value = false
    void loadDeliveries()
  } catch (err) {
    error(parseApiError(err, '保存 Webhook Endpoint 失败'))
    log.error('保存 Webhook Endpoint 失败:', err)
  } finally {
    savingEndpoint.value = false
  }
}

function buildEndpointPayload(): WebhookEndpointUpsertRequest | null {
  const name = endpointForm.value.name.trim()
  if (!name) {
    error('请填写 Endpoint 名称')
    return null
  }
  if (name.length > 64) {
    error('Endpoint 名称不能超过 64 个字符')
    return null
  }
  const urlError = validateWebhookEndpointUrl(endpointForm.value.url)
  if (urlError) {
    error(urlError)
    return null
  }
  if (endpointForm.value.subscribed_events.length === 0) {
    error('至少选择一个订阅事件')
    return null
  }
  const timeout = Number(endpointForm.value.timeout_ms)
  if (!Number.isFinite(timeout) || timeout < 1000 || timeout > 30000) {
    error('超时时间需在 1000-30000ms 之间')
    return null
  }
  const maxRetries = Number(endpointForm.value.max_retries)
  if (!Number.isInteger(maxRetries) || maxRetries < 0 || maxRetries > 10) {
    error('最大重试次数需在 0-10 之间')
    return null
  }

  return {
    name,
    url: endpointForm.value.url.trim(),
    enabled: endpointForm.value.enabled,
    subscribed_events: endpointForm.value.subscribed_events,
    timeout_ms: timeout,
    max_retries: maxRetries,
    secret: endpointForm.value.secret.trim() || undefined,
  }
}

async function updateEndpointEnabled(endpoint: WebhookEndpoint, enabled: boolean) {
  const originalEnabled = endpoint.enabled
  endpoint.enabled = enabled
  busyEndpointId.value = endpoint.id
  try {
    const updated = await outboundWebhooksApi.updateEndpoint(endpoint.id, { enabled })
    replaceEndpoint(updated)
    success(enabled ? 'Webhook Endpoint 已启用' : 'Webhook Endpoint 已停用')
  } catch (err) {
    endpoint.enabled = originalEnabled
    error(parseApiError(err, '更新 Webhook Endpoint 状态失败'))
    log.error('更新 Webhook Endpoint 状态失败:', err)
  } finally {
    busyEndpointId.value = null
  }
}

async function deleteEndpoint(endpoint: WebhookEndpoint) {
  if (!window.confirm(`确认删除 Webhook Endpoint「${endpoint.name}」？`)) return
  busyEndpointId.value = endpoint.id
  try {
    await outboundWebhooksApi.deleteEndpoint(endpoint.id)
    endpoints.value = endpoints.value.filter(item => item.id !== endpoint.id)
    deliveries.value = deliveries.value.filter(item => item.endpoint_id !== endpoint.id)
    success('Webhook Endpoint 已删除')
  } catch (err) {
    error(parseApiError(err, '删除 Webhook Endpoint 失败'))
    log.error('删除 Webhook Endpoint 失败:', err)
  } finally {
    busyEndpointId.value = null
  }
}

async function sendTestDelivery() {
  let payload: Record<string, unknown> | undefined
  try {
    payload = testForm.value.payload.trim()
      ? JSON.parse(testForm.value.payload) as Record<string, unknown>
      : undefined
  } catch {
    error('测试载荷必须是有效 JSON')
    return
  }

  testingDelivery.value = true
  try {
    const delivery = await outboundWebhooksApi.sendTestDelivery(testForm.value.endpoint_id, {
      event_type: testForm.value.event_type,
      payload,
    })
    upsertDelivery(delivery)
    selectedDelivery.value = delivery
    showDeliveryDialog.value = true
    showTestDialog.value = false
    success('测试投递已提交')
  } catch (err) {
    error(parseApiError(err, '发送 Webhook 测试失败'))
    log.error('发送 Webhook 测试失败:', err)
  } finally {
    testingDelivery.value = false
  }
}

async function retryDelivery(delivery: WebhookDeliveryLog) {
  busyDeliveryId.value = delivery.id
  try {
    const updated = await outboundWebhooksApi.retryDelivery(delivery.id)
    upsertDelivery(updated)
    selectedDelivery.value = updated
    success('Webhook 投递已重新入队')
  } catch (err) {
    error(parseApiError(err, '重试 Webhook 投递失败'))
    log.error('重试 Webhook 投递失败:', err)
  } finally {
    busyDeliveryId.value = null
  }
}

function replaceEndpoint(endpoint: WebhookEndpoint) {
  const index = endpoints.value.findIndex(item => item.id === endpoint.id)
  if (index === -1) {
    endpoints.value = [endpoint, ...endpoints.value]
  } else {
    endpoints.value.splice(index, 1, endpoint)
  }
}

function upsertDelivery(delivery: WebhookDeliveryLog) {
  const index = deliveries.value.findIndex(item => item.id === delivery.id)
  if (index === -1) {
    deliveries.value = [delivery, ...deliveries.value].slice(0, 50)
  } else {
    deliveries.value.splice(index, 1, delivery)
  }
}

function canRetryDelivery(delivery: WebhookDeliveryLog): boolean {
  return delivery.status === 'failed' || delivery.status === 'dead'
}

function eventName(eventType: WebhookEventType): string {
  return WEBHOOK_EVENT_DEFINITIONS.find(item => item.type === eventType)?.name ?? eventType
}

function deliveryStatusLabel(status: WebhookDeliveryStatus): string {
  const labels: Record<WebhookDeliveryStatus, string> = {
    pending: '待投递',
    delivering: '投递中',
    succeeded: '成功',
    failed: '失败',
    retrying: '重试中',
    dead: '已终止',
    cancelled: '已取消',
  }
  return labels[status] || status
}

function deliveryStatusBadgeVariant(status: WebhookDeliveryStatus): BadgeVariant {
  if (status === 'succeeded') return 'success'
  if (status === 'failed' || status === 'dead') return 'destructive'
  if (status === 'retrying' || status === 'pending' || status === 'delivering') return 'warning'
  if (status === 'cancelled') return 'secondary'
  return 'outline'
}

function formatDateTime(value: string | null): string {
  if (!value) return '-'
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return value
  return date.toLocaleString('zh-CN', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

defineExpose({
  loadAll,
  openCreateEndpointDialog,
  openTestDialog,
})
</script>
