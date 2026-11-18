<template>
  <section
    class="min-w-0"
    :aria-label="t('ui.providerSchedulingWorkspace')"
  >
    <div
      class="grid min-w-0 items-stretch transition-[grid-template-columns,gap] duration-200 ease-out motion-reduce:transition-none"
      :class="panelCollapsed ? 'gap-0 xl:grid-cols-[0px_minmax(0,1fr)]' : 'gap-4 xl:grid-cols-[400px_minmax(0,1fr)]'"
    >
      <Transition
        name="policy-panel"
        @before-enter="setPanelHeight($event, true)"
        @enter="setPanelHeight($event, false)"
        @before-leave="setPanelHeight($event, false)"
        @leave="setPanelHeight($event, true)"
        @after-enter="clearPanelHeight"
        @after-leave="clearPanelHeight"
        @enter-cancelled="clearPanelHeight"
        @leave-cancelled="clearPanelHeight"
      >
        <aside
          v-show="!panelCollapsed"
          id="provider-policy-editor"
          class="flex min-w-0 flex-col gap-3 xl:min-h-0 xl:[contain:size]"
          :aria-label="t('ui.policyGroupSettings')"
          :inert="panelCollapsed"
        >
          <Card
            class="flex min-w-0 flex-1 flex-col xl:h-0 xl:min-h-full"
            :aria-label="t('ui.policyGroupsAndSchedulingConfiguration')"
          >
            <div class="flex shrink-0 items-center justify-between gap-2 border-b border-border/50 px-3 py-3 sm:py-3.5">
              <h3 class="sr-only">
                {{ t('ui.policyGroups') }}
              </h3>
              <Select
                :model-value="selectedValue"
                :disabled="busy"
                @update:model-value="changeGroup"
              >
                <SelectTrigger
                  class="h-8 min-w-0 flex-1 px-3 text-sm"
                  :title="draft?.name || t('ui.selectPolicyGroup')"
                  :aria-label="t('ui.currentSchedulingPolicy')"
                >
                  <span class="min-w-0 truncate">
                    <SelectValue :placeholder="loading ? t('ui.loadingGroups') : t('ui.selectPolicyGroup')" />
                  </span>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem
                    v-if="isNewDraft"
                    value="new"
                  >
                    {{ t('ui.createPolicy') }}
                  </SelectItem>
                  <SelectItem
                    v-for="group in groups"
                    :key="group.id"
                    :value="group.id"
                  >
                    {{ group.name }}{{ group.is_system_default ? " " + t('ui.defaulte99c9aa') : '' }}{{ !group.enabled ? " " + t('ui.disabled9868d7d') : '' }}
                  </SelectItem>
                </SelectContent>
              </Select>
              <div class="flex shrink-0 items-center gap-0.5">
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :disabled="busy || isNewDraft"
                  :title="t('ui.createGroup')"
                  :aria-label="t('ui.createPolicy')"
                  @click="openCreate"
                >
                  <Plus class="h-3.5 w-3.5" />
                </Button>
                <Button
                  v-if="draft?.id"
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8 text-muted-foreground hover:text-destructive"
                  :disabled="busy"
                  :title="t('ui.deletePolicyGroup')"
                  :aria-label="t('ui.deletePolicy')"
                  @click="deleteDialogOpen = true"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
                <Button
                  v-if="draft"
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :class="{ 'text-primary': draft.is_system_default }"
                  :disabled="busy"
                  :title="draft.is_system_default ? t('ui.removeAsSystemDefaultTakesEffectAfterSaving') : t('ui.setAsSystemDefaultTakesEffectAfterSaving')"
                  :aria-label="t('ui.setAsSystemDefault')"
                  :aria-pressed="draft.is_system_default"
                  @click="draft.is_system_default = !draft.is_system_default"
                >
                  <Star
                    class="h-3.5 w-3.5"
                    :class="{ 'fill-current': draft.is_system_default }"
                  />
                </Button>
                <Button
                  v-if="draft"
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :class="{ 'text-primary': draftDirty }"
                  :disabled="!canSaveDraft"
                  :title="saving ? t('ui.saving6bdb443') : billingMultiplierError ?? (!routingSchedulingValid ? t('ui.selectTheApplicableModelsFirst') : draftDirty ? t('ui.saveChanges') : t('ui.saved1bd91a7'))"
                  :aria-label="t('ui.saveScheduling')"
                  :aria-busy="saving"
                  @click="saveDraft"
                >
                  <Save
                    class="h-3.5 w-3.5"
                    :class="{ 'animate-pulse': saving }"
                  />
                </Button>
              </div>
            </div>
            <div class="min-h-0 flex-1 xl:overflow-y-auto">
              <div
                v-if="draft"
                ref="groupMetadata"
                class="min-w-0 space-y-2 border-b border-border/50 p-3"
                :aria-label="t('ui.groupInformation')"
                :inert="busy"
              >
                <div class="flex min-w-0 items-center gap-2">
                  <label class="block min-w-0 flex-1">
                    <span class="sr-only">{{ t('ui.policyName') }}</span>
                    <Input
                      v-model="draft.name"
                      size="sm"
                      class="min-w-0"
                      :aria-label="t('ui.policyName')"
                      :placeholder="t('ui.groupName')"
                      :disabled="busy"
                    />
                  </label>
                  <label class="flex shrink-0 items-center gap-1 text-xs">
                    <span>{{ t('ui.enablef4f0ead') }}</span>
                    <Switch
                      v-model="draft.enabled"
                      :disabled="busy"
                      :aria-label="t('ui.enablePolicy')"
                    />
                  </label>
                </div>
                <div class="flex min-w-0 items-center justify-between gap-3">
                  <label class="flex min-w-0 items-center gap-2 text-xs">
                    <span class="shrink-0">{{ t('ui.groupMultiplier') }}</span>
                    <Input
                      :model-value="billingMultiplierInput"
                      type="number"
                      min="0"
                      step="any"
                      size="sm"
                      class="w-24 min-w-0"
                      :aria-label="t('ui.groupMultiplier')"
                      :aria-invalid="Boolean(billingMultiplierError)"
                      :aria-describedby="billingMultiplierError ? 'group-billing-multiplier-error' : undefined"
                      :disabled="busy"
                      @update:model-value="updateBillingMultiplier"
                    />
                    <span class="shrink-0 text-muted-foreground">{{ t('ui.multiplierSuffix') }}</span>
                  </label>
                  <label class="flex shrink-0 items-center gap-1 text-xs">
                    <span>{{ t('ui.visibleToUsers') }}</span>
                    <Switch
                      v-model="draft.config_json.user_visible"
                      :disabled="busy"
                      :aria-label="t('ui.visibleToUsers')"
                    />
                  </label>
                </div>
                <p
                  v-if="billingMultiplierError"
                  id="group-billing-multiplier-error"
                  class="text-xs text-destructive"
                >
                  {{ billingMultiplierError }}
                </p>
              </div>
              <RoutingSchedulingPolicyEditor
                v-if="draft"
                :key="draftGeneration"
                ref="routingSchedulingPolicyEditor"
                :config="draft.config_json"
                :initial-selection="initialSchedulingSelection ?? undefined"
                :refresh-revision="providerRevision"
                :global-models="globalModels"
                :loading-models="loadingGlobalModels"
                :models-error="globalModelsError"
                :disabled="busy"
                :aria-busy="saving"
                class="p-3"
                :aria-label="t('ui.policySchedulingConfiguration')"
                layout="config-only"
                sidebar
                @update:config="updateDraftConfig"
                @validity-change="routingSchedulingValid = $event"
                @selection-change="activePolicy = $event"
                @reload-models="loadGlobalModels()"
                @inspect-provider="emit('inspect-provider', $event)"
              />

              <div
                v-if="draft"
                class="min-w-0"
                :inert="busy"
              >
                <section
                  class="min-w-0 border-t border-border/50"
                  :inert="busy"
                >
                  <button
                    type="button"
                    class="flex w-full items-center justify-between gap-3 px-3 py-3 text-left"
                    :aria-expanded="advancedOpen"
                    :aria-label="t('ui.advancedSettings')"
                    @click="advancedOpen = !advancedOpen"
                  >
                    <span class="text-sm font-medium">{{ t('ui.advancedSettings') }}</span>
                    <ChevronRight
                      class="h-4 w-4 shrink-0 text-muted-foreground transition-transform"
                      :class="{ 'rotate-90': advancedOpen }"
                    />
                  </button>
                  <div
                    v-show="advancedOpen"
                    class="min-w-0 space-y-4 px-3 pb-3"
                  >
                    <section class="min-w-0 space-y-2">
                      <div>
                        <h3 class="text-sm font-medium">
                          {{ t('ui.systemConfiguration') }}
                        </h3>
                      </div>
                      <div class="grid min-w-0 grid-cols-1 gap-2">
                        <div
                          class="order-1 flex min-w-0 min-h-10 items-center justify-between gap-2 rounded-md border border-border/60 px-2 py-2 text-xs"
                          data-testid="keep-priority-on-conversion"
                        >
                          <div class="flex min-w-0 items-center gap-1.5">
                            <span class="font-medium">{{ t('ui.keepPriorityOnFormatConversion') }}</span>
                            <HelpHint
                              portal
                              :label="t('ui.keepPriorityOnFormatConversion')"
                              :text="t('ui.whenEnabledCandidatesRequiringApiFormatConversionAreNotRanked')"
                            />
                          </div>
                          <Switch
                            :model-value="keepPriorityOnConversion"
                            :disabled="saving"
                            :aria-label="t('ui.keepPriorityOnFormatConversion')"
                            @update:model-value="updateKeepPriorityOnConversion"
                          />
                        </div>
                        <div
                          class="order-1 flex min-w-0 min-h-10 items-center justify-between gap-2 rounded-md border border-border/60 px-2 py-2 text-xs"
                          data-testid="strict-format-conversion"
                        >
                          <div class="flex min-w-0 items-center gap-1.5">
                            <span class="font-medium">{{ t('ui.strictFormatConversion') }}</span>
                            <HelpHint
                              portal
                              :label="t('ui.strictFormatConversion')"
                              :text="t('ui.strictFormatConversionHelp')"
                            />
                          </div>
                          <Switch
                            :model-value="draft.config_json.default_policy.strict_format_conversion"
                            :disabled="saving"
                            :aria-label="t('ui.strictFormatConversion')"
                            @update:model-value="updateExecutionPolicy('strict_format_conversion', $event)"
                          />
                        </div>
                        <div
                          class="order-3 flex min-w-0 min-h-10 items-center justify-between gap-2 rounded-md border border-border/60 px-2 py-2 text-xs"
                          data-testid="cf-heartbeat"
                        >
                          <div class="flex min-w-0 items-center gap-1.5">
                            <span class="font-medium">{{ t('ui.cfKeepalive') }}</span>
                            <HelpHint
                              portal
                              :label="t('ui.cfKeepalive')"
                              :text="t('ui.forSynchronousImageGenerationAndStandardNonStreamingTextFailures')"
                            />
                          </div>
                          <Switch
                            :model-value="cfHeartbeat"
                            :disabled="saving"
                            :aria-label="t('ui.cfKeepalive')"
                            @update:model-value="updateExecutionPolicy('enable_cf_heartbeat', $event)"
                          />
                        </div>
                        <div
                          class="order-2 flex min-w-0 min-h-10 items-center justify-between gap-2 rounded-md border border-border/60 px-2 py-2 text-xs"
                          data-testid="cyber-continue-failover"
                        >
                          <div class="flex min-w-0 items-center gap-1.5">
                            <span class="font-medium">{{ t('ui.failOverOnCyberErrors') }}</span>
                            <HelpHint
                              portal
                              :label="t('ui.failOverOnCyberErrors')"
                              :text="t('ui.continueFailoverIfACyberPolicyErrorOccursBeforeThe')"
                            />
                          </div>
                          <Switch
                            :model-value="cyberContinueFailover"
                            :disabled="saving"
                            :aria-label="t('ui.failOverOnCyberErrors')"
                            @update:model-value="updateExecutionPolicy('cyber_continue_failover', $event)"
                          />
                        </div>
                        <div
                          class="order-5 flex min-w-0 min-h-10 items-center justify-between gap-2 rounded-md border border-border/60 px-2 py-2 text-xs"
                          data-testid="cancel-on-client-disconnect"
                        >
                          <div class="flex min-w-0 items-center gap-1.5">
                            <span class="font-medium">{{ t('ui.abortImmediatelyOnClientCancellation') }}</span>
                            <HelpHint
                              portal
                              :label="t('ui.abortImmediatelyOnClientCancellation')"
                              :text="t('ui.offByDefaultAfterClientCancellationOrDisconnectionTheServer')"
                            />
                          </div>
                          <Switch
                            :model-value="cancelOnClientDisconnect"
                            :disabled="saving"
                            :aria-label="t('ui.abortImmediatelyOnClientCancellation')"
                            @update:model-value="updateExecutionPolicy('cancel_on_client_disconnect', $event)"
                          />
                        </div>
                      </div>
                    </section>
                  </div>
                </section>
                <section class="min-w-0 border-t border-border/50">
                  <button
                    type="button"
                    class="flex w-full items-center justify-between gap-3 px-3 py-3 text-left"
                    :aria-expanded="failoverOpen"
                    :aria-label="t('ui.failover')"
                    @click="failoverOpen = !failoverOpen"
                  >
                    <span class="text-sm font-medium">{{ t('ui.failover') }}</span>
                    <ChevronRight
                      class="h-4 w-4 shrink-0 text-muted-foreground transition-transform"
                      :class="{ 'rotate-90': failoverOpen }"
                    />
                  </button>
                  <div
                    v-show="failoverOpen"
                    class="min-w-0 px-3 pb-3"
                  >
                    <RoutingFailoverPolicyEditor
                      :key="draftGeneration"
                      ref="routingFailoverPolicyEditor"
                      :model-value="draft.config_json.default_policy"
                      :disabled="saving"
                      sidebar
                      @update:model-value="updateRoutingFailoverPolicy"
                      @pending-change="routingFailoverPending = $event"
                    >
                      <template #limits-extra>
                        <label
                          class="min-w-0 space-y-1.5 text-xs"
                          data-testid="sticky-key-attempts"
                        >
                          <span>{{ t('ui.errorRetryCount') }}</span>
                          <Input
                            :model-value="invalidStickyKeyInput ?? stickyKeyAttempts"
                            type="number"
                            min="0"
                            :max="MAX_STICKY_KEY_ATTEMPTS"
                            :aria-invalid="invalidStickyKeyInput !== null"
                            :aria-describedby="invalidStickyKeyInput !== null ? 'sticky-key-attempts-error' : undefined"
                            :disabled="saving"
                            :aria-label="t('ui.errorRetryCount')"
                            @update:model-value="updateStickyKeyAttempts"
                          />
                          <span v-if="invalidStickyKeyInput !== null" id="sticky-key-attempts-error" class="text-destructive" role="alert">
                            {{ t('ui.invalidStickyKeyAttempts') }}
                          </span>
                        </label>
                      </template>
                    </RoutingFailoverPolicyEditor>
                  </div>
                </section>
              </div>
              <div
                v-if="saveConflict"
                role="alert"
                class="m-3 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-500/30 bg-amber-500/5 p-3 text-sm"
              >
                <span>{{ t('ui.theGroupWasUpdatedByAnotherOperationYourChangesHave') }}</span>
                <Button
                  variant="outline"
                  size="sm"
                  :disabled="busy"
                  :aria-label="t('ui.reloadGroup')"
                  @click="reloadCurrentGroup"
                >
                  {{ t('ui.reloadGroup') }}
                </Button>
              </div>
              <div
                v-if="!draft"
                class="p-8 text-center"
              >
                <p class="text-sm text-muted-foreground">
                  {{ emptyMessage }}
                </p>
                <Button
                  v-if="!loading"
                  variant="outline"
                  class="mt-4"
                  @click="loadingError ? refreshGroups() : groups.length ? openGroup(defaultGroupId) : openCreate()"
                >
                  {{ loadingError ? t('ui.reload') : groups.length ? t('ui.openDefaultGroup') : t('ui.createPolicyGroup') }}
                </Button>
              </div>
            </div>
          </Card>
        </aside>
      </Transition>
      <div
        class="min-w-0 xl:col-start-2"
        :aria-label="t('ui.providerDirectory')"
      >
        <slot
          :panel-collapsed="panelCollapsed"
          :toggle-panel="togglePanel"
        />
      </div>
    </div>
    <AlertDialog
      v-model="deleteDialogOpen"
      type="destructive"
      :title="t('ui.deleteRoutingPolicy')"
      :description="t('ui.deleteSchedulingPolicyValueThisCannotBeUndone', { name: draft?.name ?? '' })"
      :confirm-text="t('ui.delete')"
      :loading="deleting"
      @confirm="confirmDeleteDraft"
    />
  </section>
