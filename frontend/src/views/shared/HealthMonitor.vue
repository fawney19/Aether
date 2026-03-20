<template>
  <div class="space-y-6 pb-8">
    <div class="flex flex-col gap-4 lg:flex-row lg:items-start lg:justify-between">
      <div class="space-y-1">
        <h2 class="text-xl font-semibold tracking-tight">
          健康监控
        </h2>
        <p class="text-sm text-muted-foreground">
          {{ isAdminPage ? '统一查看端点、提供商和模型的运行状态' : '查看当前开放能力的端点监控状态' }}
        </p>
      </div>

      <div class="flex items-center gap-3">
        <Label class="text-xs text-muted-foreground">回溯时间：</Label>
        <Select v-model="lookbackHours">
          <SelectTrigger class="w-28 h-8 text-xs border-border/60">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="1">
              1 小时
            </SelectItem>
            <SelectItem value="6">
              6 小时
            </SelectItem>
            <SelectItem value="12">
              12 小时
            </SelectItem>
            <SelectItem value="24">
              24 小时
            </SelectItem>
            <SelectItem value="48">
              48 小时
            </SelectItem>
          </SelectContent>
        </Select>
      </div>
    </div>

    <Tabs
      v-if="isAdminPage"
      v-model="activeTab"
      class="space-y-4"
    >
      <TabsList class="tabs-button-list grid w-full max-w-[520px] grid-cols-3">
        <TabsTrigger value="endpoint">
          端点监控
        </TabsTrigger>
        <TabsTrigger value="provider">
          提供商监控
        </TabsTrigger>
        <TabsTrigger value="model">
          模型监控
        </TabsTrigger>
      </TabsList>

      <TabsContent value="endpoint">
        <HealthMonitorCard
          title="端点监控"
          :is-admin="true"
          :show-provider-info="true"
          :lookback-hours="resolvedLookbackHours"
          :show-lookback-control="false"
        />
      </TabsContent>

      <TabsContent value="provider">
        <ProviderHealthCard :lookback-hours="resolvedLookbackHours" />
      </TabsContent>

      <TabsContent value="model">
        <ModelHealthCard :lookback-hours="resolvedLookbackHours" />
      </TabsContent>
    </Tabs>

    <HealthMonitorCard
      v-else
      title="端点监控"
      :is-admin="false"
      :show-provider-info="false"
      :lookback-hours="resolvedLookbackHours"
      :show-lookback-control="false"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import { useRoute } from 'vue-router'
import Label from '@/components/ui/label.vue'
import Select from '@/components/ui/select.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import Tabs from '@/components/ui/tabs.vue'
import TabsContent from '@/components/ui/tabs-content.vue'
import TabsList from '@/components/ui/tabs-list.vue'
import TabsTrigger from '@/components/ui/tabs-trigger.vue'
import HealthMonitorCard from '@/features/providers/components/HealthMonitorCard.vue'
import ProviderHealthCard from '@/features/providers/components/ProviderHealthCard.vue'
import ModelHealthCard from '@/features/providers/components/ModelHealthCard.vue'

const route = useRoute()
const isAdminPage = computed(() => route.path.startsWith('/admin'))
const activeTab = ref('endpoint')
const lookbackHours = ref('6')
const resolvedLookbackHours = computed(() => parseInt(lookbackHours.value))
</script>
