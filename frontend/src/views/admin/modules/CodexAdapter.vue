<template>
  <PageContainer>
    <PageHeader
      title="Codex 适配器"
      description="按 Codex 请求模型配置全局模型调度。"
      :icon="Route"
    >
      <template #actions>
        <Button
          variant="outline"
          :disabled="loading || saving"
          @click="loadPage"
        >
          <RefreshCw
            class="mr-2 h-4 w-4"
            :class="{ 'animate-spin': loading }"
          />
          刷新
        </Button>
        <Button
          :disabled="loading || saving || !hasChanges"
          @click="saveConfig"
        >
          {{ saving ? '保存中...' : '保存配置' }}
        </Button>
      </template>
    </PageHeader>

    <div class="mt-6 grid gap-4 xl:grid-cols-[280px_minmax(0,1fr)]">
      <RouteListPane
        :global-model-count="globalModels.length"
        :has-changes="hasChanges"
        :loading="loading"
        :routes="routes"
        :selected-index="selectedRouteIndex"
        :status-text="statusText"
        @add="addRoute"
        @select="selectRoute"
      />

      <RouteEditorPane
        :global-model-options="selectedRouteGlobalModelOptions"
        :has-catalog-global-models="globalModels.length > 0"
        :route="selectedRoute"
        :route-index="selectedRouteIndex"
        :scheduling-mode-help="codexAdapterSchedulingModeHelp"
        :scheduling-mode-options="codexAdapterSchedulingModeOptions"
        @add-candidate="addCandidate"
        @move-candidate="moveCandidate"
        @remove-candidate="removeCandidate"
        @remove-route="removeRoute"
        @update-candidate="updateCandidate"
        @update-route="updateRoute"
      />
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { RefreshCw, Route } from 'lucide-vue-next'
import { PageContainer, PageHeader } from '@/components/layout'
import { Button } from '@/components/ui'
import RouteEditorPane from './codex-adapter/RouteEditorPane.vue'
import RouteListPane from './codex-adapter/RouteListPane.vue'
import {
  codexAdapterSchedulingModeHelp,
  codexAdapterSchedulingModeOptions,
} from './codex-adapter/schedulingModes'
import {
  useCodexAdapterConfig,
} from './codex-adapter/useCodexAdapterConfig'

const {
  addCandidate,
  addRoute,
  globalModels,
  hasChanges,
  loadPage,
  loading,
  moveCandidate,
  removeCandidate,
  removeRoute,
  routes,
  saveConfig,
  saving,
  selectRoute,
  selectedRoute,
  selectedRouteGlobalModelOptions,
  selectedRouteIndex,
  statusText,
  updateCandidate,
  updateRoute,
} = useCodexAdapterConfig()
</script>
