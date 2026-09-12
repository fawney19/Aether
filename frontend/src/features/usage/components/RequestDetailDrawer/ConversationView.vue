<template>
  <div :class="{ 'max-h-[500px] overflow-y-auto': !embedded }">
    <div class="flex flex-col gap-2 p-2">
      <!-- 渲染错误提示 -->
      <div
        v-if="renderResult.error"
        class="flex items-center gap-2 p-3 bg-destructive/10 border border-destructive/20 rounded-lg text-destructive text-sm"
      >
        <AlertCircle class="w-4 h-4 shrink-0" />
        <span>{{ renderResult.error }}</span>
      </div>

      <!-- 空内容提示 -->
      <div
        v-else-if="renderResult.blocks.length === 0"
        class="text-sm text-muted-foreground"
      >
        {{ emptyMessage }}
      </div>

      <template v-else-if="embedded">
        <BlockRenderer :blocks="renderResult.blocks" />
        <div
          v-if="renderResult.isStream"
          class="flex items-center gap-1.5 px-3 py-2 text-xs text-muted-foreground bg-muted/30 rounded-lg w-fit"
        >
          <Zap class="w-4 h-4" />
          <span>流式响应</span>
        </div>
      </template>

      <!-- 轮次视图 -->
      <template v-else>
        <!-- System Prompt -->
        <div
          v-if="groupedConversation.system"
          class="rounded-lg border border-dashed border-border bg-card overflow-hidden"
        >
          <div
            class="flex items-center gap-2 px-3 py-2 cursor-pointer hover:bg-muted/30 transition-colors"
            @click="systemExpanded = !systemExpanded"
          >
            <Settings class="w-4 h-4 text-muted-foreground" />
            <span class="text-xs font-medium text-muted-foreground">System</span>
            <span class="text-xs text-muted-foreground">
              ({{ groupedConversation.system.length }} 字符)
            </span>
            <component
              :is="systemExpanded ? ChevronDown : ChevronRight"
              class="w-4 h-4 text-muted-foreground ml-auto"
            />
          </div>
          <div
            v-if="systemExpanded"
            class="p-3 text-sm leading-relaxed"
          >
            <pre class="m-0 whitespace-pre-wrap break-words">{{ groupedConversation.system }}</pre>
          </div>
        </div>

        <!-- 历史轮次（折叠区域） -->
        <div
          v-if="historyTurns.length > 0"
          class="flex flex-col gap-2"
        >
          <!-- 历史轮次标题栏 -->
          <div
            class="flex items-center gap-2 px-3 py-2 cursor-pointer hover:bg-muted/30 rounded-lg border border-dashed border-border transition-colors"
            @click="historyExpanded = !historyExpanded"
          >
            <span class="text-xs font-medium text-muted-foreground">
              历史轮次 ({{ historyTurns.length }})
            </span>
            <component
              :is="historyExpanded ? ChevronDown : ChevronRight"
              class="w-4 h-4 text-muted-foreground ml-auto"
            />
          </div>

          <!-- 历史轮次列表 -->
          <div
            v-if="historyExpanded"
            class="flex flex-col gap-2"
          >
            <TurnCard
              v-for="turn in historyTurns"
              :key="turn.index"
              :turn="turn"
              :expanded="expandedTurns.has(turn.index)"
              @toggle="toggleTurn(turn.index)"
            />
          </div>
        </div>

        <!-- 最新轮次（默认展开，但可折叠） -->
        <template v-if="latestTurns.length > 0">
          <TurnCard
            v-for="turn in latestTurns"
            :key="turn.index"
            :turn="turn"
            :expanded="expandedTurns.has(turn.index)"
            :is-latest="turn.index === groupedConversation.totalTurns"
            @toggle="toggleTurn(turn.index)"
          />
        </template>

        <!-- 流式响应标记 -->
        <div
          v-if="renderResult.isStream"
          class="flex items-center gap-1.5 px-3 py-2 text-xs text-muted-foreground bg-muted/30 rounded-lg w-fit"
        >
          <Zap class="w-4 h-4" />
          <span>流式响应</span>
        </div>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, nextTick } from 'vue'
import { AlertCircle, Zap, Settings, ChevronRight, ChevronDown } from 'lucide-vue-next'
import TurnCard from './TurnCard.vue'
import BlockRenderer from './BlockRenderer.vue'
import type { RenderResult } from '../../conversation'
import { groupRenderBlocksIntoTurns } from '../../conversation/grouper'

const props = defineProps<{
  renderResult: RenderResult
  emptyMessage: string
  embedded?: boolean
}>()

// 状态
const systemExpanded = ref(false)
const historyExpanded = ref(false)
const expandedTurns = ref<Set<number>>(new Set())
const lastRenderResultId = ref<string>('')

// 将渲染结果转换为分组对话
const groupedConversation = computed(() => {
  return groupRenderBlocksIntoTurns(props.renderResult.blocks, props.renderResult.isStream)
})

// 生成 renderResult 的唯一标识
function getRenderResultId(): string {
  return `${props.renderResult.blocks.length}-${props.renderResult.isStream}`
}

// 历史轮次（除最后 1-2 轮外的所有轮次）
const historyTurns = computed(() => {
  const turns = groupedConversation.value.turns
  if (turns.length <= 2) return []
  return turns.slice(0, -2)
})

// 最新轮次（最后 1-2 轮）
const latestTurns = computed(() => {
  const turns = groupedConversation.value.turns
  if (turns.length <= 2) return turns
  return turns.slice(-2)
})

// 切换单个轮次的展开状态
function toggleTurn(index: number) {
  const newSet = new Set(expandedTurns.value)
  if (newSet.has(index)) {
    newSet.delete(index)
  } else {
    newSet.add(index)
  }
  expandedTurns.value = newSet
}

// 初始化最新轮次为展开状态
function initExpandedTurns() {
  const turns = groupedConversation.value.turns
  if (turns.length <= 2) {
    expandedTurns.value = new Set(turns.map(t => t.index))
  } else {
    expandedTurns.value = new Set(turns.slice(-2).map(t => t.index))
  }
}

// 监听 renderResult 变化，重置状态并初始化展开轮次
watch(() => props.renderResult, () => {
  const currentId = getRenderResultId()
  if (currentId !== lastRenderResultId.value) {
    lastRenderResultId.value = currentId
    systemExpanded.value = false
    historyExpanded.value = false
    nextTick(() => {
      initExpandedTurns()
    })
  }
}, { deep: true, immediate: true })
</script>