</template>

<script setup lang="ts">
import { useI18n } from '@/i18n'

import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useLocalStorage } from '@vueuse/core'
import { onBeforeRouteLeave, onBeforeRouteUpdate, useRoute, useRouter, type RouteLocationNormalized } from 'vue-router'
import { ChevronRight, Plus, Save, Star, Trash2 } from 'lucide-vue-next'
import { Button, Card, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch } from '@/components/ui'
import { AlertDialog } from '@/components/common'
import HelpHint from '@/components/common/HelpHint.vue'
import {
  DEFAULT_STICKY_KEY_ATTEMPTS,
  createEmptyRoutingGroupConfig,
  MAX_STICKY_KEY_ATTEMPTS,
  parseStickyKeyAttempts,
  parseBillingMultiplier,
  type RoutingModelPolicy,
  type RoutingPriorityMode,
  type RoutingSchedulingMode,
  type RoutingGroupConfig,
} from '@/features/routing/utils/routingPolicy'
import RoutingFailoverPolicyEditor from '@/features/routing/components/RoutingFailoverPolicyEditor.vue'
import RoutingSchedulingPolicyEditor from '@/features/routing/components/RoutingSchedulingPolicyEditor.vue'
import { normalizeProviderSchedulingConfig } from '@/features/routing/utils/schedulingPolicies'
import { normalizeRoutingFailoverPolicy, validateRoutingFailoverPolicy, type RoutingFailoverPolicy } from '@/features/routing/utils/routingFailover'
import { createRoutingGroup, deleteRoutingGroup, listRoutingGroups, updateRoutingGroup, type RoutingGroupRecord } from '@/api/routing-profiles'
import { getGlobalModels, type GlobalModelResponse } from '@/api/global-models'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'

