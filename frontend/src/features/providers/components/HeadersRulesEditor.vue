<template>
  <div class="space-y-4">
    <!-- 操作按钮 -->
    <div class="flex flex-wrap gap-2">
      <Button variant="outline" size="sm" @click="openAddRuleDialog">
        <Plus class="mr-2 h-4 w-4" />
        新增 Header
      </Button>
      <Button variant="outline" size="sm" @click="openRemoveRuleDialog">
        <Trash2 class="mr-2 h-4 w-4" />
        删除 Header
      </Button>
      <Button variant="outline" size="sm" @click="openRenameRuleDialog">
        <Edit3 class="mr-2 h-4 w-4" />
        重命名 Header
      </Button>
      <Button variant="outline" size="sm" @click="openReplaceValueRuleDialog">
        <Replace class="mr-2 h-4 w-4" />
        替换 Header 值
      </Button>

      <Button
        v-if="hasRules"
        variant="ghost"
        size="sm"
        @click="clearAllRules"
      >
        清空所有
      </Button>
    </div>

    <!-- 规则列表 -->
    <div v-if="hasRules" class="space-y-3">
      <!-- Add Rules -->
      <div
        v-if="localRules.add && Object.keys(localRules.add).length > 0"
        class="rounded-lg border bg-muted/50 p-3"
      >
        <div class="mb-2 flex items-center gap-2">
          <Plus class="h-4 w-4 text-green-600" />
          <span class="text-sm font-medium">新增 Headers ({{ Object.keys(localRules.add).length }})</span>
        </div>
        <div class="space-y-1">
          <div
            v-for="[key, value] in Object.entries(localRules.add)"
            :key="`add-${key}`"
            class="flex items-center justify-between rounded bg-background px-2 py-1 text-sm"
          >
            <div class="flex flex-col">
              <span class="font-medium">{{ key }}</span>
              <span class="text-muted-foreground">{{ value }}</span>
            </div>
            <Button
              variant="ghost"
              size="icon"
              class="h-6 w-6"
              @click="removeRule('add', key)"
            >
              <X class="h-3 w-3" />
            </Button>
          </div>
        </div>
      </div>

      <!-- Remove Rules -->
      <div
        v-if="localRules.remove && localRules.remove.length > 0"
        class="rounded-lg border bg-muted/50 p-3"
      >
        <div class="mb-2 flex items-center gap-2">
          <Trash2 class="h-4 w-4 text-red-600" />
          <span class="text-sm font-medium">删除 Headers ({{ localRules.remove.length }})</span>
        </div>
        <div class="flex flex-wrap gap-2">
          <Badge
            v-for="(key, idx) in localRules.remove"
            :key="`remove-${idx}`"
            variant="secondary"
            class="flex items-center gap-1"
          >
            {{ key }}
            <button
              class="ml-1 hover:text-destructive"
              @click="removeRule('remove', idx)"
            >
              <X class="h-3 w-3" />
            </button>
          </Badge>
        </div>
      </div>

      <!-- Rename Rules -->
      <div
        v-if="localRules.replace_name && Object.keys(localRules.replace_name).length > 0"
        class="rounded-lg border bg-muted/50 p-3"
      >
        <div class="mb-2 flex items-center gap-2">
          <Edit3 class="h-4 w-4 text-blue-600" />
          <span class="text-sm font-medium">重命名 Headers ({{ Object.keys(localRules.replace_name).length }})</span>
        </div>
        <div class="space-y-1">
          <div
            v-for="[oldName, newName] in Object.entries(localRules.replace_name)"
            :key="`rename-${oldName}`"
            class="flex items-center justify-between rounded bg-background px-2 py-1 text-sm"
          >
            <div class="flex items-center gap-2">
              <span class="font-medium">{{ oldName }}</span>
              <ArrowRight class="h-3 w-3 text-muted-foreground" />
              <span class="text-muted-foreground">{{ newName }}</span>
            </div>
            <Button
              variant="ghost"
              size="icon"
              class="h-6 w-6"
              @click="removeRule('replace_name', oldName)"
            >
              <X class="h-3 w-3" />
            </Button>
          </div>
        </div>
      </div>

      <!-- Replace Value Rules -->
      <div
        v-if="localRules.replace_value && Object.keys(localRules.replace_value).length > 0"
        class="rounded-lg border bg-muted/50 p-3"
      >
        <div class="mb-2 flex items-center gap-2">
          <Replace class="h-4 w-4 text-orange-600" />
          <span class="text-sm font-medium">替换 Header 值 ({{ Object.keys(localRules.replace_value).length }})</span>
        </div>
        <div class="space-y-1">
          <div
            v-for="[headerName, rule] in Object.entries(localRules.replace_value)"
            :key="`replace-${headerName}`"
            class="flex items-center justify-between rounded bg-background px-2 py-1 text-sm"
          >
            <div class="flex flex-col">
              <span class="font-medium">{{ headerName }}</span>
              <span class="text-xs text-muted-foreground">
                {{ rule.regex ? '正则' : '字符串' }}: {{ rule.search }} → {{ rule.replace }}
              </span>
            </div>
            <Button
              variant="ghost"
              size="icon"
              class="h-6 w-6"
              @click="removeRule('replace_value', headerName)"
            >
              <X class="h-3 w-3" />
            </Button>
          </div>
        </div>
      </div>
    </div>

    <!-- 空状态 -->
    <div
      v-else
      class="flex flex-col items-center justify-center rounded-lg border border-dashed py-8 text-center"
    >
      <Settings class="mb-2 h-8 w-8 text-muted-foreground" />
      <p class="text-sm text-muted-foreground">
        暂无 Headers 规则
      </p>
      <p class="text-xs text-muted-foreground">
        点击上方按钮开始配置
      </p>
    </div>

    <!-- 添加规则对话框 -->
    <Dialog
      :model-value="showAddRuleDialog"
      @update:model-value="showAddRuleDialog = $event"
      title="新增 Header"
      :icon="Plus"
    >
      <div class="space-y-4">
        <p class="text-sm text-muted-foreground">
          添加固定的 header 到请求中（不会覆盖已存在的 header）
        </p>

        <div class="space-y-2">
          <Label for="add-header-name">Header 名称</Label>
          <Input
            id="add-header-name"
            v-model="addRuleForm.name"
            placeholder="X-Custom-Header"
          />
        </div>
        <div class="space-y-2">
          <Label for="add-header-value">Header 值</Label>
          <Input
            id="add-header-value"
            v-model="addRuleForm.value"
            placeholder="custom-value"
          />
        </div>
      </div>

      <template #footer>
        <Button variant="outline" @click="showAddRuleDialog = false">
          取消
        </Button>
        <Button @click="confirmAddRule">
          添加
        </Button>
      </template>
    </Dialog>

    <!-- 删除规则对话框 -->
    <Dialog
      :model-value="showRemoveRuleDialog"
      @update:model-value="showRemoveRuleDialog = $event"
      title="删除 Header"
      :icon="Trash2"
    >
      <div class="space-y-4">
        <p class="text-sm text-muted-foreground">
          删除指定的 headers（大小写不敏感）
        </p>

        <div class="space-y-2">
          <Label for="remove-header-name">Header 名称</Label>
          <Input
            id="remove-header-name"
            v-model="removeRuleForm.name"
            placeholder="User-Agent"
          />
        </div>
      </div>

      <template #footer>
        <Button variant="outline" @click="showRemoveRuleDialog = false">
          取消
        </Button>
        <Button @click="confirmRemoveRule">
          添加
        </Button>
      </template>
    </Dialog>

    <!-- 重命名规则对话框 -->
    <Dialog
      :model-value="showRenameRuleDialog"
      @update:model-value="showRenameRuleDialog = $event"
      title="重命名 Header"
      :icon="Edit3"
    >
      <div class="space-y-4">
        <p class="text-sm text-muted-foreground">
          将一个 header 的名称改为另一个（大小写不敏感）
        </p>

        <div class="space-y-2">
          <Label for="rename-old-name">原 Header 名称</Label>
          <Input
            id="rename-old-name"
            v-model="renameRuleForm.oldName"
            placeholder="X-Old-Name"
          />
        </div>
        <div class="space-y-2">
          <Label for="rename-new-name">新 Header 名称</Label>
          <Input
            id="rename-new-name"
            v-model="renameRuleForm.newName"
            placeholder="X-New-Name"
          />
        </div>
      </div>

      <template #footer>
        <Button variant="outline" @click="showRenameRuleDialog = false">
          取消
        </Button>
        <Button @click="confirmRenameRule">
          添加
        </Button>
      </template>
    </Dialog>

    <!-- 替换值规则对话框 -->
    <Dialog
      :model-value="showReplaceValueRuleDialog"
      @update:model-value="showReplaceValueRuleDialog = $event"
      title="替换 Header 值"
      :icon="Replace"
    >
      <div class="space-y-4">
        <p class="text-sm text-muted-foreground">
          替换 header 的值，支持普通字符串和正则表达式
        </p>

        <div class="space-y-2">
          <Label for="replace-header-name">Header 名称</Label>
          <Input
            id="replace-header-name"
            v-model="replaceValueRuleForm.headerName"
            placeholder="User-Agent"
          />
        </div>
        <div class="space-y-2">
          <Label for="replace-search">搜索值</Label>
          <Input
            id="replace-search"
            v-model="replaceValueRuleForm.search"
            placeholder="要搜索的值"
          />
        </div>
        <div class="space-y-2">
          <Label for="replace-replace">替换为</Label>
          <Input
            id="replace-replace"
            v-model="replaceValueRuleForm.replace"
            placeholder="替换后的值"
          />
        </div>
        <div class="flex items-center space-x-2">
          <Switch
            id="replace-regex"
            v-model="replaceValueRuleForm.regex"
          />
          <Label for="replace-regex" class="cursor-pointer">
            使用正则表达式
          </Label>
        </div>
        <div v-if="!replaceValueRuleForm.regex" class="flex items-center space-x-2">
          <Switch
            id="replace-case"
            v-model="replaceValueRuleForm.caseSensitive"
          />
          <Label for="replace-case" class="cursor-pointer">
            区分大小写
          </Label>
        </div>
      </div>

      <template #footer>
        <Button variant="outline" @click="showReplaceValueRuleDialog = false">
          取消
        </Button>
        <Button @click="confirmReplaceValueRule">
          添加
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch } from 'vue'
import { Dialog } from '@/components/ui'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Badge from '@/components/ui/badge.vue'
import Switch from '@/components/ui/switch.vue'
import {
  Plus,
  Trash2,
  Edit3,
  Replace,
  X,
  ArrowRight,
  Settings,
} from 'lucide-vue-next'

