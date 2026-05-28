<template>
  <PageContainer
    max-width="full"
    padding="none"
  >
    <div class="external-integration-workspace -mx-4 -mt-4 px-2 pb-2 pt-2 sm:-mx-6 sm:px-3 lg:-mx-8 lg:-mt-6 lg:px-3">
      <div
        v-if="loading"
        class="overflow-hidden rounded-lg border border-border bg-card"
      >
        <div class="relative h-[calc(100dvh-7rem)] min-h-[420px] animate-pulse bg-muted/60 lg:h-[calc(100dvh-6.5rem)]" />
      </div>

      <div
        v-else-if="!currentItem || !enabled"
        class="rounded-lg border border-dashed border-border bg-card px-6 py-14 text-center"
      >
        <ExternalLink class="mx-auto h-8 w-8 text-muted-foreground" />
        <p class="mt-3 text-sm font-medium text-foreground">
          外部系统入口不可用
        </p>
      </div>

      <div
        v-else-if="currentItem.open_mode === 'new_tab'"
        class="rounded-lg border border-border bg-card p-6"
      >
        <div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div class="min-w-0">
            <p class="truncate text-sm font-medium text-foreground">
              {{ currentItem.name }}
            </p>
            <p class="truncate text-xs text-muted-foreground">
              {{ currentItem.url }}
            </p>
          </div>
          <Button @click="openExternal(currentItem.url)">
            <ExternalLink class="mr-2 h-4 w-4" />
            打开
          </Button>
        </div>
      </div>

      <div
        v-else
        class="relative overflow-hidden rounded-lg border border-border bg-card"
      >
        <div class="absolute right-3 top-3 z-10 flex items-center gap-2">
          <Button
            variant="outline"
            size="icon"
            class="h-9 w-9 border-border/70 bg-background/85 backdrop-blur"
            title="刷新"
            :disabled="loading"
            @click="loadItems"
          >
            <RefreshCw
              class="h-4 w-4"
              :class="{ 'animate-spin': loading }"
            />
          </Button>
          <Button
            variant="outline"
            size="icon"
            class="h-9 w-9 border-border/70 bg-background/85 backdrop-blur"
            title="新窗口打开"
            @click="openExternal(currentItem.url)"
          >
            <ExternalLink class="h-4 w-4" />
          </Button>
        </div>
        <iframe
          :src="currentItem.url"
          class="block h-[calc(100dvh-7rem)] min-h-[420px] w-full bg-background lg:h-[calc(100dvh-6.5rem)]"
          sandbox="allow-forms allow-popups allow-popups-to-escape-sandbox allow-same-origin allow-scripts"
          referrerpolicy="no-referrer"
        />
      </div>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useRoute } from 'vue-router'
import { ExternalLink, RefreshCw } from 'lucide-vue-next'
import { PageContainer } from '@/components/layout'
import Button from '@/components/ui/button.vue'
import { modulesApi, type ExternalIntegrationItem } from '@/api/modules'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

const route = useRoute()
const { error } = useToast()

const loading = ref(false)
const enabled = ref(false)
const items = ref<ExternalIntegrationItem[]>([])

const currentId = computed(() => String(route.params.id || ''))
const currentItem = computed(() => items.value.find(item => item.id === currentId.value) ?? null)

onMounted(() => {
  void loadItems()
})

watch(currentId, () => {
  void loadItems()
})

async function loadItems() {
  loading.value = true
  try {
    const payload = await modulesApi.getVisibleExternalIntegrations()
    enabled.value = payload.enabled
    items.value = payload.items
  } catch (err) {
    enabled.value = false
    items.value = []
    error(parseApiError(err, '加载外部系统入口失败'))
    log.error('加载外部系统入口失败:', err)
  } finally {
    loading.value = false
  }
}

function openExternal(url: string) {
  window.open(url, '_blank', 'noopener,noreferrer')
}
</script>