withDefaults(defineProps<{ providerRevision?: number }>(), { providerRevision: 0 })

const emit = defineEmits<{
  'inspect-provider': [providerId: string]
  saved: []
  'context-change': [context: { groupId: string | null; groupName: string; config: RoutingGroupConfig | null; busy: boolean; activePolicy: SchedulingSelection | null; providerModelIds: string[] | undefined; priorityMode: RoutingPriorityMode; schedulingMode: RoutingSchedulingMode }]
}>()

const { t } = useI18n()

interface SchedulingSelection {
  id?: string | null
  policy: RoutingModelPolicy | null
  priorityMode: RoutingPriorityMode
  schedulingMode: RoutingSchedulingMode
  scope: 'all' | 'selected'
  modelNames: string[]
}

interface RoutingGroupDraft {
  id?: string
  version: number
  name: string
  enabled: boolean
  is_system_default: boolean
  config_json: RoutingGroupConfig
}

const { success, error: showError } = useToast()
const { confirm } = useConfirm()
const route = useRoute()
const router = useRouter()
const groups = ref<RoutingGroupRecord[]>([])
const draft = ref<RoutingGroupDraft | null>(null)
const activePolicy = ref<SchedulingSelection | null>(null)
const initialSchedulingSelection = ref<Pick<SchedulingSelection, 'id' | 'scope' | 'modelNames'> | null>(null)
const routingSchedulingPolicyEditor = ref<{ updateSelectedPolicy: (policy: RoutingModelPolicy) => void } | null>(null)
const providerModelIds = computed(() => activePolicy.value?.scope === 'selected'
  ? globalModels.value.filter(model => activePolicy.value?.modelNames.includes(model.name)).map(model => model.id)
  : undefined)
