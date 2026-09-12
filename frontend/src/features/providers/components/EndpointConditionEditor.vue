<template>
  <div>
    <!-- Group node -->
    <div
      v-if="modelValue.kind === 'group'"
      class="rounded-md border p-2"
      :class="[
        nested ? 'bg-muted/30 border-dashed' : 'bg-muted/10 border-border',
      ]"
    >
      <!-- Group header: mode selector + actions -->
      <div class="flex items-center gap-1.5 mb-2">
        <Button
          v-if="!nested"
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-muted-foreground"
          title="转回单条件"
          @click="convertToLeaf"
        >
          <ListFilter class="w-3.5 h-3.5" />
        </Button>
        <Select
          :model-value="modelValue.mode"
          @update:model-value="(value: string) => updateGroupMode(value as ConditionGroupMode)"
        >
          <SelectTrigger class="w-[86px] h-6 text-xs font-medium shrink-0">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="all">
              AND
            </SelectItem>
            <SelectItem value="any">
              OR
            </SelectItem>
          </SelectContent>
        </Select>
        <div class="flex-1" />
        <Button
          variant="ghost"
          size="sm"
          class="h-6 px-1.5 text-xs text-muted-foreground"
          title="添加条件"
          @click="addLeafChild"
        >
          <Plus class="w-3 h-3 mr-0.5" />
          条件
        </Button>
        <Button
          v-if="!nested"
          variant="ghost"
          size="sm"
          class="h-6 px-1.5 text-xs text-muted-foreground"
          @click="addGroupChild(modelValue.mode === 'all' ? 'any' : 'all')"
        >
          <Plus class="w-3 h-3 mr-0.5" />
          子组
        </Button>
        <Button
          v-if="removable && nested"
          variant="ghost"
          size="icon"
          class="h-6 w-6 shrink-0 text-muted-foreground hover:text-destructive"
          @click="emit('remove')"
        >
          <X class="w-3 h-3" />
        </Button>
      </div>

      <!-- Children with logic connector labels between them -->
      <div class="space-y-0">
        <template
          v-for="(child, index) in modelValue.children"
          :key="index"
        >
          <!-- Logic connector label between children -->
          <div
            v-if="index > 0"
            class="flex items-center gap-2 py-0.5 pl-2"
          >
            <span
              class="text-[10px] font-semibold px-1.5 py-0.5 rounded"
              :class="modelValue.mode === 'all'
                ? 'bg-blue-500/15 text-blue-600 dark:text-blue-400'
                : 'bg-amber-500/15 text-amber-600 dark:text-amber-400'"
            >
              {{ modelValue.mode === 'all' ? 'AND' : 'OR' }}
            </span>
            <div class="flex-1 border-t border-dashed border-muted-foreground/20" />
          </div>

          <EndpointConditionEditor
            :model-value="child"
            :path-hint="pathHint"
            nested
            removable
            @update:model-value="(next) => updateChild(index, next)"
            @remove="removeChild(index)"
          />
        </template>
      </div>
    </div>

    <!-- Leaf node -->
    <div
      v-else
      class="flex flex-wrap items-center gap-1.5 rounded-md px-2 py-1.5"
      :class="nested ? 'bg-background/60' : 'bg-muted/10 border border-border'"
    >
      <!-- Toggle to group mode (icon button at the start, top-level only) -->
      <Button
        v-if="!nested"
        variant="ghost"
        size="icon"
        class="h-7 w-7 shrink-0 text-muted-foreground"
        title="转为组合条件 (AND/OR)"
        @click="convertToGroup('all')"
      >
        <ListFilter class="w-3.5 h-3.5" />
      </Button>
      <Select
        :model-value="modelValue.source"
        @update:model-value="(value: string) => updateLeafField('source', value)"
      >
        <SelectTrigger class="w-[96px] h-7 text-xs shrink-0">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="body">
            请求体
          </SelectItem>
          <SelectItem value="original">
            原始请求体
          </SelectItem>
          <SelectItem value="request_headers">
            请求头
          </SelectItem>
        </SelectContent>
      </Select>
      <Input
        :model-value="modelValue.path"
        :placeholder="modelValue.source === 'request_headers' ? 'Header 名称' : (pathHint || '字段路径')"
        size="sm"
        class="flex-1 min-w-[120px] h-7 text-xs"
        @update:model-value="(value) => updateLeafField('path', value)"
      />
      <Select
        :model-value="modelValue.op"
        @update:model-value="(value: string) => updateLeafField('op', value)"
      >
        <SelectTrigger class="w-[110px] h-7 text-xs shrink-0">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem
            v-for="option in CONDITION_OP_OPTIONS"
            :key="option.value"
            :value="option.value"
          >
            {{ option.label }}
          </SelectItem>
        </SelectContent>
      </Select>
      <Input
        v-if="isConditionValueRequired(modelValue.op)"
        :model-value="modelValue.value"
        :placeholder="getConditionValuePlaceholder(modelValue.op)"
        size="sm"
        class="flex-1 min-w-[120px] h-7 text-xs"
        @update:model-value="(value) => updateLeafField('value', value)"
      />
      <Button
        v-if="removable"
        variant="ghost"
        size="icon"
        class="h-7 w-7 shrink-0 text-muted-foreground hover:text-destructive"
        @click="emit('remove')"
      >
        <X class="w-3 h-3" />
      </Button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { Button, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui'
import { ListFilter, Plus, X } from 'lucide-vue-next'

import type { BodyRuleConditionOp } from '@/api/endpoints'
import {
  CONDITION_OP_OPTIONS,
  cloneEditableCondition,
  createConditionGroup,
  createEmptyConditionLeaf,
  getConditionValuePlaceholder,
  isConditionValueRequired,
  type ConditionGroupMode,
  type ConditionSource,
  type EditableConditionGroup,
  type EditableConditionLeaf,
  type EditableConditionNode,
} from './endpoint-rule-condition'

const props = withDefaults(defineProps<{
  modelValue: EditableConditionNode
  pathHint?: string
  nested?: boolean
  removable?: boolean
}>(), {
  pathHint: '',
  nested: false,
  removable: false,
})

const emit = defineEmits<{
  'update:modelValue': [value: EditableConditionNode]
  remove: []
}>()

defineOptions({ name: 'EndpointConditionEditor' })

function updateLeafField(field: keyof EditableConditionLeaf, rawValue: string): void {
  if (props.modelValue.kind !== 'leaf') return

  const next = cloneEditableCondition(props.modelValue) as EditableConditionLeaf

  if (field === 'op') {
    next.op = rawValue as BodyRuleConditionOp
    if (!isConditionValueRequired(next.op)) next.value = ''
  } else if (field === 'source') {
    next.source = rawValue as ConditionSource
  } else if (field === 'path' || field === 'value') {
    next[field] = rawValue
    if (field === 'value') next.retainValue = false
  }

  emit('update:modelValue', next)
}

function updateGroupMode(mode: ConditionGroupMode): void {
  if (props.modelValue.kind !== 'group') return

  const next = cloneEditableCondition(props.modelValue) as EditableConditionGroup
  next.mode = mode
  emit('update:modelValue', next)
}

function convertToLeaf(): void {
  emit('update:modelValue', createEmptyConditionLeaf())
}

function convertToGroup(mode: ConditionGroupMode): void {
  const seed = props.modelValue.kind === 'leaf'
    ? cloneEditableCondition(props.modelValue)
    : createEmptyConditionLeaf()
  emit('update:modelValue', createConditionGroup(mode, [seed]))
}

function addLeafChild(): void {
  if (props.modelValue.kind !== 'group') return

  const next = cloneEditableCondition(props.modelValue) as EditableConditionGroup
  next.children.push(createEmptyConditionLeaf())
  emit('update:modelValue', next)
}

function addGroupChild(mode: ConditionGroupMode): void {
  if (props.modelValue.kind !== 'group') return

  const next = cloneEditableCondition(props.modelValue) as EditableConditionGroup
  next.children.push(createConditionGroup(mode))
  emit('update:modelValue', next)
}

function updateChild(index: number, child: EditableConditionNode): void {
  if (props.modelValue.kind !== 'group') return

  const next = cloneEditableCondition(props.modelValue) as EditableConditionGroup
  next.children[index] = child
  emit('update:modelValue', next)
}

function removeChild(index: number): void {
  if (props.modelValue.kind !== 'group') return

  const next = cloneEditableCondition(props.modelValue) as EditableConditionGroup
  next.children.splice(index, 1)

  // When all children are removed, if top-level group, emit remove to let parent clear the condition
  if (next.children.length === 0) {
    if (!props.nested) {
      emit('remove')
    } else {
      next.children.push(createEmptyConditionLeaf())
      emit('update:modelValue', next)
    }
    return
  }

  emit('update:modelValue', next)
}
</script>
