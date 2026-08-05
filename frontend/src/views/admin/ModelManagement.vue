<template>
  <div class="flex flex-col h-[calc(100vh-12rem)]">
    <!-- 主内容区 -->
    <div class="flex-1 flex flex-col min-w-0">
      <!-- 模型列表 -->
      <Card class="overflow-hidden">
        <!-- 标题和操作栏 -->
        <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
          <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
            <!-- 左侧：标题 -->
            <h3 class="text-sm sm:text-base font-semibold shrink-0">
              模型管理
            </h3>

            <!-- 右侧：操作区 -->
            <div class="flex flex-wrap items-center gap-2">
              <!-- 搜索框 -->
              <div class="relative">
                <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground/70 z-10 pointer-events-none" />
                <Input
                  id="model-search"
                  v-model="searchQuery"
                  type="text"
                  placeholder="搜索模型名称..."
                  class="w-32 sm:w-44 pl-8 pr-3 h-8 text-sm bg-muted/30 border-border/50 focus:border-primary/50 transition-colors"
                />
              </div>

              <!-- 操作按钮 -->
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="快速筛选与批量操作"
                @click="openBatchManageDialog"
              >
                <ListChecks class="w-3.5 h-3.5" />
              </Button>
              <ExternalModelsAccessControl />
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="创建模型"
                @click="openCreateModelDialog"
              >
                <Plus class="w-3.5 h-3.5" />
              </Button>
              <RefreshButton
                :loading="loading"
                @click="refreshData"
              />
            </div>
          </div>
        </div>

        <Table class="hidden xl:table">
          <TableHeader>
            <TableRow>
              <TableHead class="w-[240px]">
                模型名称
              </TableHead>
              <TableHead class="w-[160px] text-center">
                价格 ($/M)
              </TableHead>
              <TableHead class="w-[80px] text-center">
                提供商
              </TableHead>
              <TableHead class="w-[80px] text-center">
                调用次数
              </TableHead>
              <TableHead class="w-[70px]">
                状态
              </TableHead>
              <TableHead class="w-[140px] text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow v-if="loading">
              <TableCell
                colspan="6"
                class="text-center py-8"
              >
                <Loader2 class="w-6 h-6 animate-spin mx-auto" />
              </TableCell>
            </TableRow>
            <TableRow v-else-if="filteredGlobalModels.length === 0">
              <TableCell
                colspan="6"
                class="text-center py-8 text-muted-foreground"
              >
                没有找到匹配的模型
              </TableCell>
            </TableRow>
            <template v-else>
              <TableRow
                v-for="model in paginatedGlobalModels"
                :key="model.id"
                class="cursor-pointer hover:bg-muted/50 group"
                @mousedown="handleMouseDown"
                @click="handleRowClick($event, model)"
              >
                <TableCell>
                  <div>
                    <div class="font-medium">
                      {{ model.display_name }}
                    </div>
                    <div class="text-xs text-muted-foreground flex items-center gap-1">
                      <span>{{ model.name }}</span>
                      <button
                        class="p-0.5 rounded hover:bg-muted transition-colors"
                        title="复制模型 ID"
                        @click.stop="copyToClipboard(model.name)"
                      >
                        <Copy class="w-3 h-3" />
                      </button>
                    </div>
                  </div>
                </TableCell>
                <TableCell class="text-center">
                  <div class="text-xs space-y-0.5">
                    <!-- 按 Token 计费 -->
                    <div v-if="getFirstTierPrice(model, 'input') || getFirstTierPrice(model, 'output')">
                      <span class="text-muted-foreground">In:</span>
                      <span class="font-mono ml-1">{{ getFirstTierPrice(model, 'input')?.toFixed(2) || '-' }}</span>
                      <span class="text-muted-foreground mx-1">/</span>
                      <span class="text-muted-foreground">Out:</span>
                      <span class="font-mono ml-1">{{ getFirstTierPrice(model, 'output')?.toFixed(2) || '-' }}</span>
                      <!-- 阶梯计费标记 -->
                      <span
                        v-if="hasTieredPricing(model)"
                        class="ml-1 text-muted-foreground"
                        title="阶梯计费"
                      >[阶梯]</span>
                    </div>
                    <!-- 按次计费 -->
                    <div v-if="model.default_price_per_request && model.default_price_per_request > 0">
                      <span class="text-muted-foreground">按次:</span>
                      <span class="font-mono ml-1">${{ model.default_price_per_request.toFixed(3) }}/次</span>
                    </div>
                    <!-- 视频费用计费 -->
                    <div v-if="hasVideoPricing(model)">
                      <span class="text-muted-foreground">视频:</span>
                      <span
                        class="font-mono ml-1"
                        :title="getVideoPricingTooltip(model)"
                      >{{ getVideoPricingDisplay(model) }}</span>
                    </div>
                    <!-- 无计费配置 -->
                    <div
                      v-if="!getFirstTierPrice(model, 'input') && !getFirstTierPrice(model, 'output') && !model.default_price_per_request && !hasVideoPricing(model)"
                      class="text-muted-foreground"
                    >
                      -
                    </div>
                  </div>
                </TableCell>
                <TableCell class="text-center">
                  <Badge variant="secondary">
                    {{ model.active_provider_count || 0 }}/{{ model.provider_count || 0 }}
                  </Badge>
                </TableCell>
                <TableCell class="text-center">
                  <span class="text-sm font-mono">{{ formatUsageCount(model.usage_count || 0) }}</span>
                </TableCell>
                <TableCell>
                  <Badge :variant="model.is_active ? 'default' : 'secondary'">
                    {{ model.is_active ? '活跃' : '停用' }}
                  </Badge>
                </TableCell>
                <TableCell>
                  <div class="flex items-center justify-center gap-1">
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="查看详情"
                      @click.stop="selectModel(model)"
                    >
                      <Eye class="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="编辑模型"
                      @click.stop="editModel(model)"
                    >
                      <Edit class="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      :title="model.is_active ? '停用模型' : '启用模型'"
                      @click.stop="toggleModelStatus(model)"
                    >
                      <Power class="w-4 h-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-8 w-8"
                      title="删除模型"
                      @click.stop="deleteModel(model)"
                    >
                      <Trash2 class="w-4 h-4" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </template>
          </TableBody>
        </Table>

        <!-- 移动端卡片列表 -->
        <div
          v-if="!loading && paginatedGlobalModels.length > 0"
          class="xl:hidden divide-y divide-border/40"
        >
          <div
            v-for="model in paginatedGlobalModels"
            :key="model.id"
            class="p-4 space-y-3 hover:bg-muted/50 cursor-pointer transition-colors"
            @click="selectModel(model)"
          >
            <!-- 第一行：名称 + 状态 + 操作 -->
            <div class="flex items-start justify-between gap-3">
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="font-medium truncate">{{ model.display_name }}</span>
                  <Badge
                    :variant="model.is_active ? 'default' : 'secondary'"
                    class="text-xs shrink-0"
                  >
                    {{ model.is_active ? '活跃' : '停用' }}
                  </Badge>
                </div>
                <div class="text-xs text-muted-foreground flex items-center gap-1 mt-0.5">
                  <span class="font-mono truncate">{{ model.name }}</span>
                  <button
                    class="p-0.5 rounded hover:bg-muted transition-colors shrink-0"
                    @click.stop="copyToClipboard(model.name)"
                  >
                    <Copy class="w-3 h-3" />
                  </button>
                </div>
              </div>
              <div
                class="flex items-center gap-0.5 shrink-0"
                @click.stop
              >
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  @click="editModel(model)"
                >
                  <Edit class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  @click="toggleModelStatus(model)"
                >
                  <Power class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  @click="deleteModel(model)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </Button>
              </div>
            </div>

            <!-- 第二行：统计信息 -->
            <div class="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
              <span>提供商 {{ model.active_provider_count || 0 }}/{{ model.provider_count || 0 }}</span>
              <span>调用 {{ formatUsageCount(model.usage_count || 0) }}</span>
              <span
                v-if="getFirstTierPrice(model, 'input') || getFirstTierPrice(model, 'output')"
                class="font-mono"
              >
                ${{ getFirstTierPrice(model, 'input')?.toFixed(2) || '-' }}/${{ getFirstTierPrice(model, 'output')?.toFixed(2) || '-' }}
              </span>
            </div>
          </div>
        </div>

        <!-- 分页 -->
        <Pagination
          v-if="!loading && totalGlobalModels > 0"
          :current="catalogCurrentPage"
          :total="totalGlobalModels"
          :page-size="catalogPageSize"
          cache-key="model-management-page-size"
          @update:current="catalogCurrentPage = $event"
          @update:page-size="catalogPageSize = $event"
        />
      </Card>
    </div>

    <!-- 创建/编辑模型对话框 -->
    <GlobalModelFormDialog
      :open="createModelDialogOpen"
      :model="editingModel"
      @update:open="handleModelDialogUpdate"
      @success="handleModelFormSuccess"
      @edit-model="editModel"
      @pricing-synced="handleModelPricingSynced"
    />

    <!-- 模型详情抽屉 -->
    <ModelDetailDrawer
      ref="modelDetailDrawerRef"
      :model="selectedModel"
      :open="!!selectedModel"
      :has-blocking-dialog-open="hasBlockingDialogOpen"
      @update:open="handleDrawerOpenChange"
      @edit-model="editModel"
      @toggle-model-status="toggleModelStatus"
      @add-provider="openAddProviderDialog"
      @edit-provider="openEditProviderImplementation"
      @delete-provider="confirmDeleteProviderImplementation"
      @toggle-provider-status="toggleProviderStatus"
      @refresh-model="refreshSelectedModel"
      @link-provider="linkProviderToModel"
      @link-providers="linkProvidersToModel"
    />

    <!-- 批量添加关联提供商对话框 -->
    <Dialog
      :model-value="batchAddProvidersDialogOpen"
      :title="selectedModel ? `批量管理提供商 - ${selectedModel.display_name}` : '批量管理提供商'"
      description="选中的提供商将被关联到模型，取消选中将移除关联"
      :icon="Server"
      size="2xl"
      @update:model-value="handleBatchAddProvidersDialogUpdate"
    >
      <template #default>
        <div class="space-y-4">
          <!-- 搜索栏 -->
          <div class="flex items-center gap-2">
            <div class="flex-1 relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
              <Input
                v-model="batchProviderSearchQuery"
                placeholder="搜索提供商..."
                class="pl-8 h-9"
              />
            </div>
          </div>

          <!-- 单列提供商列表 -->
          <div class="border rounded-lg overflow-hidden">
            <div class="max-h-96 overflow-y-auto">
              <div
                v-if="loadingProviderOptions"
                class="flex items-center justify-center py-12"
              >
                <Loader2 class="w-6 h-6 animate-spin text-primary" />
              </div>

              <template v-else>
                <!-- 提供商组 -->
                <div v-if="filteredBatchProviders.length > 0">
                  <div
                    class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10"
                  >
                    <div class="flex items-center gap-2">
                      <span class="text-xs font-medium">提供商</span>
                      <span class="text-xs text-muted-foreground">({{ filteredBatchProviders.length }})</span>
                    </div>
                    <button
                      type="button"
                      class="text-xs text-primary hover:underline shrink-0"
                      @click="toggleAllBatchProviders"
                    >
                      {{ isAllBatchProvidersSelected ? '取消全选' : '全选' }}
                    </button>
                  </div>
                  <div class="space-y-1 p-2">
                    <div
                      v-for="provider in filteredBatchProviders"
                      :key="provider.id"
                      class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                      @click="toggleBatchProviderSelection(provider.id)"
                    >
                      <div
                        class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                        :class="isBatchProviderSelected(provider.id) ? 'bg-primary border-primary' : ''"
                      >
                        <Check
                          v-if="isBatchProviderSelected(provider.id)"
                          class="w-3 h-3 text-primary-foreground"
                        />
                      </div>
                      <div class="flex-1 min-w-0">
                        <p class="text-sm font-medium truncate">
                          {{ provider.name }}
                        </p>
                      </div>
                      <Badge
                        :variant="provider.is_active ? 'outline' : 'secondary'"
                        :class="provider.is_active ? 'text-green-600 border-green-500/60' : ''"
                        class="text-xs shrink-0"
                      >
                        {{ provider.is_active ? '活跃' : '停用' }}
                      </Badge>
                    </div>
                  </div>
                </div>

                <!-- 空状态 -->
                <div
                  v-if="filteredBatchProviders.length === 0"
                  class="flex flex-col items-center justify-center py-12 text-muted-foreground"
                >
                  <Building2 class="w-10 h-10 mb-2 opacity-30" />
                  <p class="text-sm">
                    {{ batchProviderSearchQuery ? '无匹配结果' : '暂无可用提供商' }}
                  </p>
                </div>
              </template>
            </div>
          </div>
        </div>
      </template>
      <template #footer>
        <div class="flex items-center justify-between w-full">
          <p class="text-xs text-muted-foreground">
            {{ hasBatchProviderChanges ? `${batchProviderPendingChangesCount} 项更改待保存` : '' }}
          </p>
          <div class="flex items-center gap-2">
            <Button
              :disabled="!hasBatchProviderChanges || submittingBatchProviders"
              @click="saveBatchProviderChanges"
            >
              <Loader2
                v-if="submittingBatchProviders"
                class="w-4 h-4 mr-1 animate-spin"
              />
              {{ submittingBatchProviders ? '保存中...' : '保存' }}
            </Button>
            <Button
              variant="outline"
              @click="closeBatchAddProvidersDialog"
            >
              关闭
            </Button>
          </div>
        </div>
      </template>
    </Dialog>

    <!-- 编辑提供商模型对话框 -->
    <ProviderModelFormDialog
      :open="editProviderDialogOpen"
      :provider-id="editingProvider?.id || ''"
      :provider-name="editingProvider?.name || ''"
      :editing-model="editingProviderModel"
      @update:open="handleEditProviderDialogUpdate"
      @saved="handleEditProviderSaved"
    />

    <!-- 批量管理全局模型对话框 -->
    <Dialog
      :model-value="batchManageDialogOpen"
      title="快速筛选与批量操作"
      description="默认按每个模型上次选择的在线来源同步，也可手动指定统一来源"
      :icon="ListChecks"
      size="2xl"
      @update:model-value="batchManageDialogOpen = $event"
    >
      <template #default>
        <div class="space-y-4">
          <!-- 搜索与在线价格来源 -->
          <div class="grid grid-cols-1 gap-2 sm:grid-cols-[minmax(0,1fr)_220px]">
            <div class="flex-1 relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
              <Input
                v-model="batchManageSearchQuery"
                placeholder="搜索模型..."
                class="pl-8 h-9"
              />
            </div>
            <Select
              v-model="batchPricingProviderId"
              :disabled="batchManageOnlineLoading"
            >
              <SelectTrigger class="h-9 text-xs">
                <SelectValue placeholder="选择在线价格来源" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem :value="REMEMBERED_PRICING_PROVIDER_ID">
                  上次选择（按模型）
                </SelectItem>
                <SelectItem
                  v-for="provider in batchPricingProviderOptions"
                  :key="provider.providerId"
                  :value="provider.providerId"
                >
                  {{ provider.providerName }}
                  <span class="ml-1 text-muted-foreground">({{ provider.matchCount }})</span>
                </SelectItem>
              </SelectContent>
            </Select>
          </div>

          <!-- 快捷选中 -->
          <div class="flex flex-wrap items-center gap-1.5">
            <span class="text-xs text-muted-foreground mr-0.5">快捷选中:</span>
            <button
              v-for="shortcut in batchManageShortcuts"
              :key="shortcut.label"
              type="button"
              class="rounded-md border border-border/60 px-2 py-1 text-xs transition-colors hover:bg-muted active:scale-[0.96]"
              :class="shortcut.emphasis ? 'border-primary/30 bg-primary/5 text-primary' : ''"
              :title="shortcut.description"
              @click="applyBatchManageShortcut(shortcut.filter)"
            >
              {{ shortcut.label }} ({{ shortcut.count }})
            </button>
          </div>

          <!-- 模型列表 -->
          <div class="border rounded-lg overflow-hidden">
            <div class="max-h-96 overflow-y-auto">
              <div
                v-if="batchManageLoading || batchManageOnlineLoading"
                class="flex items-center justify-center py-12"
              >
                <Loader2 class="w-6 h-6 animate-spin text-primary" />
              </div>

              <template v-else-if="filteredBatchManageModels.length > 0">
                <div
                  class="flex items-center justify-between px-3 py-2 bg-muted sticky top-0 z-10"
                >
                  <div class="flex items-center gap-2">
                    <span class="text-xs font-medium">模型</span>
                    <span class="text-xs text-muted-foreground">({{ filteredBatchManageModels.length }})</span>
                  </div>
                  <button
                    type="button"
                    class="text-xs text-primary hover:underline shrink-0"
                    @click="toggleAllBatchManageModels"
                  >
                    {{ isAllBatchManageModelsSelected ? '取消全选' : '全选' }}
                  </button>
                </div>
                <div class="space-y-1 p-2">
                  <div
                    v-for="model in filteredBatchManageModels"
                    :key="model.id"
                    class="flex items-center gap-2 px-2 py-1.5 rounded hover:bg-muted cursor-pointer"
                    @click="toggleBatchManageModelSelection(model.id)"
                  >
                    <div
                      class="w-4 h-4 border rounded flex items-center justify-center shrink-0"
                      :class="selectedBatchManageModelIds.has(model.id) ? 'bg-primary border-primary' : ''"
                    >
                      <Check
                        v-if="selectedBatchManageModelIds.has(model.id)"
                        class="w-3 h-3 text-primary-foreground"
                      />
                    </div>
                    <div class="flex-1 min-w-0">
                      <p class="text-sm font-medium truncate">
                        {{ model.display_name }}
                      </p>
                      <p class="text-xs text-muted-foreground truncate">
                        {{ model.name }}
                      </p>
                    </div>
                    <div class="flex items-center gap-2 shrink-0">
                      <span
                        class="max-w-28 truncate text-[10px] text-muted-foreground"
                        :title="getBatchPricingSourceLabel(model)"
                      >{{ getBatchPricingSourceLabel(model) }}</span>
                      <span
                        class="inline-flex items-center gap-1.5 text-[11px] font-medium"
                        :class="getBatchPricingStateClass(model)"
                        :title="getBatchPricingStateDescription(model)"
                      >
                        <span
                          class="h-1.5 w-1.5 rounded-full"
                          :class="getBatchPricingStateDotClass(model)"
                        />
                        {{ getBatchPricingStateLabel(model) }}
                      </span>
                      <Badge
                        variant="secondary"
                        class="text-xs"
                      >
                        {{ model.active_provider_count || 0 }}/{{ model.provider_count || 0 }}
                      </Badge>
                      <Badge
                        :variant="model.is_active ? 'outline' : 'secondary'"
                        :class="model.is_active ? 'text-green-600 border-green-500/60' : ''"
                        class="text-xs"
                      >
                        {{ model.is_active ? '活跃' : '停用' }}
                      </Badge>
                    </div>
                  </div>
                </div>
              </template>

              <!-- 空状态 -->
              <div
                v-else
                class="flex flex-col items-center justify-center py-12 text-muted-foreground"
              >
                <p class="text-sm">
                  {{ batchManageSearchQuery ? '无匹配结果' : '暂无模型' }}
                </p>
              </div>
            </div>
          </div>
        </div>
      </template>
      <template #footer>
        <div class="flex w-full min-w-0 flex-col gap-3 lg:flex-row lg:items-center lg:justify-between">
          <p class="min-w-0 break-keep text-pretty text-xs leading-5 text-muted-foreground lg:flex-1">
            {{ batchManageSelectionSummary }}
          </p>
          <div class="flex w-full min-w-0 flex-col gap-2 sm:flex-row lg:w-auto lg:shrink-0">
            <Button
              class="w-full whitespace-nowrap sm:flex-1 lg:w-auto lg:flex-none"
              :disabled="selectedBatchPriceSyncPlan.syncable.length === 0 || submittingBatchManage"
              @click="confirmBatchSyncPrices"
            >
              <Loader2
                v-if="batchManageAction === 'sync-prices'"
                class="w-4 h-4 mr-1 animate-spin"
              />
              <RefreshCw
                v-else
                class="w-4 h-4 mr-1"
              />
              {{ batchManageAction === 'sync-prices' ? '同步中...' : `同步在线价格 (${selectedBatchPriceSyncPlan.syncable.length})` }}
            </Button>
            <Button
              class="w-full whitespace-nowrap sm:flex-1 lg:w-auto lg:flex-none"
              variant="destructive"
              :disabled="selectedBatchManageModelIds.size === 0 || submittingBatchManage"
              @click="confirmBatchDeleteModels"
            >
              <Loader2
                v-if="batchManageAction === 'delete'"
                class="w-4 h-4 mr-1 animate-spin"
              />
              {{ batchManageAction === 'delete' ? '删除中...' : '删除选中' }}
            </Button>
            <Button
              class="w-full whitespace-nowrap sm:flex-1 lg:w-auto lg:flex-none"
              variant="outline"
              @click="batchManageDialogOpen = false"
            >
              关闭
            </Button>
          </div>
        </div>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onBeforeUnmount, watch } from 'vue'
