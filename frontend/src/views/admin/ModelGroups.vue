<template>
  <div class="space-y-4 pb-6 sm:space-y-5 sm:pb-8">
    <div class="grid gap-4 lg:grid-cols-[280px_minmax(0,1fr)] lg:gap-5 xl:grid-cols-[300px_minmax(0,1fr)] xl:gap-6">
      <Card
        variant="default"
        class="overflow-hidden"
      >
        <div class="border-b border-border/60 px-3.5 py-3 sm:px-4 sm:py-4">
          <div class="flex items-center justify-between gap-3">
            <div>
              <h3 class="text-base font-semibold">
                模型分组
              </h3>
              <p class="mt-0.5 hidden text-[11px] text-muted-foreground sm:block">
                独立维护模型成员、路由优先级和用户计费倍率。
              </p>
            </div>
            <Button
              class="h-8 shrink-0 px-3"
              @click="openCreateDialog"
            >
              <Plus class="mr-1.5 h-3.5 w-3.5" />
              新增
            </Button>
          </div>

          <div class="relative mt-3">
            <Search class="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              v-model="searchQuery"
              type="text"
              placeholder="搜索模型分组..."
              class="h-9 pl-8 text-sm"
            />
          </div>
        </div>

        <div class="p-2.5 sm:p-3">
          <div
            v-if="filteredGroups.length === 0"
            class="rounded-xl border border-dashed border-border/60 px-4 py-8 text-center text-sm text-muted-foreground"
          >
            {{ searchQuery ? '没有匹配的模型分组' : '还没有模型分组' }}
          </div>

          <div
            v-else
            class="flex gap-2 overflow-x-auto pb-1 sm:max-h-[68vh] sm:flex-col sm:overflow-y-auto sm:pb-0"
          >
            <button
              v-for="group in filteredGroups"
              :key="group.id"
              type="button"
              class="relative w-full min-w-[220px] shrink-0 overflow-hidden rounded-xl border px-3 py-2.5 text-left transition-all sm:min-w-0 sm:px-4 sm:py-3"
              :class="selectedGroupId === group.id
                ? 'border-border bg-muted/35 shadow-sm'
                : 'border-border/60 bg-card hover:border-primary/30 hover:bg-muted/30'"
              @click="selectedGroupId = group.id"
            >
              <span
                v-if="selectedGroupId === group.id"
                class="absolute inset-y-2 left-0 w-1 rounded-r-full bg-primary"
              />
              <div class="flex items-start justify-between gap-2">
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <span class="truncate text-sm font-semibold text-foreground">
                      {{ group.display_name }}
                    </span>
                    <Badge
                      v-if="group.is_default"
                      variant="outline"
                      class="h-5 shrink-0 px-1.5 py-0 text-[10px] font-medium"
                    >
                      默认
                    </Badge>
                    <Badge
                      v-if="!group.is_active"
                      variant="outline"
                      class="h-5 shrink-0 px-1.5 py-0 text-[10px] font-medium text-muted-foreground"
                    >
                      停用
                    </Badge>
                  </div>
                  <p class="mt-1 truncate text-[11px] text-muted-foreground">
                    {{ group.name }}
                  </p>
                  <p class="mt-1 line-clamp-1 text-[11px] text-muted-foreground sm:line-clamp-2 sm:text-xs">
                    {{ group.description || '未填写模型分组说明' }}
                  </p>
                </div>
              </div>
            </button>
          </div>
        </div>
      </Card>

      <div class="space-y-4 sm:space-y-5 lg:space-y-6">
        <Card
          variant="default"
          class="overflow-hidden"
        >
          <div
            v-if="selectedGroup"
            class="space-y-4 px-3.5 py-3.5 sm:px-5 sm:py-4"
          >
            <div class="flex flex-col gap-3 lg:flex-row lg:items-start lg:justify-between">
              <div class="min-w-0">
                <div class="flex items-center gap-2">
                  <h2 class="truncate text-lg font-semibold text-foreground">
                    {{ selectedGroup.display_name }}
                  </h2>
                  <Badge
                    v-if="selectedGroup.is_default"
                    variant="outline"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    默认分组
                  </Badge>
                  <Badge
                    v-if="!selectedGroup.is_active"
                    variant="outline"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium text-muted-foreground"
                  >
                    已停用
                  </Badge>
                </div>
                <p class="mt-1 text-sm text-muted-foreground">
                  {{ selectedGroup.name }}
                </p>
                <p class="mt-1 text-sm text-muted-foreground">
                  {{ selectedGroup.description || '未填写模型分组说明' }}
                </p>
              </div>

              <div class="grid grid-cols-3 gap-2 sm:flex sm:flex-wrap sm:items-center">
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 w-full px-3 text-xs sm:w-auto"
                  @click="goToModelsPage"
                >
                  模型页
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 w-full px-3 text-xs sm:w-auto"
                  @click="editGroup(selectedGroup)"
                >
                  <SquarePen class="mr-1.5 h-3.5 w-3.5" />
                  编辑
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 w-full border-rose-200 px-3 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40 sm:w-auto"
                  :disabled="selectedGroup.is_default"
                  @click="removeGroup(selectedGroup)"
                >
                  <Trash2 class="mr-1.5 h-3.5 w-3.5" />
                  删除
                </Button>
              </div>
            </div>

            <div class="grid gap-2 sm:grid-cols-2 xl:grid-cols-4">
              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <Layers3 class="h-4 w-4" />
                  <span class="text-xs font-medium">模型数量</span>
                </div>
                <div class="mt-2 text-xl font-semibold text-foreground sm:text-2xl">
                  {{ selectedGroup.model_count }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <UsersIcon class="h-4 w-4" />
                  <span class="text-xs font-medium">关联用户组</span>
                </div>
                <div class="mt-2 text-xl font-semibold text-foreground sm:text-2xl">
                  {{ selectedGroup.user_group_count }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <Network class="h-4 w-4" />
                  <span class="text-xs font-medium">路由模式</span>
                </div>
                <div class="mt-2 text-sm font-medium text-foreground">
                  {{ selectedGroup.routing_mode === 'custom' ? '自定义渠道路由' : '继承全局路由' }}
                </div>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/30 p-3">
                <div class="flex items-center gap-2 text-muted-foreground">
                  <Percent class="h-4 w-4" />
                  <span class="text-xs font-medium">默认计费倍率</span>
                </div>
                <div class="mt-2 text-sm font-medium text-foreground">
                  {{ formatMultiplier(selectedGroup.default_user_billing_multiplier) }}
                </div>
              </div>
            </div>
          </div>

          <div
            v-else
            class="px-5 py-9 text-center sm:px-6 sm:py-12"
          >
            <div class="mx-auto flex h-12 w-12 items-center justify-center rounded-full bg-primary/10">
              <Layers3 class="h-5 w-5 text-primary" />
            </div>
            <h3 class="mt-4 text-base font-semibold text-foreground">
              选择一个模型分组
            </h3>
            <p class="mt-2 text-sm text-muted-foreground">
              左侧选择现有模型分组，或先创建一个新分组来配置模型成员和渠道策略。
            </p>
          </div>
        </Card>

        <Card
          v-if="selectedGroup"
          variant="default"
          class="overflow-hidden"
        >
          <div class="border-b border-border/60 px-3.5 py-3.5 sm:px-5 sm:py-4">
            <div class="flex items-center gap-2">
              <Layers3 class="h-4 w-4 text-muted-foreground" />
              <h3 class="text-base font-semibold">
                模型成员
              </h3>
            </div>
          </div>
          <div class="px-3.5 py-3.5 sm:px-5 sm:py-4">
            <div
              v-if="selectedGroup.models.length === 0"
              class="rounded-xl border border-dashed border-border/60 px-4 py-8 text-center text-sm text-muted-foreground"
            >
              这个模型分组还没有包含任何统一模型。
            </div>
            <div
              v-else
              class="flex flex-wrap gap-2"
            >
              <Badge
                v-for="model in selectedGroup.models"
                :key="model.global_model_id"
                variant="outline"
                class="h-7 gap-1.5 px-2.5 text-xs"
              >
                <span>{{ model.model_display_name }}</span>
                <span class="text-muted-foreground">{{ model.model_name }}</span>
              </Badge>
            </div>
          </div>
        </Card>

        <Card
          v-if="selectedGroup"
          variant="default"
          class="overflow-hidden"
        >
          <div class="border-b border-border/60 px-3.5 py-3.5 sm:px-5 sm:py-4">
            <div class="flex items-center gap-2">
              <RouteIcon class="h-4 w-4 text-muted-foreground" />
              <h3 class="text-base font-semibold">
                渠道路由
              </h3>
            </div>
          </div>

          <div class="px-3.5 py-3.5 sm:px-5 sm:py-4">
            <div
              v-if="selectedGroup.routing_mode !== 'custom'"
              class="rounded-xl border border-dashed border-border/60 px-4 py-8 text-center text-sm text-muted-foreground"
            >
              当前继承全局路由，未启用分组级渠道覆盖。
            </div>

            <div
              v-else-if="selectedGroup.routes.length === 0"
              class="rounded-xl border border-dashed border-border/60 px-4 py-8 text-center text-sm text-muted-foreground"
            >
              当前已切换到自定义路由，但还没有配置渠道规则。
            </div>

            <div
              v-else
              class="hidden overflow-x-auto xl:block"
            >
              <Table>
                <TableHeader>
                  <TableRow class="border-b border-border/60 hover:bg-transparent">
                    <TableHead class="w-[220px]">
                      Provider
                    </TableHead>
                    <TableHead class="w-[180px]">
                      供应商 Key
                    </TableHead>
                    <TableHead class="w-[120px]">
                      优先级
                    </TableHead>
                    <TableHead class="w-[140px]">
                      计费倍率
                    </TableHead>
                    <TableHead class="w-[120px]">
                      状态
                    </TableHead>
                    <TableHead>
                      备注
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  <TableRow
                    v-for="route in selectedGroup.routes"
                    :key="route.id"
                    class="border-b border-border/40"
                  >
                    <TableCell class="font-medium">
                      {{ route.provider_name || route.provider_id }}
                    </TableCell>
                    <TableCell>
                      {{ formatRouteKey(route) }}
                    </TableCell>
                    <TableCell>
                      {{ route.priority }}
                    </TableCell>
                    <TableCell>
                      {{ route.user_billing_multiplier_override == null ? '跟随分组默认' : formatMultiplier(route.user_billing_multiplier_override) }}
                    </TableCell>
                    <TableCell>
                      <Badge :variant="route.is_active ? 'default' : 'secondary'">
                        {{ route.is_active ? '启用' : '停用' }}
                      </Badge>
                    </TableCell>
                    <TableCell class="text-muted-foreground">
                      {{ route.notes || '-' }}
                    </TableCell>
                  </TableRow>
                </TableBody>
              </Table>
            </div>

            <div class="space-y-2.5 xl:hidden">
              <div
                v-for="route in selectedGroup.routes"
                :key="route.id"
                class="rounded-xl border border-border/60 bg-card p-3"
              >
                <div class="flex items-center justify-between gap-2">
                  <div class="text-sm font-semibold text-foreground">
                    {{ route.provider_name || route.provider_id }}
                  </div>
                  <Badge :variant="route.is_active ? 'default' : 'secondary'">
                    {{ route.is_active ? '启用' : '停用' }}
                  </Badge>
                </div>
                <div class="mt-2 grid grid-cols-2 gap-2 text-[11px]">
                  <div class="rounded-lg border border-border/50 bg-muted/20 px-2.5 py-2">
                    <div class="text-muted-foreground">
                      供应商 Key
                    </div>
                    <div class="mt-1 truncate font-medium text-foreground">
                      {{ formatRouteKey(route) }}
                    </div>
                  </div>
                  <div class="rounded-lg border border-border/50 bg-muted/20 px-2.5 py-2">
                    <div class="text-muted-foreground">
                      优先级
                    </div>
                    <div class="mt-1 truncate font-medium text-foreground">
                      {{ route.priority }}
                    </div>
                  </div>
                  <div class="rounded-lg border border-border/50 bg-muted/20 px-2.5 py-2">
                    <div class="text-muted-foreground">
                      计费倍率
                    </div>
                    <div class="mt-1 truncate font-medium text-foreground">
                      {{ route.user_billing_multiplier_override == null ? '跟随分组默认' : formatMultiplier(route.user_billing_multiplier_override) }}
                    </div>
                  </div>
                  <div class="rounded-lg border border-border/50 bg-muted/20 px-2.5 py-2">
                    <div class="text-muted-foreground">
                      备注
                    </div>
                    <div class="mt-1 truncate font-medium text-foreground">
                      {{ route.notes || '-' }}
                    </div>
                  </div>
                </div>
              </div>
            </div>
          </div>
        </Card>
      </div>
    </div>

    <ModelGroupFormDialog
      ref="formDialogRef"
      :open="showFormDialog"
      :group="editingGroup"
      @close="closeDialog"
      @submit="handleSubmit"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import {
  Layers3,
  Network,
  Percent,
  Plus,
  Route as RouteIcon,
  Search,
  SquarePen,
  Trash2,
  Users as UsersIcon,
} from 'lucide-vue-next'
import {
  Badge,
  Button,
  Card,
  Input,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui'
import {
  modelGroupsApi,
  type ModelGroupDetail,
  type ModelGroupSummary,
} from '@/api/model-groups'
import ModelGroupFormDialog, {
  type ModelGroupFormData,
} from '@/features/models/components/ModelGroupFormDialog.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'

const router = useRouter()
const { confirmDanger } = useConfirm()
const { success, error } = useToast()

const searchQuery = ref('')
const groups = ref<ModelGroupSummary[]>([])
const selectedGroupId = ref<string | null>(null)
const selectedGroup = ref<ModelGroupDetail | null>(null)
const showFormDialog = ref(false)
const editingGroup = ref<ModelGroupDetail | null>(null)
const formDialogRef = ref<InstanceType<typeof ModelGroupFormDialog> | null>(null)

const filteredGroups = computed(() => {
  const keyword = searchQuery.value.trim().toLowerCase()
  if (!keyword) return groups.value
  return groups.value.filter((group) => {
    const searchable = `${group.name} ${group.display_name} ${group.description || ''}`.toLowerCase()
    return searchable.includes(keyword)
  })
})

function formatMultiplier(value: number): string {
  return `${value.toFixed(2)}x`
}

function formatRouteKey(route: ModelGroupDetail['routes'][number]): string {
  if (route.provider_api_key_name) return route.provider_api_key_name
  if (route.provider_api_key_id) return route.provider_api_key_id
  return '全部 Key'
}

async function loadGroups() {
  const items = await modelGroupsApi.list()
  groups.value = items
  if (!items.length) {
    selectedGroupId.value = null
    selectedGroup.value = null
    return
  }
  if (!selectedGroupId.value || !items.some((item) => item.id === selectedGroupId.value)) {
    selectedGroupId.value = items[0].id
  }
}

async function loadSelectedGroup(groupId: string | null) {
  if (!groupId) {
    selectedGroup.value = null
    return
  }
  try {
    selectedGroup.value = await modelGroupsApi.get(groupId)
  } catch (err) {
    error(parseApiError(err, '未知错误'), '加载模型分组详情失败')
  }
}

async function refreshData() {
  await loadGroups()
  await loadSelectedGroup(selectedGroupId.value)
}

function openCreateDialog() {
  editingGroup.value = null
  showFormDialog.value = true
}

function editGroup(group: ModelGroupDetail) {
  editingGroup.value = group
  showFormDialog.value = true
}

function closeDialog() {
  showFormDialog.value = false
  editingGroup.value = null
}

async function handleSubmit(data: ModelGroupFormData) {
  formDialogRef.value?.setSaving(true)
  try {
    if (data.id) {
      const updated = await modelGroupsApi.update(data.id, {
        name: data.name,
        display_name: data.display_name,
        description: data.description ?? null,
        default_user_billing_multiplier: data.default_user_billing_multiplier,
        routing_mode: data.routing_mode,
        is_active: data.is_active,
        sort_order: data.sort_order,
        model_ids: data.model_ids ?? [],
        routes: data.routes ?? [],
      })
      success('模型分组已更新')
      selectedGroupId.value = updated.id
    } else {
      const created = await modelGroupsApi.create({
        name: data.name,
        display_name: data.display_name,
        description: data.description ?? null,
        default_user_billing_multiplier: data.default_user_billing_multiplier,
        routing_mode: data.routing_mode,
        is_active: data.is_active,
        sort_order: data.sort_order,
        model_ids: data.model_ids ?? [],
        routes: data.routes ?? [],
      })
      success('模型分组创建成功')
      selectedGroupId.value = created.id
    }

    await refreshData()
    closeDialog()
  } catch (err) {
    error(parseApiError(err, '未知错误'), data.id ? '更新模型分组失败' : '创建模型分组失败')
  } finally {
    formDialogRef.value?.setSaving(false)
  }
}

async function removeGroup(group: ModelGroupDetail) {
  if (group.is_default) {
    error('默认模型分组不能删除', '操作不支持')
    return
  }

  const confirmed = await confirmDanger(
    `确定要删除模型分组 ${group.display_name} 吗？`,
    '删除模型分组',
    '删除',
  )
  if (!confirmed) return

  try {
    await modelGroupsApi.delete(group.id)
    success('模型分组已删除')
    if (selectedGroupId.value === group.id) {
      selectedGroupId.value = null
    }
    await refreshData()
  } catch (err) {
    error(parseApiError(err, '未知错误'), '删除模型分组失败')
  }
}

function goToModelsPage() {
  void router.push('/admin/models')
}

watch(selectedGroupId, (value) => {
  void loadSelectedGroup(value)
})

watch(searchQuery, () => {
  if (selectedGroupId.value && filteredGroups.value.some((group) => group.id === selectedGroupId.value)) {
    return
  }
  selectedGroupId.value = filteredGroups.value[0]?.id ?? null
})

onMounted(async () => {
  await refreshData()
})
</script>