type HeaderRules = {
  add?: Record<string, string>
  remove?: string[]
  replace_name?: Record<string, string>
  replace_value?: Record<string, {
    search: string
    replace: string
    regex: boolean
    case_sensitive?: boolean
  }>
}

const props = defineProps<{
  modelValue: HeaderRules | null
}>()

const emit = defineEmits<{
  'update:modelValue': [value: HeaderRules | null]
}>()

const localRules = ref<HeaderRules>({})

// 同步 props 到 localRules
watch(
  () => props.modelValue,
  (newValue) => {
    localRules.value = newValue || {}
  },
  { immediate: true }
)

// 同步 localRules 到 props
watch(localRules, (newValue) => {
  const hasRules = Object.keys(newValue).some(key => {
    const value = newValue[key as keyof HeaderRules]
    if (Array.isArray(value)) return value.length > 0
    if (typeof value === 'object') return Object.keys(value).length > 0
    return false
  })

  emit('update:modelValue', hasRules ? newValue : null)
}, { deep: true })

const hasRules = computed(() => {
  return Object.keys(localRules.value).some(key => {
    const value = localRules.value[key as keyof HeaderRules]
    if (Array.isArray(value)) return value.length > 0
    if (typeof value === 'object') return Object.keys(value).length > 0
    return false
  })
})