import {
  Plus,
  Edit,
  Trash2,
  Loader2,
  Eye,
  Building2,
  Search,
  Power,
  Copy,
  Server,
  Check,
  ListChecks,
  RefreshCw,
} from 'lucide-vue-next'
import ModelDetailDrawer from '@/features/models/components/ModelDetailDrawer.vue'
import GlobalModelFormDialog from '@/features/models/components/GlobalModelFormDialog.vue'
import ExternalModelsAccessControl from '@/features/models/components/ExternalModelsAccessControl.vue'
import ProviderModelFormDialog from '@/features/providers/components/ProviderModelFormDialog.vue'
import type { Model } from '@/api/endpoints'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useClipboard } from '@/composables/useClipboard'
import { useRowClick } from '@/composables/useRowClick'
import { parseApiError } from '@/utils/errorParser'
import {
  getVideoBillingMode,
  getVideoDefaultPricePerSecond,
  getVideoPricePerSecondTable,
  getVideoTokenPriceEntries,
  hasVideoPricing as hasVideoPricingConfigured,
  sortResolutionEntries,
  type VideoTokenPriceEntry,
} from '@/utils/form'
import {
  Button,
  Card,
  Input,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Badge,
  Dialog,
  Pagination,
  RefreshButton,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import {
  listGlobalModels,
  getGlobalModel,
  updateGlobalModel,
  deleteGlobalModel,
  batchDeleteGlobalModels,
  batchAssignToProviders,
  getGlobalModelProviders,
  type GlobalModelResponse,
} from '@/api/global-models'
import { log } from '@/utils/logger'
import { formatUsageCount } from '@/utils/format'
import { getProvidersSummary, type ProviderWithEndpointsSummary } from '@/api/endpoints/providers'
import { getModelsDevList, type ModelsDevModelItem } from '@/api/models-dev'
import {
  buildGlobalModelPriceSyncPlan,
  cloneTieredPricingConfig,
} from '@/features/models/components/global-model-form-helpers'
import { useModelsDevPricingSources } from '@/features/models/composables/useModelsDevPricingSources'


interface ModelProviderDisplay {
  id: string
  model_id?: string | null
  name: string
  provider_type: string
  target_model: string
  is_active: boolean
  input_price_per_1m?: number | null
  output_price_per_1m?: number | null
  cache_creation_price_per_1m?: number | null
  cache_read_price_per_1m?: number | null
  cache_1h_creation_price_per_1m?: number | null
  price_per_request?: number | null
  effective_tiered_pricing?: unknown
  tier_count?: number
  supports_vision?: boolean | null
  supports_function_calling?: boolean | null
  supports_streaming?: boolean | null
  supports_extended_thinking?: boolean | null
  supports_embedding?: boolean | null
}

const { success, error: showError } = useToast()
const { copyToClipboard } = useClipboard()
const { getSource: getModelsDevPricingSource, setSource: setModelsDevPricingSource } = useModelsDevPricingSources()
const REMEMBERED_PRICING_PROVIDER_ID = '__remembered__'

// 状态
const loading = ref(false)
const detailTab = ref('basic')
const searchQuery = ref('')
const selectedModel = ref<GlobalModelResponse | null>(null)
const modelDetailDrawerRef = ref<InstanceType<typeof ModelDetailDrawer> | null>(null)
const createModelDialogOpen = ref(false)
const editingModel = ref<GlobalModelResponse | null>(null)

// 数据
const globalModels = ref<GlobalModelResponse[]>([])
const totalGlobalModels = ref(0)
const batchManageModels = ref<GlobalModelResponse[]>([])
const batchManageLoading = ref(false)
const batchManageOnlineModels = ref<ModelsDevModelItem[]>([])
const batchManageOnlineLoading = ref(false)
const GLOBAL_MODELS_BATCH_FETCH_PAGE_SIZE = 1000
let globalModelsRequestId = 0
let modelSelectionRequestId = 0
let modelProvidersRequestId = 0
let batchManageModelsRequestId = 0
let providerOptionsRequest: Promise<void> | null = null
const GLOBAL_MODELS_LIST_CACHE_TTL_MS = 10 * 1000

// 模型目录分页
const catalogCurrentPage = ref(1)
const catalogPageSize = ref(20)

// 选中模型的详细数据
const selectedModelProviders = ref<ModelProviderDisplay[]>([])
const loadingModelProviders = ref(false)

// 批量添加关联提供商
const batchAddProvidersDialogOpen = ref(false)
const submittingBatchProviders = ref(false)
const providerOptions = ref<ProviderWithEndpointsSummary[]>([])
const loadingProviderOptions = ref(false)

// 单列勾选模式所需状态
const batchProviderSearchQuery = ref('')
const selectedBatchProviderIds = ref<Set<string>>(new Set())
const initialBatchProviderIds = ref<Set<string>>(new Set())

// 编辑提供商模型
const editProviderDialogOpen = ref(false)
const editingProvider = ref<ModelProviderDisplay | null>(null)

// 批量管理全局模型
const batchManageDialogOpen = ref(false)
const batchManageSearchQuery = ref('')
const batchPricingProviderId = ref(REMEMBERED_PRICING_PROVIDER_ID)
const selectedBatchManageModelIds = ref<Set<string>>(new Set())
const submittingBatchManage = ref(false)
const batchManageAction = ref<'sync-prices' | 'delete' | null>(null)

// 将 provider 数据转换为 Model 类型供 ProviderModelFormDialog 使用
const editingProviderModel = computed<Model | null>(() => {
  if (!editingProvider.value) return null

  const p = editingProvider.value

  return {
    id: p.model_id,
    provider_id: p.id,
    provider_model_name: p.target_model || '',
    // 使用 API 返回的完整阶梯配置
    tiered_pricing: null,  // 原始配置为空（继承模式）
    effective_tiered_pricing: p.effective_tiered_pricing,  // 有效配置（含继承）
    price_per_request: p.price_per_request,
    supports_streaming: p.supports_streaming,
    supports_vision: p.supports_vision,
    supports_function_calling: p.supports_function_calling,
    supports_extended_thinking: p.supports_extended_thinking,
    supports_embedding: p.supports_embedding,
    is_active: p.is_active,
    global_model_display_name: selectedModel.value?.display_name,
  } as Model
})

// 使用全局确认对话框
const { confirm, confirmDanger } = useConfirm()

// 从 GlobalModel 的 default_tiered_pricing 获取第一阶梯价格
function getFirstTierPrice(model: GlobalModelResponse, type: 'input' | 'output'): number | null {
  const tiered = model.default_tiered_pricing
  if (!tiered?.tiers?.length) return null
  const firstTier = tiered.tiers[0]
  if (type === 'input') {
    return firstTier.input_price_per_1m || null
  }
  return firstTier.output_price_per_1m || null
}

// 检测是否有阶梯计费（多于一个阶梯）
function hasTieredPricing(model: GlobalModelResponse): boolean {
  const tiered = model.default_tiered_pricing
  return (tiered?.tiers?.length || 0) > 1
}

// 检测是否有生效的视频计费配置（按秒或按 token）
function hasVideoPricing(model: GlobalModelResponse): boolean {
  return hasVideoPricingConfigured(model.config)
}

function videoPricePerSecondEntries(model: GlobalModelResponse): [string, number][] {
  return sortResolutionEntries(Object.entries(getVideoPricePerSecondTable(model.config)))
}

/** 一条 token 单价的紧凑写法：有输入价时写成 `in/out`，否则只写输出价。 */
function formatTokenPricePair(entry: VideoTokenPriceEntry, digits: number): string {
  const output = entry.outputPricePer1m
  const input = entry.inputPricePer1m
  if (input !== null && output !== null) {
    return `$${input.toFixed(digits)}/$${output.toFixed(digits)}`
  }
  const single = output ?? input
  return single === null ? '' : `$${single.toFixed(digits)}`
}

// 获取视频计费的显示文本（如：720p $0.1/s [2种]、720p $15.00/M [2种]）
function getVideoPricingDisplay(model: GlobalModelResponse): string {
  // 按 token 计费时视频价格来自 billing.video，而不是模型的 token 阶梯目录，
  // 所以这一列必须自己呈现，否则整行会退化成「-」。
  if (getVideoBillingMode(model.config) === 'per_token') {
    const entries = getVideoTokenPriceEntries(model.config)
    if (entries.length === 0) return ''
    const first = `${entries[0].resolution} ${formatTokenPricePair(entries[0], 2)}/M`
    return entries.length > 1 ? `${first} [${entries.length}种]` : first
  }

  const entries = videoPricePerSecondEntries(model)
  const defaultPrice = getVideoDefaultPricePerSecond(model.config)
  if (entries.length === 0) {
    return defaultPrice === null ? '' : `默认 $${defaultPrice.toFixed(2)}/s`
  }
  // 获取最低分辨率和价格
  const [firstRes, firstPrice] = entries[0]
  const priceStr = `${firstRes} $${firstPrice.toFixed(2)}/s`
  // 如果有多个分辨率，添加标记
  const variants = entries.length + (defaultPrice === null ? 0 : 1)
  if (variants > 1) {
    return `${priceStr} [${variants}种]`
  }
  return priceStr
}

// 获取视频计费详情的 tooltip
function getVideoPricingTooltip(model: GlobalModelResponse): string {
  if (getVideoBillingMode(model.config) === 'per_token') {
    return getVideoTokenPriceEntries(model.config)
      .map(entry => `${entry.resolution}: ${formatTokenPricePair(entry, 4)}/M`)
      .join('\n')
  }

  const lines = videoPricePerSecondEntries(model)
    .map(([res, price]) => `${res}: $${price.toFixed(4)}/s`)
  const defaultPrice = getVideoDefaultPricePerSecond(model.config)
  if (defaultPrice !== null) {
    lines.push(`默认: $${defaultPrice.toFixed(4)}/s`)
  }
  return lines.join('\n')
}

// 检测是否有对话框打开（防止误关闭抽屉）
const hasBlockingDialogOpen = computed(() =>
  createModelDialogOpen.value ||
  batchAddProvidersDialogOpen.value ||
  editProviderDialogOpen.value ||
  batchManageDialogOpen.value
)

// 能力筛选
const capabilityFilters = ref({
  streaming: false,
  imageGeneration: false,
  vision: false,
  toolUse: false,
  extendedThinking: false,
})

// 过滤后的提供商列表
const filteredBatchProviders = computed(() => {
  const query = batchProviderSearchQuery.value.toLowerCase().trim()
  return providerOptions.value.filter(p => {
    if (query && !p.name.toLowerCase().includes(query)) {
      return false
    }
    return true
  })
})

// 检查提供商是否已选中
function isBatchProviderSelected(providerId: string): boolean {
  return selectedBatchProviderIds.value.has(providerId)
}

// 是否全选
const isAllBatchProvidersSelected = computed(() => {
  if (filteredBatchProviders.value.length === 0) return false
  return filteredBatchProviders.value.every(p => isBatchProviderSelected(p.id))
})

// 计算待添加的提供商
const batchProvidersToAdd = computed(() => {
  const toAdd: string[] = []
  for (const id of selectedBatchProviderIds.value) {
    if (!initialBatchProviderIds.value.has(id)) {
      toAdd.push(id)
    }
  }
  return toAdd
})

// 计算待移除的提供商
const batchProvidersToRemove = computed(() => {
  const toRemove: string[] = []
  for (const id of initialBatchProviderIds.value) {
    if (!selectedBatchProviderIds.value.has(id)) {
      toRemove.push(id)
    }
  }
  return toRemove
})

// 是否有变更
const hasBatchProviderChanges = computed(() => {
  return batchProvidersToAdd.value.length > 0 || batchProvidersToRemove.value.length > 0
})

// 待变更数量
const batchProviderPendingChangesCount = computed(() => {
  return batchProvidersToAdd.value.length + batchProvidersToRemove.value.length
})

// 切换提供商选择
function toggleBatchProviderSelection(providerId: string) {
  if (selectedBatchProviderIds.value.has(providerId)) {
    selectedBatchProviderIds.value.delete(providerId)
  } else {
    selectedBatchProviderIds.value.add(providerId)
  }
  selectedBatchProviderIds.value = new Set(selectedBatchProviderIds.value)
}

// 全选/取消全选
function toggleAllBatchProviders() {
  const allIds = filteredBatchProviders.value.map(p => p.id)
  if (isAllBatchProvidersSelected.value) {
    for (const id of allIds) {
      selectedBatchProviderIds.value.delete(id)
    }
  } else {
    for (const id of allIds) {
      selectedBatchProviderIds.value.add(id)
    }
  }
  selectedBatchProviderIds.value = new Set(selectedBatchProviderIds.value)
}

// 同步初始选择状态
function syncBatchProviderSelection() {
  const existingIds = new Set(selectedModelProviders.value.map((p) => p.id))
  selectedBatchProviderIds.value = new Set(existingIds)
  initialBatchProviderIds.value = new Set(existingIds)
}

// 保存变更
async function saveBatchProviderChanges() {
  if (!hasBatchProviderChanges.value || submittingBatchProviders.value || !selectedModel.value) return

  submittingBatchProviders.value = true
  try {
    let totalSuccess = 0
    const allErrors: string[] = []

    // 并行移除提供商
    if (batchProvidersToRemove.value.length > 0) {
      const { deleteModel } = await import('@/api/endpoints')
      const removePromises = batchProvidersToRemove.value.map(async (providerId) => {
        const existingProvider = selectedModelProviders.value.find((p) => p.id === providerId)
        if (existingProvider && existingProvider.model_id) {
          return deleteModel(providerId, existingProvider.model_id)
        }
        return null
      })

      const results = await Promise.allSettled(removePromises)
      for (const result of results) {
        if (result.status === 'fulfilled' && result.value !== null) {
          totalSuccess++
        } else if (result.status === 'rejected') {
          allErrors.push(parseApiError(result.reason, '移除失败'))
        }
      }
    }

    // 添加提供商
    if (batchProvidersToAdd.value.length > 0) {
      const result = await batchAssignToProviders(selectedModel.value.id, {
        provider_ids: batchProvidersToAdd.value,
        create_models: true
      })
      totalSuccess += result.success.length
      if (result.errors.length > 0) {
        allErrors.push(...result.errors.map(e => e.error))
      }
    }

    if (totalSuccess > 0) {
      success(`成功处理 ${totalSuccess} 个提供商`)
    }

    if (allErrors.length > 0) {
      showError(`部分操作失败: ${allErrors.slice(0, 3).join(', ')}${allErrors.length > 3 ? '...' : ''}`, '警告')
    }

    // 刷新数据并关闭对话框
    await loadModelProviders(selectedModel.value.id)
    await loadGlobalModels()
    // 刷新路由数据
    modelDetailDrawerRef.value?.refreshRoutingData?.()
    closeBatchAddProvidersDialog()
  } catch (err: unknown) {
    showError(parseApiError(err, '保存失败'), '错误')
  } finally {
    submittingBatchProviders.value = false
  }
}

// 筛选后的模型列表
const filteredGlobalModels = computed(() => {
  let result = globalModels.value

  // 能力筛选
  if (capabilityFilters.value.streaming) {
    result = result.filter(m => m.config?.streaming !== false)
  }
  if (capabilityFilters.value.imageGeneration) {
    result = result.filter(m => m.config?.image_generation === true)
  }
  if (capabilityFilters.value.vision) {
    result = result.filter(m => m.config?.vision === true)
  }
  if (capabilityFilters.value.toolUse) {
    result = result.filter(m => m.config?.function_calling === true)
  }
  if (capabilityFilters.value.extendedThinking) {
    result = result.filter(m => m.config?.extended_thinking === true)
  }

  return result
})

// 模型目录分页计算
const paginatedGlobalModels = computed(() => filteredGlobalModels.value)

watch(searchQuery, () => {
  catalogCurrentPage.value = 1
})

watch(catalogPageSize, () => {
  catalogCurrentPage.value = 1
})

const globalModelsQueryParams = computed(() => ({
  skip: Math.max(0, (catalogCurrentPage.value - 1) * catalogPageSize.value),
  limit: catalogPageSize.value,
  search: searchQuery.value.trim() || undefined,
}))

let modelSearchDebounceTimer: ReturnType<typeof setTimeout> | null = null

async function loadGlobalModels(options: { cacheTtlMs?: number } = {}) {
  const requestId = ++globalModelsRequestId
  loading.value = true
  try {
    const response = await listGlobalModels(globalModelsQueryParams.value, {
      cacheTtlMs: options.cacheTtlMs ?? 0,
    })
    if (requestId !== globalModelsRequestId) return

    const pageModels = response.models || []
    const total = typeof response.total === 'number' ? response.total : pageModels.length
    const totalPages = Math.max(1, Math.ceil(total / Math.max(catalogPageSize.value, 1)))
    if (total > 0 && catalogCurrentPage.value > totalPages) {
      catalogCurrentPage.value = totalPages
      return
    }

    globalModels.value = pageModels
    totalGlobalModels.value = total
  } catch (err: unknown) {
    if (requestId !== globalModelsRequestId) return
    log.error('加载模型失败:', err)
    showError(parseApiError(err, '加载模型失败'), '加载模型失败')
  } finally {
    if (requestId === globalModelsRequestId) {
      loading.value = false
    }
  }
}

async function loadBatchManageModels() {
  const requestId = ++batchManageModelsRequestId
  batchManageLoading.value = true
  try {
    const allModels: GlobalModelResponse[] = []
    let skip = 0
    let expectedTotal: number | null = null

    while (true) {
      const response = await listGlobalModels({
        skip,
        limit: GLOBAL_MODELS_BATCH_FETCH_PAGE_SIZE,
      })
      if (expectedTotal === null && typeof response.total === 'number') {
        expectedTotal = response.total
      }
      const pageModels = response.models || []
      allModels.push(...pageModels)

      if (expectedTotal !== null && allModels.length >= expectedTotal) {
        break
      }
      if (pageModels.length < GLOBAL_MODELS_BATCH_FETCH_PAGE_SIZE) {
        break
      }

      skip += pageModels.length
    }

    if (requestId !== batchManageModelsRequestId) return
    batchManageModels.value = allModels
  } catch (err: unknown) {
    if (requestId !== batchManageModelsRequestId) return
    log.error('加载批量管理模型失败:', err)
    showError(parseApiError(err, '加载模型失败'), '加载模型失败')
  } finally {
    if (requestId === batchManageModelsRequestId) {
      batchManageLoading.value = false
    }
  }
}

// 使用复用的行点击逻辑
const { handleMouseDown, shouldTriggerRowClick } = useRowClick()

// 处理行点击，如果用户选择了文字则不触发抽屉
function handleRowClick(event: MouseEvent, model: GlobalModelResponse) {
  if (!shouldTriggerRowClick(event)) return
  selectModel(model)
}

async function selectModel(model: GlobalModelResponse) {
  const requestId = ++modelSelectionRequestId
  // 先显示缓存数据，提升响应速度
  selectedModel.value = model
  detailTab.value = 'basic'

  // 并行加载最新模型数据和关联提供商
  const [latestModel] = await Promise.all([
    getGlobalModel(model.id).catch(err => {
      log.error('获取最新模型数据失败:', err)
      return null
    }),
    loadModelProviders(model.id)
  ])

  // 更新为最新数据（如果获取成功）
  if (requestId !== modelSelectionRequestId) return
  if (latestModel) {
    selectedModel.value = latestModel
  }
}

// 刷新当前选中的模型数据
async function refreshSelectedModel() {
  if (!selectedModel.value) return
  try {
    const updated = await getGlobalModel(selectedModel.value.id)
    selectedModel.value = updated
  } catch (err) {
    log.error('刷新模型数据失败:', err)
  }
}

// 加载指定模型的关联提供商
async function loadModelProviders(_globalModelId: string) {
  const requestId = ++modelProvidersRequestId
  loadingModelProviders.value = true
  try {
    // 使用新的 API 获取所有关联提供商（包括非活跃的）
    const response = await getGlobalModelProviders(_globalModelId)
    if (requestId !== modelProvidersRequestId) return

    // 转换为展示格式
    selectedModelProviders.value = response.providers.map(p => ({
      id: p.provider_id,
      model_id: p.model_id,
      name: p.provider_name,
      provider_type: 'API',
      target_model: p.target_model,
      is_active: p.is_active,
      // 价格信息
      input_price_per_1m: p.input_price_per_1m,
      output_price_per_1m: p.output_price_per_1m,
      cache_creation_price_per_1m: p.cache_creation_price_per_1m,
      cache_read_price_per_1m: p.cache_read_price_per_1m,
      cache_1h_creation_price_per_1m: p.cache_1h_creation_price_per_1m,
      price_per_request: p.price_per_request,
      effective_tiered_pricing: p.effective_tiered_pricing,
      tier_count: p.tier_count,
      // 能力信息
      supports_vision: p.supports_vision,
      supports_function_calling: p.supports_function_calling,
      supports_streaming: p.supports_streaming,
      supports_embedding: p.supports_embedding
    }))
  } catch (err: unknown) {
    if (requestId !== modelProvidersRequestId) return
    log.error('加载关联提供商失败:', err)
    showError(parseApiError(err, '加载关联提供商失败'), '错误')
    selectedModelProviders.value = []
  } finally {
    if (requestId === modelProvidersRequestId) {
      loadingModelProviders.value = false
    }
  }
}

// 确保 Provider 选项已加载
async function ensureProviderOptions() {
  if (providerOptions.value.length > 0 || loadingProviderOptions.value) {
    return
  }
  if (providerOptionsRequest) {
    await providerOptionsRequest
    return
  }
  providerOptionsRequest = (async () => {
    try {
      loadingProviderOptions.value = true
      providerOptions.value = (await getProvidersSummary({ page_size: 9999 })).items
    } catch (err: unknown) {
      const message = parseApiError(err, '加载 Provider 列表失败')
      showError(message, '错误')
    } finally {
      loadingProviderOptions.value = false
    }
  })()
  try {
    await providerOptionsRequest
  } finally {
    providerOptionsRequest = null
  }
}

// 打开添加关联提供商对话框
function openAddProviderDialog() {
  if (!selectedModel.value) return
  batchProviderSearchQuery.value = ''
  batchAddProvidersDialogOpen.value = true
  ensureProviderOptions().then(() => {
    // 同步选择状态
    syncBatchProviderSelection()
  })
}

// 关联指定提供商到当前模型
async function linkProviderToModel(providerId: string) {
  await linkProvidersToModel([providerId])
}

// 批量关联提供商到当前模型
async function linkProvidersToModel(providerIds: string[]) {
  if (!selectedModel.value || providerIds.length === 0) return

  try {
    const result = await batchAssignToProviders(selectedModel.value.id, {
      provider_ids: providerIds,
      create_models: true
    })

    // 显示关联结果
    if (result.errors.length > 0) {
      showError(`${result.errors.length} 个提供商关联失败`, '部分失败')
    } else {
      success(`${providerIds.length} 个提供商已关联`)
    }

    // 刷新数据
    await loadModelProviders(selectedModel.value.id)
    await loadGlobalModels()
    modelDetailDrawerRef.value?.refreshRoutingData?.()
  } catch (err: unknown) {
    showError(parseApiError(err, '关联失败'), '错误')
  }
}

// 处理批量添加 Provider 对话框关闭事件
function handleBatchAddProvidersDialogUpdate(value: boolean) {
  // 只有在不处于提交状态时才允许关闭
  if (!value && submittingBatchProviders.value) {
    return
  }
  batchAddProvidersDialogOpen.value = value
}

// 关闭批量添加对话框
function closeBatchAddProvidersDialog() {
  batchAddProvidersDialogOpen.value = false
  batchProviderSearchQuery.value = ''
  selectedBatchProviderIds.value = new Set()
  initialBatchProviderIds.value = new Set()
  submittingBatchProviders.value = false
}

// 批量管理全局模型 - 过滤
const filteredBatchManageModels = computed(() => {
  const query = batchManageSearchQuery.value.toLowerCase().trim()
  if (!query) return batchManageModels.value
  return batchManageModels.value.filter(m => {
    const searchableText = `${m.name} ${m.display_name || ''}`.toLowerCase()
    return searchableText.includes(query)
  })
})

const batchPricingProviderOptions = computed(() => {
  const existingModelNames = new Set(batchManageModels.value.map(model => model.name.trim().toLowerCase()))
  const providers = new Map<string, {
    providerId: string
    providerName: string
    matchCount: number
    official: boolean
  }>()
  for (const onlineModel of batchManageOnlineModels.value) {
    const provider = providers.get(onlineModel.providerId) ?? {
      providerId: onlineModel.providerId,
      providerName: onlineModel.providerName,
      matchCount: 0,
      official: onlineModel.official === true,
    }
    if (
      (onlineModel.tieredPricing || onlineModel.pricingUnsupportedFields?.length)
      && existingModelNames.has(onlineModel.modelId.trim().toLowerCase())
    ) {
      provider.matchCount += 1
    }
    providers.set(onlineModel.providerId, provider)
  }
  return [...providers.values()]
    .filter(provider => provider.matchCount > 0)
    .sort((left, right) => (
      Number(right.official) - Number(left.official)
      || right.matchCount - left.matchCount
      || left.providerName.localeCompare(right.providerName)
    ))
})

const selectedBatchPricingProvider = computed(() => (
  batchPricingProviderOptions.value.find(provider => provider.providerId === batchPricingProviderId.value)
))

const batchPricingProviderModels = computed(() => (
  batchPricingProviderId.value === REMEMBERED_PRICING_PROVIDER_ID
    ? batchManageOnlineModels.value
    : batchManageOnlineModels.value.filter(model => model.providerId === batchPricingProviderId.value)
))

const rememberedBatchPricingProviderIds = computed(() => {
  if (batchPricingProviderId.value !== REMEMBERED_PRICING_PROVIDER_ID) return undefined
  const providerIds = new Map<string, string>()
  for (const model of batchManageModels.value) {
    const source = getModelsDevPricingSource(model.id)
    if (source) providerIds.set(model.id, source.provider_id)
  }
  return providerIds
})

const batchPriceSyncPlan = computed(() => (
  buildGlobalModelPriceSyncPlan(
    batchManageModels.value,
    batchPricingProviderModels.value,
    rememberedBatchPricingProviderIds.value,
  )
))

const batchPricingStateByModelId = computed(() => {
  const states = new Map<string, 'syncable' | 'unchanged' | 'unsupported' | 'unavailable'>()
  for (const entry of batchPriceSyncPlan.value.syncable) states.set(entry.model.id, 'syncable')
  for (const entry of batchPriceSyncPlan.value.unchanged) states.set(entry.model.id, 'unchanged')
  for (const entry of batchPriceSyncPlan.value.unsupported) states.set(entry.model.id, 'unsupported')
  for (const model of batchPriceSyncPlan.value.unavailable) states.set(model.id, 'unavailable')
  return states
})

const selectedBatchManageModels = computed(() => (
  batchManageModels.value.filter(model => selectedBatchManageModelIds.value.has(model.id))
))

const selectedBatchPriceSyncPlan = computed(() => (
  buildGlobalModelPriceSyncPlan(
    selectedBatchManageModels.value,
    batchPricingProviderModels.value,
    rememberedBatchPricingProviderIds.value,
  )
))

const batchManageSelectionSummary = computed(() => {
  const selectedCount = selectedBatchManageModelIds.value.size
  if (selectedCount === 0) return '选择模型后执行批量操作'
  const plan = selectedBatchPriceSyncPlan.value
  return `已选择 ${selectedCount} 个 · 可更新 ${plan.syncable.length} · 已一致 ${plan.unchanged.length} · 不兼容 ${plan.unsupported.length} · 无在线价格 ${plan.unavailable.length}`
})

function getBatchPricingState(model: GlobalModelResponse) {
  return batchPricingStateByModelId.value.get(model.id) ?? 'unavailable'
}

function getBatchPricingStateLabel(model: GlobalModelResponse): string {
  const state = getBatchPricingState(model)
  if (state === 'syncable') return '价格可更新'
  if (state === 'unchanged') return '价格一致'
  if (state === 'unsupported') return '计价不兼容'
  return '无在线价格'
}

function getBatchPricingStateDescription(model: GlobalModelResponse): string {
  const providerName = getBatchPricingSourceLabel(model)
  const unsupportedEntry = batchPriceSyncPlan.value.unsupported.find(entry => entry.model.id === model.id)
  if (unsupportedEntry) {
    return `${providerName || unsupportedEntry.onlineModel.providerName} · 不支持独立结算 ${formatBatchUnsupportedPricingFields(unsupportedEntry.onlineModel)}`
  }
  return `${providerName} · ${getBatchPricingStateLabel(model)}`
}

function getBatchPricingSourceLabel(model: GlobalModelResponse): string {
  if (batchPricingProviderId.value !== REMEMBERED_PRICING_PROVIDER_ID) {
    return selectedBatchPricingProvider.value?.providerName ?? '未选择来源'
  }
  return getModelsDevPricingSource(model.id)?.provider_name ?? '未记录来源'
}

function getBatchPricingStateClass(model: GlobalModelResponse): string {
  const state = getBatchPricingState(model)
  if (state === 'syncable') return 'text-amber-700 dark:text-amber-300'
  if (state === 'unchanged') return 'text-emerald-700 dark:text-emerald-300'
  if (state === 'unsupported') return 'text-rose-700 dark:text-rose-300'
  return 'text-muted-foreground'
}

function getBatchPricingStateDotClass(model: GlobalModelResponse): string {
  const state = getBatchPricingState(model)
  if (state === 'syncable') return 'bg-amber-500'
  if (state === 'unchanged') return 'bg-emerald-500'
  if (state === 'unsupported') return 'bg-rose-500'
  return 'bg-muted-foreground/45'
}

function formatBatchUnsupportedPricingFields(model: ModelsDevModelItem): string {
  const labels = {
    reasoning: '推理 Token',
    input_audio: '输入音频 Token',
    output_audio: '输出音频 Token',
  }
  return (model.pricingUnsupportedFields ?? []).map(field => labels[field]).join('、')
}

// 批量管理 - 快捷筛选定义
function hasNoPrice(m: GlobalModelResponse): boolean {
  return !getFirstTierPrice(m, 'input') && !getFirstTierPrice(m, 'output')
    && !m.default_price_per_request
}

const batchManageShortcuts = computed(() => {
  const models = batchManageModels.value
  const defs: {
    label: string
    description: string
    filter: (m: GlobalModelResponse) => boolean
    emphasis?: boolean
  }[] = [
    { label: '价格可更新', description: '当前价格与所选供应商在线价格不同', filter: m => getBatchPricingState(m) === 'syncable', emphasis: true },
    { label: '价格一致', description: '当前价格与所选供应商在线价格一致', filter: m => getBatchPricingState(m) === 'unchanged' },
    { label: '计价不兼容', description: '在线来源包含当前计费引擎无法独立结算的价格维度', filter: m => getBatchPricingState(m) === 'unsupported' },
    { label: '无在线价格', description: '所选供应商没有该模型的在线价格', filter: m => getBatchPricingState(m) === 'unavailable' },
    { label: '无提供商', description: '没有关联任何提供商的模型', filter: m => (m.provider_count || 0) === 0 },
    { label: '无活跃提供商', description: '有提供商但没有活跃提供商的模型', filter: m => (m.active_provider_count || 0) === 0 && (m.provider_count || 0) > 0 },
    { label: '禁用', description: '被禁用的模型', filter: m => !m.is_active },
    { label: '未调用', description: '没有调用记录的模型', filter: m => (m.usage_count || 0) === 0 },
    { label: '无价格', description: '没有配置任何价格的模型', filter: m => hasNoPrice(m) },
  ]
  return defs.map(d => ({ ...d, count: models.filter(d.filter).length }))
})

// 批量管理 - 应用快捷选中
function applyBatchManageShortcut(filter: (m: GlobalModelResponse) => boolean) {
  const matchedIds = batchManageModels.value.filter(filter).map(m => m.id)
  selectedBatchManageModelIds.value = new Set(matchedIds)
}

// 批量管理 - 是否全选
const isAllBatchManageModelsSelected = computed(() => {
  if (filteredBatchManageModels.value.length === 0) return false
  return filteredBatchManageModels.value.every(m => selectedBatchManageModelIds.value.has(m.id))
})

// 批量管理 - 切换单个选择
function toggleBatchManageModelSelection(modelId: string) {
  if (selectedBatchManageModelIds.value.has(modelId)) {
    selectedBatchManageModelIds.value.delete(modelId)
  } else {
    selectedBatchManageModelIds.value.add(modelId)
  }
  selectedBatchManageModelIds.value = new Set(selectedBatchManageModelIds.value)
}

// 批量管理 - 全选/取消全选
function toggleAllBatchManageModels() {
  const allIds = filteredBatchManageModels.value.map(m => m.id)
  if (isAllBatchManageModelsSelected.value) {
    for (const id of allIds) {
      selectedBatchManageModelIds.value.delete(id)
    }
  } else {
    for (const id of allIds) {
      selectedBatchManageModelIds.value.add(id)
    }
  }
  selectedBatchManageModelIds.value = new Set(selectedBatchManageModelIds.value)
}

// 打开批量管理对话框
function openBatchManageDialog() {
  batchManageSearchQuery.value = ''
  batchPricingProviderId.value = REMEMBERED_PRICING_PROVIDER_ID
  selectedBatchManageModelIds.value = new Set()
  batchManageDialogOpen.value = true
  void Promise.all([loadBatchManageModels(), loadBatchManageOnlineModels()])
}

async function loadBatchManageOnlineModels() {
  batchManageOnlineLoading.value = true
  try {
    batchManageOnlineModels.value = await getModelsDevList(false)
  } catch (err: unknown) {
    log.error('加载在线模型价格失败:', err)
    showError(parseApiError(err, '加载在线模型价格失败'), '加载失败')
  } finally {
    batchManageOnlineLoading.value = false
  }
}

async function runBatchTasksWithConcurrency(
  tasks: Array<() => Promise<void>>,
  concurrency: number = 6,
) {
  let cursor = 0
  const runNext = async (): Promise<void> => {
    while (cursor < tasks.length) {
      const taskIndex = cursor++
      await tasks[taskIndex]()
    }
  }
  await Promise.all(Array.from(
    { length: Math.min(concurrency, tasks.length) },
    () => runNext(),
  ))
}

async function confirmBatchSyncPrices() {
  const plan = selectedBatchPriceSyncPlan.value
  if (plan.syncable.length === 0) return
  const providerName = batchPricingProviderId.value === REMEMBERED_PRICING_PROVIDER_ID
    ? '各模型上次选择的提供商'
    : selectedBatchPricingProvider.value?.providerName || '所选供应商'
  const skippedCount = plan.unchanged.length + plan.unsupported.length + plan.unavailable.length
  const confirmed = await confirm({
    title: '批量同步模型价格',
    message: `将根据 ${providerName} 的在线定价更新 ${plan.syncable.length} 个模型。${skippedCount > 0 ? `另有 ${skippedCount} 个模型因价格一致、计价不兼容或无在线价格而跳过。` : ''}\n\n仅更新模型价格，不修改名称、能力或其他配置。`,
    confirmText: '同步价格',
    variant: 'info',
  })
  if (!confirmed) return

  submittingBatchManage.value = true
  batchManageAction.value = 'sync-prices'
  const failedIds = new Set<string>()
  const failureMessages: string[] = []
  let successCount = 0
  try {
    const tasks = plan.syncable.map(entry => async () => {
      try {
        const onlinePricing = entry.onlineModel.tieredPricing
        if (!onlinePricing) {
          throw new Error('在线目录未提供价格配置')
        }
        const pricing = cloneTieredPricingConfig(onlinePricing)
        await updateGlobalModel(entry.model.id, { default_tiered_pricing: pricing })
        entry.model.default_tiered_pricing = pricing
        setModelsDevPricingSource(entry.model.id, {
          provider_id: entry.onlineModel.providerId,
          provider_name: entry.onlineModel.providerName,
        })
        successCount += 1
      } catch (err: unknown) {
        failedIds.add(entry.model.id)
        failureMessages.push(`${entry.model.display_name}: ${parseApiError(err, '更新失败')}`)
      }
    })
    await runBatchTasksWithConcurrency(tasks)

    if (successCount > 0) success(`成功同步 ${successCount} 个模型价格`)
    if (failureMessages.length > 0) {
      showError(`${failureMessages.length} 个模型同步失败：${failureMessages.slice(0, 2).join('；')}`, '部分失败')
    }
    await Promise.all([loadGlobalModels(), loadBatchManageModels()])
    selectedBatchManageModelIds.value = failedIds
  } finally {
    batchManageAction.value = null
    submittingBatchManage.value = false
  }
}

// 确认批量删除模型
async function confirmBatchDeleteModels() {
  const count = selectedBatchManageModelIds.value.size
  if (count === 0) return

  const confirmed = await confirmDanger(
    `确定删除选中的 ${count} 个模型吗？\n\n此操作不可撤销。`,
    '批量删除模型'
  )
  if (!confirmed) return

  submittingBatchManage.value = true
  batchManageAction.value = 'delete'
  try {
    const ids = Array.from(selectedBatchManageModelIds.value)
    const result = await batchDeleteGlobalModels(ids)

    if (result.success_count > 0) {
      success(`成功删除 ${result.success_count} 个模型`)
    }
    if (result.failed.length > 0) {
      showError(`${result.failed.length} 个模型删除失败`, '部分失败')
    }

    // 清除选中的已删除模型
    if (selectedModel.value && selectedBatchManageModelIds.value.has(selectedModel.value.id)) {
      selectedModel.value = null
    }

    selectedBatchManageModelIds.value = new Set()
    await Promise.all([loadGlobalModels(), loadBatchManageModels()])
  } catch (err: unknown) {
    showError(parseApiError(err, '批量删除失败'), '错误')
  } finally {
    batchManageAction.value = null
    submittingBatchManage.value = false
  }
}

watch(batchPricingProviderId, (value, previousValue) => {
  if (previousValue && value !== previousValue) {
    selectedBatchManageModelIds.value = new Set()
  }
})

watch([batchPricingProviderOptions, batchManageDialogOpen], ([options, dialogOpen]) => {
  if (!dialogOpen) return
  if (
    batchPricingProviderId.value !== REMEMBERED_PRICING_PROVIDER_ID
    && !options.some(provider => provider.providerId === batchPricingProviderId.value)
  ) {
    batchPricingProviderId.value = REMEMBERED_PRICING_PROVIDER_ID
  }
}, { immediate: true })

// 抽屉控制函数
function handleDrawerOpenChange(value: boolean) {
  if (!value && !hasBlockingDialogOpen.value) {
    modelSelectionRequestId += 1
    modelProvidersRequestId += 1
    selectedModel.value = null
  }
}

// 编辑提供商模型
function openEditProviderImplementation(provider: ModelProviderDisplay) {
  editingProvider.value = provider
  editProviderDialogOpen.value = true
}

// 处理编辑 Provider 对话框关闭事件
function handleEditProviderDialogUpdate(value: boolean) {
  editProviderDialogOpen.value = value
  if (!value) {
    editingProvider.value = null
  }
}

// 编辑提供商模型保存成功后刷新列表
async function handleEditProviderSaved() {
  if (selectedModel.value) {
    await loadModelProviders(selectedModel.value.id)
  }
}

// 切换关联提供商状态
async function toggleProviderStatus(provider: ModelProviderDisplay) {
  if (!provider.model_id) {
    showError('缺少模型 ID')
    return
  }

  try {
    const { updateModel } = await import('@/api/endpoints')
    const newStatus = !provider.is_active
    await updateModel(provider.id, provider.model_id, { is_active: newStatus })
    provider.is_active = newStatus
    success(newStatus ? '已启用此关联提供商' : '已停用此关联提供商')
    // 刷新路由数据
    modelDetailDrawerRef.value?.refreshRoutingData?.()
  } catch (err: unknown) {
    showError(parseApiError(err, '更新状态失败'))
  }
}

// 删除关联提供商
async function confirmDeleteProviderImplementation(provider: ModelProviderDisplay) {
  if (!provider.model_id) {
    showError('缺少模型 ID')
    return
  }

  const confirmed = await confirmDanger(
    `确定要删除 ${provider.name} 的模型关联吗？\n\n此操作不可恢复！`,
    '删除关联提供商'
  )
  if (!confirmed) return

  try {
    const { deleteModel } = await import('@/api/endpoints')
    await deleteModel(provider.id, provider.model_id)
    success(`已删除 ${provider.name} 的模型实现`)
    // 同步更新 selectedModelProviders 确保状态一致
    if (selectedModel.value) {
      await loadModelProviders(selectedModel.value.id)
    }
    // 刷新路由数据
    modelDetailDrawerRef.value?.refreshRoutingData?.()
  } catch (err: unknown) {
    showError(parseApiError(err, '删除模型失败'))
  }
}

function openCreateModelDialog() {
  editingModel.value = null
  createModelDialogOpen.value = true
}

// 处理模型对话框关闭事件
function handleModelDialogUpdate(value: boolean) {
  createModelDialogOpen.value = value
  if (!value) {
    editingModel.value = null
  }
}

// 处理模型表单提交成功
async function handleModelFormSuccess() {
  // 编辑模式关闭对话框，创建模式保持打开以便连续添加
  if (editingModel.value) {
    createModelDialogOpen.value = false
    editingModel.value = null
  }
  await loadGlobalModels()
}

async function editModel(model: GlobalModelResponse) {
  editingModel.value = model
  createModelDialogOpen.value = true
}

function handleModelPricingSynced(model: GlobalModelResponse) {
  const updatePricing = (models: GlobalModelResponse[]) => {
    const current = models.find(entry => entry.id === model.id)
    if (current) {
      current.default_tiered_pricing = cloneTieredPricingConfig(model.default_tiered_pricing)
    }
  }
  updatePricing(globalModels.value)
  updatePricing(batchManageModels.value)
  if (editingModel.value?.id === model.id) {
    editingModel.value.default_tiered_pricing = cloneTieredPricingConfig(model.default_tiered_pricing)
  }
  if (selectedModel.value?.id === model.id) {
    selectedModel.value.default_tiered_pricing = cloneTieredPricingConfig(model.default_tiered_pricing)
  }
}

async function deleteModel(model: GlobalModelResponse) {
  const confirmed = await confirmDanger(
    `确定删除模型 "${model.name}" 吗？\n\n此操作不可撤销。`,
    '删除模型'
  )
  if (!confirmed) return

  try {
    await deleteGlobalModel(model.id)
    success('模型删除成功')
    if (selectedModel.value?.id === model.id) {
      selectedModel.value = null
    }
    await loadGlobalModels()
  } catch (err: unknown) {
    showError(parseApiError(err, '删除失败'), '删除失败')
  }
}

async function toggleModelStatus(model: GlobalModelResponse) {
  try {
    await updateGlobalModel(model.id, { is_active: !model.is_active })
    model.is_active = !model.is_active
    success(model.is_active ? '模型已启用' : '模型已停用')
  } catch (err: unknown) {
    showError(parseApiError(err, '操作失败'), '操作失败')
  }
}

async function refreshData() {
  await loadGlobalModels()
}

watch(globalModelsQueryParams, (newParams, oldParams) => {
  if (modelSearchDebounceTimer) clearTimeout(modelSearchDebounceTimer)
  const isSearchOnly = newParams.search !== oldParams?.search
    && newParams.skip === oldParams?.skip
    && newParams.limit === oldParams?.limit
  if (isSearchOnly) {
    modelSearchDebounceTimer = setTimeout(() => {
      void loadGlobalModels({ cacheTtlMs: GLOBAL_MODELS_LIST_CACHE_TTL_MS })
    }, 300)
  } else {
    void loadGlobalModels({ cacheTtlMs: GLOBAL_MODELS_LIST_CACHE_TTL_MS })
  }
}, { immediate: true })

onBeforeUnmount(() => {
  if (modelSearchDebounceTimer) clearTimeout(modelSearchDebounceTimer)
  globalModelsRequestId += 1
  batchManageModelsRequestId += 1
  modelSelectionRequestId += 1
  modelProvidersRequestId += 1
})
</script>

<style scoped>
/* 抽屉过渡动画 */
.drawer-enter-active,
.drawer-leave-active {
  transition: opacity 0.3s ease;
}

.drawer-enter-active .relative,
.drawer-leave-active .relative {
  transition: transform 0.3s ease;
}

.drawer-enter-from,
.drawer-leave-to {
  opacity: 0;
}

.drawer-enter-from .relative {
  transform: translateX(100%);
}

.drawer-leave-to .relative {
  transform: translateX(100%);
}

.drawer-enter-to .relative,
.drawer-leave-from .relative {
  transform: translateX(0);
}
</style>