const routingFailoverPolicyEditor = ref<{ commitJsonDrafts: () => boolean } | null>(null)
const routingFailoverPending = ref(false)
const routingSchedulingValid = ref(true)
const savedDraftSnapshot = ref<string | null>(null)
const globalModels = ref<GlobalModelResponse[]>([])
const loadingGlobalModels = ref(false)
const globalModelsError = ref<string | null>(null)
const loading = ref(true)
const loadingError = ref<string | null>(null)
const saving = ref(false)
const saveConflict = ref(false)
const deleting = ref(false)
const busy = computed(() => loading.value || saving.value || deleting.value)
const billingMultiplierInput = ref('1')
const invalidStickyKeyInput = ref<string | null>(null)
const draftGeneration = ref(0)
const groupMetadata = ref<HTMLElement | null>(null)
const panelCollapsed = useLocalStorage('aether-provider-policy-collapsed', false)
const advancedOpen = ref(false)
const failoverOpen = ref(false)
const deleteDialogOpen = ref(false)
let internalNavigation = false
let discardConfirmation: Promise<boolean> | null = null

const routeGroupId = computed(() => queryToString(route.query.group))
const defaultGroupId = computed(() => groups.value.find(group => group.is_system_default)?.id ?? groups.value[0]?.id ?? null)
const isNewDraft = computed(() => draft.value != null && !draft.value.id)
const selectedValue = computed(() => isNewDraft.value ? 'new' : draft.value?.id ?? '')
const priorityMode = 'provider' as const
const schedulingMode = computed(() => activePolicy.value?.schedulingMode ?? draft.value?.config_json.default_policy.scheduling_mode ?? 'cache_affinity')
const emptyMessage = computed(() => loading.value ? t('ui.loadingRoutingPolicies') : loadingError.value ?? (groups.value.length ? t('ui.routingPolicyNotFound') : t('providers.migration.noSchedulingPoliciesYetf0577d')))
const keepPriorityOnConversion = computed(() => draft.value?.config_json.default_policy.keep_priority_on_conversion ?? false)
const stickyKeyAttempts = computed(() => draft.value?.config_json.default_policy.sticky_key_attempts ?? DEFAULT_STICKY_KEY_ATTEMPTS)
const cfHeartbeat = computed(() => draft.value?.config_json.default_policy.enable_cf_heartbeat ?? false)
const cyberContinueFailover = computed(() => draft.value?.config_json.default_policy.cyber_continue_failover ?? false)
const cancelOnClientDisconnect = computed(() => draft.value?.config_json.default_policy.cancel_on_client_disconnect ?? false)
const billingMultiplierError = computed(() => parseBillingMultiplier(billingMultiplierInput.value) == null ? t('providers.migration.groupMultiplierMustBeA3664f4') : null)
const draftDirty = computed(() => draft.value != null && (Boolean(billingMultiplierError.value) || invalidStickyKeyInput.value !== null || routingFailoverPending.value || savedDraftSnapshot.value !== draftSnapshotValue(draft.value)))
const canSaveDraft = computed(() => Boolean(draft.value) && !busy.value && draftDirty.value && routingSchedulingValid.value && !billingMultiplierError.value && invalidStickyKeyInput.value === null)

