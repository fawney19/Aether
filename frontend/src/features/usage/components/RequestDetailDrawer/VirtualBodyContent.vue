<template>
  <div
    ref="viewport"
    class="virtual-body-scroll"
    tabindex="0"
    :aria-busy="loading"
    @scroll.passive="readScrollPosition"
  >
    <div
      aria-hidden="true"
      :style="{ height: `${offsets[visibleStart]}px` }"
    />
    <div
      v-for="index in visibleIndexes"
      :key="index"
      :ref="element => setChunkElement(index, element)"
      :data-body-chunk="index"
      :style="{ minHeight: chunks.has(index) ? (chunks.get(index)!.hasNext ? '250px' : undefined) : `${heights[index]}px` }"
    >
      <slot
        v-if="chunks.has(index)"
        :chunk="chunks.get(index)!"
        :index="index"
      />
      <div
        v-else
        class="p-4 text-sm text-muted-foreground"
        role="status"
      >
        {{ failed ? '正文视图加载失败' : '正在后台准备正文视图…' }}
      </div>
    </div>
    <div
      aria-hidden="true"
      :style="{ height: `${offsets[heights.length] - offsets[visibleEnd]}px` }"
    />
  </div>
</template>

<script setup lang="ts" generic="Chunk extends { hasNext: boolean }">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch, type ComponentPublicInstance } from 'vue'

const props = withDefaults(defineProps<{
  loadChunk: (index: number) => Chunk | Promise<Chunk>
  estimatedHeight?: number
}>(), { estimatedHeight: 1000 })
const emit = defineEmits<{ 'load-error': [error: unknown] }>()

const viewport = ref<HTMLElement | null>(null)
const scrollTop = ref(0)
const viewportHeight = ref(500)
const heights = shallowRef([props.estimatedHeight])
const chunks = shallowRef(new Map<number, Chunk>())
const loading = ref(false)
const failed = ref(false)
const elements = new Map<number, HTMLElement>()
let observer: ResizeObserver | undefined
let generation = 0
let disposed = false

const offsets = computed(() => {
  const positions = [0]
  for (const height of heights.value) positions.push(positions[positions.length - 1] + height)
  return positions
})

function indexAt(position: number) {
  let lower = 0
  let upper = heights.value.length
  while (lower < upper) {
    const middle = Math.floor((lower + upper) / 2)
    if (offsets.value[middle + 1] <= position) lower = middle + 1
    else upper = middle
  }
  return Math.min(lower, heights.value.length - 1)
}

const visibleStart = computed(() => indexAt(Math.max(0, scrollTop.value - 250)))
const visibleEnd = computed(() => Math.min(
  heights.value.length,
  visibleStart.value + 4,
  indexAt(scrollTop.value + viewportHeight.value + 250) + 1,
))
const visibleIndexes = computed(() => Array.from(
  { length: visibleEnd.value - visibleStart.value },
  (_value, offset) => visibleStart.value + offset,
))

function readScrollPosition() {
  if (!viewport.value) return
  scrollTop.value = viewport.value.scrollTop
  viewportHeight.value = viewport.value.clientHeight || 500
}

function measureChunks() {
  if (!viewport.value || disposed) return
  const updated = [...heights.value]
  let adjustment = 0
  let changed = false
  for (const [index, element] of elements) {
    if (!chunks.value.has(index) || index >= updated.length) continue
    const height = element.getBoundingClientRect().height
    if (height <= 0 || Math.abs(height - updated[index]) < 0.5) continue
    if (offsets.value[index + 1] <= viewport.value.scrollTop) adjustment += height - updated[index]
    updated[index] = height
    changed = true
  }
  if (changed) {
    heights.value = updated
    if (adjustment) viewport.value.scrollTop += adjustment
  }
  readScrollPosition()
}

function setChunkElement(index: number, element: Element | ComponentPublicInstance | null) {
  const previous = elements.get(index)
  if (previous) observer?.unobserve(previous)
  if (element instanceof HTMLElement) {
    elements.set(index, element)
    observer?.observe(element)
  } else {
    elements.delete(index)
  }
}

function evictDistantChunks() {
  const retained = new Map(chunks.value)
  for (const index of retained.keys()) {
    if (index < visibleStart.value - 1 || index > visibleEnd.value) retained.delete(index)
  }
  if (retained.size !== chunks.value.size) chunks.value = retained
}

async function loadVisibleChunks() {
  if (loading.value || disposed || failed.value) return
  loading.value = true
  try {
    while (!disposed && !failed.value) {
      const index = visibleIndexes.value.find(candidate => !chunks.value.has(candidate))
      if (index === undefined) break
      const currentGeneration = generation
      try {
        const result = await props.loadChunk(index)
        if (disposed) return
        if (currentGeneration !== generation) continue
        const updated = [...heights.value]
        if (result.hasNext && index === updated.length - 1) updated.push(props.estimatedHeight)
        if (!result.hasNext) updated.length = index + 1
        heights.value = updated
        chunks.value = new Map(chunks.value).set(index, result)
        evictDistantChunks()
        await nextTick()
        measureChunks()
      } catch (error) {
        if (disposed) return
        if (currentGeneration !== generation) continue
        failed.value = true
        emit('load-error', error)
      }
    }
  } finally {
    loading.value = false
  }
}

function refresh(index: number, resetTail = false) {
  generation += 1
  failed.value = false
  const retained = new Map(chunks.value)
  for (const candidate of retained.keys()) {
    if (candidate === index || (resetTail && candidate > index)) retained.delete(candidate)
  }
  chunks.value = retained
  if (resetTail) heights.value = heights.value.slice(0, index + 1)
  void loadVisibleChunks()
}

watch([visibleStart, visibleEnd], () => {
  evictDistantChunks()
  void loadVisibleChunks()
}, { immediate: true })

onMounted(() => {
  if (typeof ResizeObserver !== 'undefined') {
    observer = new ResizeObserver(measureChunks)
    if (viewport.value) observer.observe(viewport.value)
    for (const element of elements.values()) observer.observe(element)
  }
  window.addEventListener('resize', measureChunks)
  measureChunks()
})

onBeforeUnmount(() => {
  disposed = true
  generation += 1
  observer?.disconnect()
  elements.clear()
  window.removeEventListener('resize', measureChunks)
})

defineExpose({ refresh })
</script>

<style scoped>
.virtual-body-scroll {
  max-height: 500px;
  overflow: auto;
  overflow-anchor: none;
}
</style>