// Dialog states
const showAddRuleDialog = ref(false)
const showRemoveRuleDialog = ref(false)
const showRenameRuleDialog = ref(false)
const showReplaceValueRuleDialog = ref(false)

// Form states
const addRuleForm = ref({ name: '', value: '' })
const removeRuleForm = ref({ name: '' })
const renameRuleForm = ref({ oldName: '', newName: '' })
const replaceValueRuleForm = ref({
  headerName: '',
  search: '',
  replace: '',
  regex: false,
  caseSensitive: true,
})

// Open dialogs
function openAddRuleDialog() {
  addRuleForm.value = { name: '', value: '' }
  showAddRuleDialog.value = true
}

function openRemoveRuleDialog() {
  removeRuleForm.value = { name: '' }
  showRemoveRuleDialog.value = true
}

function openRenameRuleDialog() {
  renameRuleForm.value = { oldName: '', newName: '' }
  showRenameRuleDialog.value = true
}

function openReplaceValueRuleDialog() {
  replaceValueRuleForm.value = {
    headerName: '',
    search: '',
    replace: '',
    regex: false,
    caseSensitive: true,
  }
  showReplaceValueRuleDialog.value = true
}

// Confirm actions
function confirmAddRule() {
  if (!addRuleForm.value.name || !addRuleForm.value.value) return

  if (!localRules.value.add) {
    localRules.value.add = {}
  }
  localRules.value.add[addRuleForm.value.name] = addRuleForm.value.value
  showAddRuleDialog.value = false
}

function confirmRemoveRule() {
  if (!removeRuleForm.value.name) return

  if (!localRules.value.remove) {
    localRules.value.remove = []
  }
  localRules.value.remove.push(removeRuleForm.value.name)
  showRemoveRuleDialog.value = false
}

function confirmRenameRule() {
  if (!renameRuleForm.value.oldName || !renameRuleForm.value.newName) return

  if (!localRules.value.replace_name) {
    localRules.value.replace_name = {}
  }
  localRules.value.replace_name[renameRuleForm.value.oldName] = renameRuleForm.value.newName
  showRenameRuleDialog.value = false
}

function confirmReplaceValueRule() {
  if (!replaceValueRuleForm.value.headerName ||
      !replaceValueRuleForm.value.search ||
      !replaceValueRuleForm.value.replace) return

  if (!localRules.value.replace_value) {
    localRules.value.replace_value = {}
  }
  localRules.value.replace_value[replaceValueRuleForm.value.headerName] = {
    search: replaceValueRuleForm.value.search,
    replace: replaceValueRuleForm.value.replace,
    regex: replaceValueRuleForm.value.regex,
    case_sensitive: replaceValueRuleForm.value.caseSensitive,
  }
  showReplaceValueRuleDialog.value = false
}

// Remove rule
function removeRule(type: string, key: string | number) {
  if (type === 'add') {
    delete localRules.value.add![key as string]
  } else if (type === 'remove') {
    localRules.value.remove!.splice(Number(key), 1)
  } else if (type === 'replace_name') {
    delete localRules.value.replace_name![key as string]
  } else if (type === 'replace_value') {
    delete localRules.value.replace_value![key as string]
  }
}

// Clear all rules
function clearAllRules() {
  localRules.value = {}
}
</script>