function togglePanel(): void {
  panelCollapsed.value = !panelCollapsed.value
}

function setPanelHeight(element: Element, collapsed: boolean): void {
  if (window.matchMedia('(min-width: 1280px)').matches) return
  const panel = element as HTMLElement
  panel.style.height = collapsed ? '0px' : `${panel.scrollHeight}px`
}

function clearPanelHeight(element: Element): void {
  (element as HTMLElement).style.height = ''
}

function queryToString(value: unknown): string | null {
  if (Array.isArray(value)) return typeof value[0] === 'string' ? value[0] : null
  return typeof value === 'string' ? value : null
}

function normalizeRecord(group: RoutingGroupRecord): RoutingGroupRecord {
  return { ...group, sort_order: Number.isFinite(group.sort_order) ? group.sort_order : 0, config_json: normalizeProviderSchedulingConfig(group.config_json) }
}

function sortGroups(items: RoutingGroupRecord[]): RoutingGroupRecord[] {
  return [...items].sort((left, right) => {
    if (left.enabled !== right.enabled) return left.enabled ? -1 : 1
    return left.sort_order - right.sort_order || left.name.localeCompare(right.name) || left.id.localeCompare(right.id)
  })
}

function cloneConfig(config: RoutingGroupConfig): RoutingGroupConfig {
  return normalizeProviderSchedulingConfig(JSON.parse(JSON.stringify(config)) as Partial<RoutingGroupConfig>)
}

