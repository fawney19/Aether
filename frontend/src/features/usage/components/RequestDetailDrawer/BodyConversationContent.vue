<template>
  <div>
    <div
      v-if="!bodyDocument"
      class="text-sm text-muted-foreground"
    >
      {{ emptyMessage }}
    </div>
    <VirtualBodyContent
      v-else
      :key="viewRevision"
      ref="viewer"
      :load-chunk="loadChunk"
      :estimated-height="1000"
      @load-error="emit('load-error', $event)"
    >
      <template #default="{ chunk, index }">
        <ConversationView
          :render-result="chunk.result"
          :empty-message="emptyMessage"
          embedded
        />
        <div
          v-if="chunk.truncated"
          class="px-3 py-2 text-xs text-muted-foreground"
        >
          长内容仅显示预览，复制仍保留完整内容。
          <button
            v-if="(previewLimits[index] ?? 64_000) < 1_024_000"
            type="button"
            class="text-primary hover:underline"
            @click="showMore(index)"
          >
            显示更多
          </button>
        </div>
      </template>
    </VirtualBodyContent>
  </div>
</template>

<script setup lang="ts">
import { ref, watch } from 'vue'
import ConversationView from './ConversationView.vue'
import VirtualBodyContent from './VirtualBodyContent.vue'
import type { BodyDocument } from '../../utils/body-document'
import type { BodyConversationPage } from '../../utils/body-document-protocol'

const props = defineProps<{
  bodyDocument: BodyDocument | null
  kind: 'request' | 'response'
  apiFormat?: string
  emptyMessage: string
}>()
const emit = defineEmits<{ 'load-error': [error: unknown] }>()
const viewer = ref<{ refresh: (index: number) => void } | null>(null)
const viewRevision = ref(0)
const previewLimits = ref<Record<number, number>>({})

function loadChunk(index: number): Promise<BodyConversationPage> {
  const document = props.bodyDocument
  if (!document) return Promise.resolve({ result: { blocks: [] }, hasNext: false, truncated: false })
  return document.conversation({ kind: props.kind, apiFormat: props.apiFormat, page: index, previewLimit: previewLimits.value[index] ?? 64_000 })
}

function showMore(index: number) {
  previewLimits.value = { ...previewLimits.value, [index]: (previewLimits.value[index] ?? 64_000) + 64_000 }
  viewer.value?.refresh(index)
}

watch([() => props.bodyDocument, () => props.kind, () => props.apiFormat], () => {
  viewRevision.value += 1
  previewLimits.value = {}
})
</script>