function draftSnapshotValue(value: RoutingGroupDraft): string {
  return JSON.stringify({ name: value.name.trim(), enabled: value.enabled, is_system_default: value.is_system_default, config_json: cloneConfig(value.config_json) })
}

function resetEditors(): void {
  invalidStickyKeyInput.value = null
  activePolicy.value = null
  initialSchedulingSelection.value = null
  saveConflict.value = false
  draftGeneration.value += 1
  routingFailoverPending.value = false
  routingSchedulingValid.value = true
  deleteDialogOpen.value = false
}

function selectGroup(group: RoutingGroupRecord, preserveSelection = false): void {
  const selection = preserveSelection ? activePolicy.value : null
  resetEditors()
  initialSchedulingSelection.value = selection
    ? { id: selection.id, scope: selection.scope, modelNames: [...selection.modelNames] }
    : null
  draft.value = { id: group.id, version: group.version, name: group.name, enabled: group.enabled, is_system_default: group.is_system_default, config_json: cloneConfig(group.config_json) }
  billingMultiplierInput.value = String(draft.value.config_json.billing_multiplier)
  savedDraftSnapshot.value = draftSnapshotValue(draft.value)
}

function openCreate(): void {
  if (busy.value || isNewDraft.value) return
  openGroup('new')
}

function startNewDraft(): void {
  panelCollapsed.value = false
  resetEditors()
  draft.value = { version: 0, name: '', enabled: true, is_system_default: groups.value.length === 0, config_json: createEmptyRoutingGroupConfig() }
  billingMultiplierInput.value = '1'
  savedDraftSnapshot.value = null
  void nextTick(() => {
    groupMetadata.value?.scrollIntoView?.({ block: 'nearest' })
    groupMetadata.value?.querySelector<HTMLInputElement>(`[aria-label="${t('ui.policyName')}"]`)?.focus()
  })
}

function clearDraft(): void {
  resetEditors()
  draft.value = null
  savedDraftSnapshot.value = null
}

function syncRouteState(): void {
  if (loading.value) return
  if (routeGroupId.value === 'new') {
    if (!isNewDraft.value) startNewDraft()
    return
  }
  const group = groups.value.find(item => item.id === (routeGroupId.value ?? defaultGroupId.value))
  if (!group) { clearDraft(); return }
  if (draft.value?.id !== group.id) selectGroup(group)
}
function openGroup(groupId: string | null): void {
  if (busy.value) return
  void router.push({ name: 'ProviderManagement', query: { ...route.query, view: undefined, group: groupId ?? undefined } })
}

function changeGroup(value: string): void {
  // The route guard owns the switch; keep the controlled value until navigation succeeds.
  openGroup(value)
}

async function confirmDiscard(): Promise<boolean> {
  if (!draftDirty.value) return true
  if (!discardConfirmation) {
    discardConfirmation = confirm({ title: t('providers.migration.unsavedSchedulingChangesb7663a'), message: t('providers.migration.leavingWillDiscardUnsavedChanges2eb7b5'), confirmText: t('providers.migration.discardChangesAndLeavec77a61'), cancelText: t('providers.migration.continueEditing3a481b'), variant: 'warning' })
      .finally(() => { discardConfirmation = null })
  }
  return discardConfirmation
}

async function guardNavigation(to: RouteLocationNormalized): Promise<boolean> {
  if (internalNavigation) return true
  const targetGroup = queryToString(to.query.group) ?? defaultGroupId.value
  const staysOnDraft = to.name === 'ProviderManagement' && targetGroup === selectedValue.value
  if (staysOnDraft) return true
  if (busy.value) {
    showError(t('providers.migration.savingSchedulingSettingsWaitBefore6c38e9'))
    return false
  }
  return confirmDiscard()
}

onBeforeRouteUpdate(guardNavigation)
onBeforeRouteLeave(guardNavigation)

function preventUnload(event: BeforeUnloadEvent): void {
  if (!draftDirty.value && !busy.value) return
  event.preventDefault()
  event.returnValue = ''
}

function updateDraftConfig(value: RoutingGroupConfig): void {
  if (draft.value) draft.value.config_json = normalizeProviderSchedulingConfig(value)
}

function updateBillingMultiplier(value: string | number): void {
  if (!draft.value || busy.value) return
  billingMultiplierInput.value = String(value)
  const parsed = parseBillingMultiplier(value)
  if (parsed != null) draft.value.config_json.billing_multiplier = parsed
}

function updatePriorityPolicy(policy: RoutingModelPolicy): void {
  if (busy.value) return
  routingSchedulingPolicyEditor.value?.updateSelectedPolicy(policy)
}

function updateStickyKeyAttempts(value: string | number): void {
  if (!draft.value) return
  const parsed = parseStickyKeyAttempts(value)
  invalidStickyKeyInput.value = parsed === null ? String(value) : null
  if (parsed !== null) updateDraftConfig({ ...draft.value.config_json, default_policy: { ...draft.value.config_json.default_policy, sticky_key_attempts: parsed } })
}

function updateKeepPriorityOnConversion(value: boolean): void {
  if (!draft.value) return
  updateDraftConfig({ ...draft.value.config_json, default_policy: { ...draft.value.config_json.default_policy, keep_priority_on_conversion: value } })
}

function updateExecutionPolicy(field: 'enable_cf_heartbeat' | 'cyber_continue_failover' | 'cancel_on_client_disconnect' | 'strict_format_conversion', value: boolean): void {
  if (!draft.value) return
  updateDraftConfig({ ...draft.value.config_json, default_policy: { ...draft.value.config_json.default_policy, [field]: value } })
}

function updateRoutingFailoverPolicy(value: RoutingFailoverPolicy): void {
  if (draft.value) Object.assign(draft.value.config_json.default_policy, normalizeRoutingFailoverPolicy(value))
}

function replaceGroup(group: RoutingGroupRecord, select: boolean, preserveSelection = false): void {
  const normalized = normalizeRecord(group)
  const otherGroups = groups.value.filter(item => item.id !== normalized.id).map(item => normalized.is_system_default ? { ...item, is_system_default: false } : item)
  groups.value = sortGroups([...otherGroups, normalized])
  if (select) selectGroup(normalized, preserveSelection)
}

async function refreshGroups(): Promise<void> {
  const preservedDraft = draft.value
  loading.value = true
  loadingError.value = null
  try {
    const response = await listRoutingGroups()
    groups.value = sortGroups(response.items.map(normalizeRecord))
  } catch (err) {
    loadingError.value = parseApiError(err, t('providers.migration.failedToLoadSchedulingPolicy9cfb4b'))
    showError(loadingError.value)
    log.error('加载调度策略失败:', err)
  } finally {
    loading.value = false
    if (!preservedDraft) syncRouteState()
  }
}

async function reloadCurrentGroup(): Promise<void> {
  if (busy.value || !draft.value?.id) return
  if (draftDirty.value && !await confirm({ title: t('providers.migration.reloadTheLatestGroup6a36ee'), message: t('providers.migration.reloadingWillDiscardYourUnsaved6b357f'), confirmText: t('providers.migration.discardChangesAndReload49bf36'), cancelText: t('providers.migration.keepChanges3d1dcc'), variant: 'warning' })) return
  const groupId = draft.value.id
  await refreshGroups()
  if (loadingError.value) return
  const group = groups.value.find(item => item.id === groupId)
  if (group) selectGroup(group)
  else { clearDraft(); syncRouteState() }
}

async function loadGlobalModels(options: { cacheTtlMs?: number } = {}): Promise<void> {
  loadingGlobalModels.value = true
  globalModelsError.value = null
  try {
    const response = await getGlobalModels({ limit: 1000, is_active: true }, { cacheTtlMs: options.cacheTtlMs ?? 0 })
    globalModels.value = response.models ?? []
  } catch (err) {
    globalModels.value = []
    globalModelsError.value = parseApiError(err, t('ui.failedToLoadGlobalModels'))
    log.error('加载全局模型失败:', err)
  } finally {
    loadingGlobalModels.value = false
  }
}

async function saveDraft(): Promise<boolean> {
  if (!draft.value || busy.value) return false
  const name = draft.value.name.trim()
  if (!name) {
    panelCollapsed.value = false
    await nextTick()
    groupMetadata.value?.scrollIntoView?.({ block: 'nearest' })
    groupMetadata.value?.querySelector<HTMLInputElement>(`[aria-label="${t('ui.policyName')}"]`)?.focus()
    showError(t('providers.migration.policyNameCannotBeEmpty384f05'))
    return false
  }
  if (billingMultiplierError.value) {
    panelCollapsed.value = false
    await nextTick()
    groupMetadata.value?.scrollIntoView?.({ block: 'nearest' })
    groupMetadata.value?.querySelector<HTMLInputElement>(`[aria-label="${t('ui.groupMultiplier')}"]`)?.focus()
    showError(billingMultiplierError.value)
    return false
  }
  if (routingFailoverPolicyEditor.value && !routingFailoverPolicyEditor.value.commitJsonDrafts()) {
    panelCollapsed.value = false
    failoverOpen.value = true
    return false
  }
  const failoverError = validateRoutingFailoverPolicy(draft.value.config_json.default_policy)
  if (failoverError || invalidStickyKeyInput.value !== null) {
    panelCollapsed.value = false
    failoverOpen.value = true
    showError(failoverError ?? t('ui.invalidStickyKeyAttempts'))
    return false
  }
  if (!routingSchedulingValid.value) {
    panelCollapsed.value = false
    showError(t('providers.migration.selectApplicableModelsForEverycdf617'))
    return false
  }
  const targetGroupId = draft.value.id
  const submittedGeneration = draftGeneration.value
  const submittedSnapshot = draftSnapshotValue(draft.value)
  const config = cloneConfig(draft.value.config_json)
  const savedGroup = groups.value.find(group => group.id === targetGroupId)
  const configChanged = !savedGroup || JSON.stringify(config) !== JSON.stringify(savedGroup.config_json)
  const payload = { name, enabled: draft.value.enabled, is_system_default: draft.value.is_system_default, ...(configChanged ? { config_json: config } : {}) }
  const expectedVersion = draft.value.version
  saving.value = true
  try {
    const saved = targetGroupId
      ? await updateRoutingGroup(targetGroupId, { ...payload, expected_version: expectedVersion })
      : await createRoutingGroup({ ...payload, sort_order: groups.value.length })
    const unchanged = draftGeneration.value === submittedGeneration && draft.value?.id === targetGroupId && draftSnapshotValue(draft.value) === submittedSnapshot
    replaceGroup(saved, unchanged, true)
    if (!targetGroupId) {
      // Retain any newer edits while attaching the server ID, so retries update the created group.
      if (!unchanged && draft.value && draftGeneration.value === submittedGeneration && !draft.value.id) {
        draft.value.id = saved.id
        draft.value.version = saved.version
        savedDraftSnapshot.value = draftSnapshotValue({ ...saved, config_json: cloneConfig(saved.config_json) })
      }
      internalNavigation = true
      try { await router.replace({ name: 'ProviderManagement', query: { ...route.query, view: undefined, group: saved.id } }) }
      finally { internalNavigation = false }
    }
    success(targetGroupId ? t('ui.routingPolicySaved') : t('providers.migration.policyGroupCreatedde1b43'))
    emit('saved')
    return unchanged
  } catch (err) {
    panelCollapsed.value = false
    const status = (err as { response?: { status?: number } })?.response?.status
    saveConflict.value = Boolean(targetGroupId) && status === 409
    showError(saveConflict.value ? t('providers.migration.thisGroupWasUpdatedElsewherea29490') : parseApiError(err, targetGroupId ? t('providers.migration.failedToSaveSchedulingPolicyd47258') : t('providers.migration.failedToCreatePolicyGroup58a204')))
    log.error('保存调度策略失败:', err)
    return false
  } finally { saving.value = false }
}
async function ensureSaved(): Promise<boolean> {
  if (busy.value || !draft.value) return false
  if (!draftDirty.value) return true
  const approved = await confirm({ title: t('providers.migration.saveTheCurrentGroupFirstf30a4f'), message: t('providers.migration.thisGroupHasUnsavedChangesa09432'), confirmText: t('providers.migration.saveAndContinue4e2605'), cancelText: t('providers.migration.continueEditing3a481b'), variant: 'question' })
  return approved && await saveDraft()
}

async function confirmDeleteDraft(): Promise<void> {
  if (!draft.value?.id || busy.value) return
  const targetId = draft.value.id
  deleting.value = true
  try {
    await deleteRoutingGroup(targetId)
    groups.value = groups.value.filter(group => group.id !== targetId)
    clearDraft()
    internalNavigation = true
    try {
      await router.replace({ name: 'ProviderManagement', query: { ...route.query, view: undefined, group: defaultGroupId.value ?? undefined } })
      syncRouteState()
    } finally {
      internalNavigation = false
    }
    success(t('ui.routingPolicyDeleted'))
    emit('saved')
  } catch (err) {
    showError(parseApiError(err, t('providers.migration.failedToDeleteSchedulingPolicyba98ea')))
    log.error('删除调度策略失败:', err)
  } finally {
    deleting.value = false
  }
}

onMounted(() => {
  void refreshGroups()
  void loadGlobalModels({ cacheTtlMs: 60_000 })
  window.addEventListener('beforeunload', preventUnload)
})
onBeforeUnmount(() => window.removeEventListener('beforeunload', preventUnload))
watch(() => [route.name, route.query.group], syncRouteState)
watch(() => [draft.value, busy.value, activePolicy.value, providerModelIds.value], () => emit('context-change', { groupId: draft.value?.id ?? null, groupName: draft.value?.name ?? '', config: draft.value?.config_json ?? null, busy: busy.value, activePolicy: activePolicy.value, providerModelIds: providerModelIds.value, priorityMode, schedulingMode: schedulingMode.value }), { deep: true, immediate: true })
defineExpose({ updateDraftConfig, updatePriorityPolicy, refreshGroups, ensureSaved })

</script>

<style scoped>
.policy-panel-enter-active,
.policy-panel-leave-active {
  overflow: hidden;
  transition: height 200ms ease-out, opacity 200ms ease-out, transform 200ms ease-out;
}

.policy-panel-enter-from,
.policy-panel-leave-to {
  opacity: 0;
  transform: translateX(-8px);
}

@media (prefers-reduced-motion: reduce) {
  .policy-panel-enter-active,
  .policy-panel-leave-active {
    transition: none;
  }
}
</style>
