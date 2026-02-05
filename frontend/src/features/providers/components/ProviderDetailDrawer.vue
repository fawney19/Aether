<template>
  <!-- 自定义抽屉 -->
  <Teleport to="body">
    <Transition name="drawer">
      <div
        v-if="open && (loading || provider)"
        class="fixed inset-0 z-50 flex justify-end"
        @click.self="handleBackdropClick"
      >
        <!-- 背景遮罩 -->
        <div
          class="absolute inset-0 bg-black/30 backdrop-blur-sm"
          @click="handleBackdropClick"
        />

        <!-- 抽屉内容 -->
        <Card class="relative h-full w-full sm:w-[700px] sm:max-w-[90vw] rounded-none shadow-2xl overflow-y-auto">
          <!-- 加载状态 -->
          <div
            v-if="loading"
            class="flex items-center justify-center py-12"
          >
            <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
          </div>

          <template v-else-if="provider">
            <!-- 头部:名称 + 快捷操作 -->
            <div class="sticky top-0 z-10 bg-background border-b px-4 sm:px-6 pt-4 sm:pt-6 pb-3 sm:pb-3">
              <div class="flex items-center justify-between gap-x-3 sm:gap-x-4 flex-wrap">
                <div class="flex items-center gap-2 min-w-0">
                  <h2 class="text-lg sm:text-xl font-bold truncate">
                    {{ provider.name }}
                  </h2>
                  <!-- 网站图标 -->
                  <a
                    v-if="provider.website"
                    :href="provider.website"
                    target="_blank"
                    rel="noopener noreferrer"
                    class="text-muted-foreground hover:text-primary transition-colors shrink-0"
                    :title="provider.website"
                  >
                    <ExternalLink class="w-4 h-4" />
                  </a>
                  <Badge
                    :variant="provider.is_active ? 'default' : 'secondary'"
                    class="text-xs shrink-0"
                  >
                    {{ provider.is_active ? '活跃' : '已停用' }}
                  </Badge>
                </div>
                <div class="flex items-center gap-1 shrink-0">
                  <span :title="systemFormatConversionEnabled ? '请先关闭系统级开关' : (provider.enable_format_conversion ? '已启用格式转换（点击关闭）' : '启用格式转换')">
                    <Button
                      variant="ghost"
                      size="icon"
                      :class="`${provider.enable_format_conversion ? 'text-primary' : ''} ${systemFormatConversionEnabled ? 'opacity-50' : ''}`"
                      :disabled="systemFormatConversionEnabled"
                      @click="toggleFormatConversion"
                    >
                      <Shuffle class="w-4 h-4" />
                    </Button>
                  </span>
                  <Button
                    variant="ghost"
                    size="icon"
                    title="编辑提供商"
                    @click="$emit('edit', provider)"
                  >
                    <Edit class="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    :title="provider.is_active ? '点击停用' : '点击启用'"
                    @click="$emit('toggleStatus', provider)"
                  >
                    <Power class="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    title="关闭"
                    @click="handleClose"
                  >
                    <X class="w-4 h-4" />
                  </Button>
                </div>
              </div>
              <!-- 描述（独占整行，紧贴名称行下方） -->
              <div class="-mt-0.5">
                <span
                  v-if="!editingDescription"
                  class="text-xs text-muted-foreground truncate block cursor-pointer hover:text-foreground transition-colors"
                  :title="provider.description || '点击添加描述'"
                  @click="startEditDescription"
                >{{ provider.description || '添加描述...' }}</span>
                <input
                  v-else
                  ref="descriptionInputRef"
                  v-model="editingDescriptionValue"
                  type="text"
                  class="text-xs px-1.5 py-0.5 border rounded bg-background focus:outline-none focus:ring-1 focus:ring-primary w-full"
                  placeholder="输入描述..."
                  @keydown="handleDescriptionKeydown"
                  @blur="saveDescription"
                >
              </div>
              <!-- 端点 API 格式 -->
              <div class="flex items-center gap-1.5 flex-wrap mt-3">
                <template
                  v-for="endpoint in endpoints"
                  :key="endpoint.id"
                >
                  <span
                    class="text-xs px-2 py-0.5 rounded-md border border-border bg-background hover:bg-accent hover:border-accent-foreground/20 cursor-pointer transition-colors font-medium"
                    :class="{ 'opacity-40': !endpoint.is_active }"
                    :title="`编辑 ${API_FORMAT_LABELS[endpoint.api_format]} 端点`"
                    @click="handleEditEndpoint(endpoint)"
                  >{{ API_FORMAT_LABELS[endpoint.api_format] || endpoint.api_format }}</span>
                </template>
                <span
                  v-if="endpoints.length > 0"
                  class="text-xs px-2 py-0.5 rounded-md border border-dashed border-border hover:bg-accent hover:border-accent-foreground/20 cursor-pointer transition-colors text-muted-foreground"
                  title="编辑端点"
                  @click="showAddEndpointDialog"
                >编辑</span>
                <Button
                  v-else
                  variant="outline"
                  size="sm"
                  class="h-7 text-xs"
                  @click="showAddEndpointDialog"
                >
                  <Plus class="w-3 h-3 mr-1" />
                  添加 API 端点
                </Button>
              </div>
            </div>

            <div class="space-y-6 p-4 sm:p-6">
              <!-- 配额使用情况 -->
              <Card
                v-if="provider.billing_type === 'monthly_quota' && provider.monthly_quota_usd"
                class="p-4"
              >
                <div class="space-y-3">
                  <div class="flex items-center justify-between">
                    <h3 class="text-sm font-semibold">
                      订阅配额
                    </h3>
                    <Badge
                      variant="secondary"
                      class="text-xs"
                    >
                      {{ ((provider.monthly_used_usd || 0) / provider.monthly_quota_usd * 100).toFixed(1) }}%
                    </Badge>
                  </div>
                  <div class="relative w-full h-2 bg-border rounded-full overflow-hidden">
                    <div
                      class="absolute left-0 top-0 h-full transition-all duration-300"
                      :class="{
                        'bg-green-500': (provider.monthly_used_usd || 0) / provider.monthly_quota_usd < 0.7,
                        'bg-yellow-500': (provider.monthly_used_usd || 0) / provider.monthly_quota_usd >= 0.7 && (provider.monthly_used_usd || 0) / provider.monthly_quota_usd < 0.9,
                        'bg-red-500': (provider.monthly_used_usd || 0) / provider.monthly_quota_usd >= 0.9
                      }"
                      :style="{ width: `${Math.min((provider.monthly_used_usd || 0) / provider.monthly_quota_usd * 100, 100)}%` }"
                    />
                  </div>
                  <div class="flex items-center justify-between text-xs">
                    <span class="font-semibold">
                      ${{ (provider.monthly_used_usd || 0).toFixed(2) }} / ${{ provider.monthly_quota_usd.toFixed(2) }}
                    </span>
                    <span
                      v-if="provider.quota_reset_day"
                      class="text-muted-foreground"
                    >
                      每月 {{ provider.quota_reset_day }} 号重置
                    </span>
                  </div>
                </div>
              </Card>

              <!-- 密钥管理 -->
              <Card class="overflow-hidden">
                <div class="p-4 border-b border-border/60">
                  <div class="flex items-center justify-between">
                    <h3 class="text-sm font-semibold">
                      {{ provider.provider_type === 'custom' ? '密钥管理' : '账号管理' }}
                    </h3>
                    <div class="flex items-center gap-2">
                      <!-- Codex 刷新限额按钮 -->
                      <Button
                        v-if="provider.provider_type === 'codex' && allKeys.length > 0"
                        variant="outline"
                        size="sm"
                        class="h-8"
                        :disabled="refreshingQuota"
                        title="刷新所有账号的限额信息"
                        @click="handleRefreshQuota"
                      >
                        <RefreshCw
                          class="w-3.5 h-3.5 mr-1.5"
                          :class="{ 'animate-spin': refreshingQuota }"
                        />
                        刷新限额
                      </Button>
                      <Button
                        v-if="endpoints.length > 0"
                        variant="outline"
                        size="sm"
                        class="h-8"
                        @click="handleAddKeyToFirstEndpoint"
                      >
                        <Plus class="w-3.5 h-3.5 mr-1.5" />
                        {{ provider.provider_type === 'custom' ? '添加密钥' : '添加账号' }}
                      </Button>
                    </div>
                  </div>
                </div>

                <!-- 密钥列表 -->
                <div
                  v-if="allKeys.length > 0"
                  class="divide-y divide-border/40"
                >
                  <div
                    v-for="({ key, endpoint }, index) in allKeys"
                    :key="key.id"
                    class="px-4 py-2.5 hover:bg-muted/30 transition-colors group/item"
                    :class="{
                      'opacity-50': keyDragState.isDragging && keyDragState.draggedIndex === index,
                      'bg-primary/5 border-l-2 border-l-primary': keyDragState.targetIndex === index && keyDragState.isDragging,
                      'opacity-40 bg-muted/20': !key.is_active
                    }"
                    draggable="true"
                    @dragstart="handleKeyDragStart($event, index)"
                    @dragend="handleKeyDragEnd"
                    @dragover="handleKeyDragOver($event, index)"
                    @dragleave="handleKeyDragLeave"
                    @drop="handleKeyDrop($event, index)"
                  >
                    <!-- 第一行：名称 + 状态 + 操作按钮 -->
                    <div class="flex items-center justify-between gap-2">
                      <div class="flex items-center gap-2 flex-1 min-w-0">
                        <!-- 拖拽手柄 -->
                        <div class="cursor-grab active:cursor-grabbing text-muted-foreground/30 group-hover/item:text-muted-foreground transition-colors shrink-0">
                          <GripVertical class="w-4 h-4" />
                        </div>
                        <div class="flex flex-col min-w-0">
                          <div class="flex items-center gap-1.5">
                            <span class="text-sm font-medium truncate">{{ key.name || '未命名密钥' }}</span>
                            <!-- OAuth 订阅类型标签 -->
                            <Badge
                              v-if="key.oauth_plan_type"
                              variant="outline"
                              class="text-[10px] px-1.5 py-0 shrink-0"
                              :class="getOAuthPlanTypeClass(key.oauth_plan_type)"
                            >
                              {{ formatOAuthPlanType(key.oauth_plan_type) }}
                            </Badge>
                          </div>
                          <div class="flex items-center gap-1">
                            <span class="text-[11px] font-mono text-muted-foreground">
                              {{ key.auth_type === 'oauth' ? '[Refresh Token]' : (key.auth_type === 'vertex_ai' ? 'Vertex AI' : key.api_key_masked) }}
                            </span>
                            <Button
                              variant="ghost"
                              size="icon"
                              class="h-4 w-4 shrink-0"
                              :title="key.auth_type === 'oauth' ? '复制 Refresh Token' : '复制密钥'"
                              @click.stop="copyFullKey(key)"
                            >
                              <Copy class="w-2.5 h-2.5" />
                            </Button>
                            <!-- OAuth 状态（失效/过期/倒计时）和刷新按钮 -->
                            <template v-if="getKeyOAuthExpires(key)">
                              <span
                                class="text-[10px]"
                                :class="{
                                  'text-destructive': getKeyOAuthExpires(key)?.isInvalid || getKeyOAuthExpires(key)?.isExpired,
                                  'text-warning': getKeyOAuthExpires(key)?.isExpiringSoon && !getKeyOAuthExpires(key)?.isExpired && !getKeyOAuthExpires(key)?.isInvalid,
                                  'text-muted-foreground': !getKeyOAuthExpires(key)?.isExpired && !getKeyOAuthExpires(key)?.isExpiringSoon && !getKeyOAuthExpires(key)?.isInvalid
                                }"
                                :title="getOAuthStatusTitle(key)"
                              >
                                {{ getKeyOAuthExpires(key)?.text }}
                              </span>
                              <Button
                                variant="ghost"
                                size="icon"
                                class="h-4 w-4 shrink-0"
                                :disabled="refreshingOAuthKeyId === key.id"
                                :title="getKeyOAuthExpires(key)?.isInvalid ? '重新授权' : '刷新 Token'"
                                @click.stop="handleRefreshOAuth(key)"
                              >
                                <RefreshCw
                                  class="w-2.5 h-2.5"
                                  :class="{ 'animate-spin': refreshingOAuthKeyId === key.id }"
                                />
                              </Button>
                            </template>
                          </div>
                        </div>
                      </div>
                      <!-- 并发 + 健康度 + 操作按钮 -->
                      <div class="flex items-center gap-1 shrink-0">
                        <!-- 熔断徽章 -->
                        <Badge
                          v-if="key.circuit_breaker_open"
                          variant="destructive"
                          class="text-[10px] px-1.5 py-0 shrink-0"
                        >
                          熔断
                        </Badge>
                        <!-- 健康度 -->
                        <div
                          v-if="key.health_score !== undefined"
                          class="flex items-center gap-1 mr-1"
                        >
                          <div class="w-10 h-1.5 bg-border rounded-full overflow-hidden">
                            <div
                              class="h-full transition-all duration-300"
                              :class="getHealthScoreBarColor(key.health_score || 0)"
                              :style="{ width: `${(key.health_score || 0) * 100}%` }"
                            />
                          </div>
                          <span
                            class="text-[10px] font-medium tabular-nums"
                            :class="getHealthScoreColor(key.health_score || 0)"
                          >
                            {{ ((key.health_score || 0) * 100).toFixed(0) }}%
                          </span>
                        </div>
                        <Button
                          v-if="key.circuit_breaker_open || (key.health_score !== undefined && key.health_score < 0.5)"
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 text-green-600"
                          title="刷新健康状态"
                          @click="handleRecoverKey(key)"
                        >
                          <RefreshCw class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          v-if="key.auth_type !== 'oauth'"
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="模型权限"
                          @click="handleKeyPermissions(key)"
                        >
                          <Shield class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          v-if="key.auth_type !== 'oauth'"
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="编辑密钥"
                          @click="handleEditKey(endpoint, key)"
                        >
                          <Edit class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          :disabled="togglingKeyId === key.id"
                          :title="key.is_active ? '点击停用' : '点击启用'"
                          @click="toggleKeyActive(key)"
                        >
                          <Power class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="删除密钥"
                          @click="handleDeleteKey(key)"
                        >
                          <Trash2 class="w-3.5 h-3.5" />
                        </Button>
                      </div>
                    </div>
                    <!-- Codex 上游额度信息（仅当有元数据时显示） -->
                    <div
                      v-if="key.upstream_metadata && hasCodexQuotaData(key.upstream_metadata)"
                      class="mt-2 p-2 bg-muted/30 rounded-md"
                    >
                      <!-- 限额并排显示 -->
                      <div class="grid grid-cols-2 gap-3">
                        <!-- 周限额（7天窗口） -->
                        <div v-if="key.upstream_metadata.primary_used_percent !== undefined">
                          <div class="flex items-center justify-between text-[10px] mb-0.5">
                            <span class="text-muted-foreground">周限额</span>
                            <span :class="getQuotaRemainingClass(key.upstream_metadata.primary_used_percent)">
                              {{ (100 - key.upstream_metadata.primary_used_percent).toFixed(1) }}%
                            </span>
                          </div>
                          <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                            <div
                              class="absolute left-0 top-0 h-full transition-all duration-300"
                              :class="getQuotaRemainingBarColor(key.upstream_metadata.primary_used_percent)"
                              :style="{ width: `${Math.max(100 - key.upstream_metadata.primary_used_percent, 0)}%` }"
                            />
                          </div>
                          <div
                            v-if="key.upstream_metadata.primary_reset_seconds"
                            class="text-[9px] text-muted-foreground/70 mt-0.5"
                          >
                            {{ formatResetTime(key.upstream_metadata.primary_reset_seconds) }}后重置
                          </div>
                        </div>
                        <!-- 5小时限额 -->
                        <div v-if="key.upstream_metadata.secondary_used_percent !== undefined">
                          <div class="flex items-center justify-between text-[10px] mb-0.5">
                            <span class="text-muted-foreground">5H限额</span>
                            <span :class="getQuotaRemainingClass(key.upstream_metadata.secondary_used_percent)">
                              {{ (100 - key.upstream_metadata.secondary_used_percent).toFixed(1) }}%
                            </span>
                          </div>
                          <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                            <div
                              class="absolute left-0 top-0 h-full transition-all duration-300"
                              :class="getQuotaRemainingBarColor(key.upstream_metadata.secondary_used_percent)"
                              :style="{ width: `${Math.max(100 - key.upstream_metadata.secondary_used_percent, 0)}%` }"
                            />
                          </div>
                          <div class="text-[9px] text-muted-foreground/70 mt-0.5">
                            <template v-if="key.upstream_metadata.secondary_reset_seconds">
                              {{ formatResetTime(key.upstream_metadata.secondary_reset_seconds) }}后重置
                            </template>
                            <template v-else>
                              已重置
                            </template>
                          </div>
                        </div>
                      </div>
                    </div>
                    <!-- 第二行：优先级 + API 格式（展开显示） + 统计信息 -->
                    <div class="flex items-center gap-1.5 mt-1 text-[11px] text-muted-foreground">
                      <!-- 优先级放最前面，支持点击编辑 -->
                      <span
                        v-if="editingPriorityKey !== key.id"
                        title="点击编辑优先级"
                        class="font-medium text-foreground/80 cursor-pointer hover:text-primary hover:underline"
                        @click="startEditPriority(key)"
                      >P{{ key.internal_priority }}</span>
                      <input
                        v-else
                        ref="priorityInputRef"
                        v-model="editingPriorityValue"
                        type="text"
                        inputmode="numeric"
                        pattern="[0-9]*"
                        class="w-8 h-5 px-1 text-[11px] text-center border rounded bg-background focus:outline-none focus:ring-1 focus:ring-primary font-medium text-foreground/80"
                        @keydown="(e) => handlePriorityKeydown(e, key)"
                        @blur="handlePriorityBlur(key)"
                      >
                      <!-- 自动获取模型状态 -->
                      <template v-if="key.auto_fetch_models">
                        <span class="text-muted-foreground/40">|</span>
                        <span
                          class="cursor-help"
                          :class="key.last_models_fetch_error ? 'text-amber-600 dark:text-amber-400' : ''"
                          :title="getAutoFetchStatusTitle(key)"
                        >
                          {{ key.last_models_fetch_error ? '同步失败' : '自动同步' }}
                        </span>
                      </template>
                      <!-- RPM 限制信息（第二位） -->
                      <template v-if="key.rpm_limit || key.is_adaptive">
                        <span class="text-muted-foreground/40">|</span>
                        <span v-if="key.is_adaptive">
                          {{ key.learned_rpm_limit != null ? `${key.learned_rpm_limit}` : '探测中' }} RPM
                          <span class="text-muted-foreground/60">(自适应)</span>
                        </span>
                        <span v-else>{{ key.rpm_limit }} RPM</span>
                      </template>
                      <span class="text-muted-foreground/40">|</span>
                      <!-- API 格式：展开显示每个格式、倍率、熔断状态 -->
                      <template
                        v-for="(format, idx) in getKeyApiFormats(key, endpoint)"
                        :key="format"
                      >
                        <span
                          v-if="idx > 0"
                          class="text-muted-foreground/40"
                        >/</span>
                        <span :class="{ 'text-destructive': isFormatCircuitOpen(key, format) }">
                          {{ API_FORMAT_SHORT[format] || format }}
                        </span>
                        <span
                          v-if="editingMultiplierKey !== key.id || editingMultiplierFormat !== format"
                          title="点击编辑倍率"
                          class="cursor-pointer hover:text-primary hover:underline"
                          :class="{ 'text-destructive': isFormatCircuitOpen(key, format) }"
                          @click="startEditMultiplier(key, format)"
                        >{{ getKeyRateMultiplier(key, format) }}x</span>
                        <input
                          v-else
                          ref="multiplierInputRef"
                          v-model="editingMultiplierValue"
                          type="text"
                          inputmode="decimal"
                          pattern="[0-9]*\.?[0-9]*"
                          class="w-10 h-5 px-1 text-[11px] text-center border rounded bg-background focus:outline-none focus:ring-1 focus:ring-primary font-medium text-foreground/80"
                          @keydown="(e) => handleMultiplierKeydown(e, key, format)"
                          @blur="handleMultiplierBlur(key, format)"
                        >
                        <span
                          v-if="getFormatProbeCountdown(key, format)"
                          :class="{ 'text-destructive': isFormatCircuitOpen(key, format) }"
                        >{{ getFormatProbeCountdown(key, format) }}</span>
                      </template>
                    </div>
                  </div>
                </div>

                <!-- 空状态 -->
                <div
                  v-else
                  class="p-8 text-center text-muted-foreground"
                >
                  <Key class="w-12 h-12 mx-auto mb-3 opacity-50" />
                  <p class="text-sm">
                    {{ provider.provider_type === 'custom' ? '暂无密钥配置' : '暂无账号配置' }}
                  </p>
                  <p class="text-xs mt-1">
                    {{ endpoints.length > 0
                      ? (provider.provider_type === 'custom' ? '点击上方"添加密钥"按钮创建第一个密钥' : '点击上方"添加账号"按钮添加第一个账号')
                      : '请先添加端点，然后再添加密钥' }}
                  </p>
                </div>
              </Card>

              <!-- 模型查看 -->
              <ModelsTab
                v-if="provider"
                ref="modelsTabRef"
                :key="`models-${provider.id}`"
                :provider="provider"
                :endpoints="endpoints"
                @edit-model="handleEditModel"
                @batch-assign="handleBatchAssign"
              />

              <!-- 模型映射 -->
              <ModelMappingTab
                v-if="provider"
                ref="modelMappingTabRef"
                :key="`mapping-${provider.id}`"
                :provider="provider"
                :provider-keys="providerKeys"
                @refresh="handleModelMappingChanged"
              />
            </div>
          </template>
        </Card>
      </div>
    </Transition>
  </Teleport>

  <!-- 端点表单对话框（管理/编辑） -->
  <EndpointFormDialog
    v-if="provider && open"
    v-model="endpointDialogOpen"
    :provider="provider"
    :endpoints="endpoints"
    :system-format-conversion-enabled="systemFormatConversionEnabled"
    :provider-format-conversion-enabled="provider.enable_format_conversion"
    @endpoint-created="handleEndpointChanged"
    @endpoint-updated="handleEndpointChanged"
  />

  <!-- 密钥编辑对话框 -->
  <KeyFormDialog
    v-if="open"
    :open="keyFormDialogOpen"
    :endpoint="currentEndpoint"
    :editing-key="editingKey"
    :provider-id="provider ? provider.id : null"
    :provider-type="provider?.provider_type || null"
    :available-api-formats="provider?.api_formats || []"
    @close="keyFormDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- OAuth 账号对话框 -->
  <OAuthAccountDialog
    v-if="open && provider"
    :open="oauthAccountDialogOpen"
    :provider-id="provider.id"
    @close="oauthAccountDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- 模型权限对话框 -->
  <KeyAllowedModelsEditDialog
    v-if="open"
    :open="keyPermissionsDialogOpen"
    :api-key="editingKey"
    :provider-id="providerId || ''"
    @close="keyPermissionsDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- 删除密钥确认对话框 -->
  <AlertDialog
    v-if="open"
    :model-value="deleteKeyConfirmOpen"
    title="删除密钥"
    :description="`确定要删除密钥 ${keyToDelete?.api_key_masked} 吗？`"
    confirm-text="删除"
    cancel-text="取消"
    type="danger"
    @update:model-value="deleteKeyConfirmOpen = $event"
    @confirm="confirmDeleteKey"
    @cancel="deleteKeyConfirmOpen = false"
  />

  <!-- 添加/编辑模型对话框 -->
  <ProviderModelFormDialog
    v-if="open && provider"
    :open="modelFormDialogOpen"
    :provider-id="provider.id"
    :provider-name="provider.name"
    :editing-model="editingModel"
    @update:open="modelFormDialogOpen = $event"
    @saved="handleModelSaved"
  />

  <!-- 批量关联模型对话框 -->
  <BatchAssignModelsDialog
    v-if="open && provider"
    :open="batchAssignDialogOpen"
    :provider-id="provider.id"
    :provider-name="provider.name"
    @update:open="batchAssignDialogOpen = $event"
    @changed="handleBatchAssignChanged"
  />
</template>

<script setup lang="ts">
import { ref, watch, computed, nextTick } from 'vue'
import {
  Plus,
  Key,
  Edit,
  Trash2,
  RefreshCw,
  X,
  Power,
  GripVertical,
  Copy,
  Shield,
  Shuffle,
  ExternalLink
} from 'lucide-vue-next'
import { useEscapeKey } from '@/composables/useEscapeKey'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Card from '@/components/ui/card.vue'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import { useCountdownTimer, formatCountdown, getOAuthExpiresCountdown } from '@/composables/useCountdownTimer'
import { getProvider, getProviderEndpoints, updateProvider } from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import {
  KeyFormDialog,
  KeyAllowedModelsEditDialog,
  ModelsTab,
  BatchAssignModelsDialog,
  OAuthAccountDialog
} from '@/features/providers/components'
import ModelMappingTab from '@/features/providers/components/provider-tabs/ModelMappingTab.vue'
import EndpointFormDialog from '@/features/providers/components/EndpointFormDialog.vue'
import ProviderModelFormDialog from '@/features/providers/components/ProviderModelFormDialog.vue'
import AlertDialog from '@/components/common/AlertDialog.vue'
import {
  deleteEndpointKey,
  recoverKeyHealth,
  getProviderKeys,
  updateProviderKey,
  revealEndpointKey,
  refreshProviderOAuth,
  refreshProviderQuota,
  type ProviderEndpoint,
  type EndpointAPIKey,
  type Model,
  API_FORMAT_LABELS,
  API_FORMAT_ORDER,
  API_FORMAT_SHORT,
  sortApiFormats,
} from '@/api/endpoints'

// 扩展端点类型,包含密钥列表
interface ProviderEndpointWithKeys extends ProviderEndpoint {
  keys?: EndpointAPIKey[]
  rpm_limit?: number
}

interface Props {
  providerId: string | null
  open: boolean
}

const props = defineProps<Props>()
const emit = defineEmits<{
  (e: 'update:open', value: boolean): void
  (e: 'edit', provider: any): void
  (e: 'toggleStatus', provider: any): void
  (e: 'refresh'): void
}>()

const { error: showError, success: showSuccess } = useToast()
const { copyToClipboard } = useClipboard()
const { tick: countdownTick, start: startCountdownTimer, stop: stopCountdownTimer } = useCountdownTimer()

const loading = ref(false)
const provider = ref<any>(null)
const endpoints = ref<ProviderEndpointWithKeys[]>([])
const providerKeys = ref<EndpointAPIKey[]>([])  // Provider 级别的 keys

// 系统级格式转换配置
const systemFormatConversionEnabled = ref(false)

// 端点相关状态
const endpointDialogOpen = ref(false)

// 密钥相关状态
const keyFormDialogOpen = ref(false)
const keyPermissionsDialogOpen = ref(false)
const oauthAccountDialogOpen = ref(false)
const currentEndpoint = ref<ProviderEndpoint | null>(null)
const editingKey = ref<EndpointAPIKey | null>(null)
const deleteKeyConfirmOpen = ref(false)
const keyToDelete = ref<EndpointAPIKey | null>(null)
const togglingKeyId = ref<string | null>(null)

// 密钥显示状态：key_id -> 完整密钥
const revealedKeys = ref<Map<string, string>>(new Map())

// 模型相关状态
const modelFormDialogOpen = ref(false)
const editingModel = ref<Model | null>(null)
const batchAssignDialogOpen = ref(false)
const modelsTabRef = ref<InstanceType<typeof ModelsTab> | null>(null)
const modelMappingTabRef = ref<InstanceType<typeof ModelMappingTab> | null>(null)

// 密钥列表拖拽排序状态
const keyDragState = ref({
  isDragging: false,
  draggedIndex: null as number | null,
  targetIndex: null as number | null
})

// 点击编辑优先级相关状态
const editingPriorityKey = ref<string | null>(null)
const editingPriorityValue = ref<number>(0)
const priorityInputRef = ref<HTMLInputElement[] | null>(null)
const prioritySaving = ref(false)

// OAuth 刷新状态
const refreshingOAuthKeyId = ref<string | null>(null)

// Codex 限额刷新状态
const refreshingQuota = ref(false)

// 描述编辑状态
const editingDescription = ref(false)
const editingDescriptionValue = ref('')
const descriptionInputRef = ref<HTMLInputElement | null>(null)

// 点击编辑倍率相关状态
const editingMultiplierKey = ref<string | null>(null)
const editingMultiplierFormat = ref<string | null>(null)
const editingMultiplierValue = ref<number>(1.0)
const multiplierInputRef = ref<HTMLInputElement[] | null>(null)
const multiplierSaving = ref(false)

// 任意模态窗口打开时,阻止抽屉被误关闭
const hasBlockingDialogOpen = computed(() =>
  endpointDialogOpen.value ||
  keyFormDialogOpen.value ||
  keyPermissionsDialogOpen.value ||
  oauthAccountDialogOpen.value ||
  deleteKeyConfirmOpen.value ||
  modelFormDialogOpen.value ||
  batchAssignDialogOpen.value ||
  modelMappingTabRef.value?.dialogOpen
)

// 所有密钥的扁平列表（带端点信息）
// key 通过 api_formats 字段确定支持的格式，endpoint 可能为 undefined
const allKeys = computed(() => {
  const result: { key: EndpointAPIKey; endpoint?: ProviderEndpointWithKeys }[] = []
  const seenKeyIds = new Set<string>()

  // 1. 先添加 Provider 级别的 keys
  for (const key of providerKeys.value) {
    if (!seenKeyIds.has(key.id)) {
      seenKeyIds.add(key.id)
      // key 没有关联特定 endpoint
      result.push({ key, endpoint: undefined })
    }
  }

  // 2. 再遍历所有端点的 keys（历史数据）
  for (const endpoint of endpoints.value) {
    if (endpoint.keys) {
      for (const key of endpoint.keys) {
        if (!seenKeyIds.has(key.id)) {
          seenKeyIds.add(key.id)
          result.push({ key, endpoint })
        }
      }
    }
  }

  return result
})

// 监听 providerId 变化
watch(() => props.providerId, (newId) => {
  if (newId && props.open) {
    loadProvider()
    loadEndpoints()
  }
}, { immediate: true })

// 监听 open 变化
watch(() => props.open, (newOpen) => {
  if (newOpen && props.providerId) {
    loadProvider()
    loadEndpoints()
    // 启动倒计时定时器
    startCountdownTimer()
  } else if (!newOpen) {
    // 停止倒计时定时器
    stopCountdownTimer()
    // 重置所有状态
    provider.value = null
    endpoints.value = []
    providerKeys.value = []  // 清空 Provider 级别的 keys

    // 重置所有对话框状态
    endpointDialogOpen.value = false
    keyFormDialogOpen.value = false
    keyPermissionsDialogOpen.value = false
    oauthAccountDialogOpen.value = false
    deleteKeyConfirmOpen.value = false
    batchAssignDialogOpen.value = false

    // 重置临时数据
    currentEndpoint.value = null
    editingKey.value = null
    keyToDelete.value = null

    // 清除已显示的密钥（安全考虑）
    revealedKeys.value.clear()
  }
})

// 处理背景点击
function handleBackdropClick() {
  if (!hasBlockingDialogOpen.value) {
    handleClose()
  }
}

// 关闭抽屉
function handleClose() {
  if (!hasBlockingDialogOpen.value) {
    emit('update:open', false)
  }
}

// 切换格式转换开关
async function toggleFormatConversion() {
  if (!provider.value) return
  const newValue = !provider.value.enable_format_conversion
  try {
    await updateProvider(provider.value.id, { enable_format_conversion: newValue })
    provider.value.enable_format_conversion = newValue
    showSuccess(newValue ? '已启用格式转换' : '已禁用格式转换')
    emit('refresh')
  } catch {
    showError('切换格式转换失败')
  }
}

// ===== 描述编辑 =====
function startEditDescription() {
  editingDescription.value = true
  editingDescriptionValue.value = provider.value?.description || ''
  nextTick(() => {
    descriptionInputRef.value?.focus()
    descriptionInputRef.value?.select()
  })
}

function handleDescriptionKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault()
    saveDescription()
  } else if (e.key === 'Escape') {
    e.preventDefault()
    editingDescription.value = false
  }
}

async function saveDescription() {
  if (!editingDescription.value || !provider.value) return
  editingDescription.value = false

  const newDescription = editingDescriptionValue.value.trim()
  // 如果没有变化，直接返回
  if (newDescription === (provider.value.description || '')) return

  try {
    await updateProvider(provider.value.id, { description: newDescription || null })
    provider.value.description = newDescription || null
    showSuccess('描述已更新')
    emit('refresh')
  } catch {
    showError('更新描述失败')
  }
}

// 显示端点管理对话框
function showAddEndpointDialog() {
  endpointDialogOpen.value = true
}

// ===== 端点事件处理 =====
function handleEditEndpoint(_endpoint: ProviderEndpoint) {
  // 点击任何端点都打开管理对话框
  endpointDialogOpen.value = true
}

async function handleEndpointChanged() {
  await Promise.all([loadProvider(), loadEndpoints()])
  emit('refresh')
}

// ===== 密钥事件处理 =====
function handleAddKey(endpoint: ProviderEndpoint) {
  currentEndpoint.value = endpoint
  editingKey.value = null
  keyFormDialogOpen.value = true
}

// 添加密钥/账号（如果有多个端点则添加到第一个）
function handleAddKeyToFirstEndpoint() {
  if (endpoints.value.length === 0) return

  // 非自定义提供商：打开 OAuth 账号对话框
  if (provider.value?.provider_type !== 'custom') {
    oauthAccountDialogOpen.value = true
  } else {
    // 自定义提供商：打开密钥表单对话框
    handleAddKey(endpoints.value[0])
  }
}

function handleEditKey(endpoint: ProviderEndpoint | undefined, key: EndpointAPIKey) {
  currentEndpoint.value = endpoint || null
  editingKey.value = key
  keyFormDialogOpen.value = true
}

function handleKeyPermissions(key: EndpointAPIKey) {
  editingKey.value = key
  keyPermissionsDialogOpen.value = true
}

// 复制完整密钥或认证配置
async function copyFullKey(key: EndpointAPIKey) {
  const cached = revealedKeys.value.get(key.id)
  if (cached) {
    copyToClipboard(cached)
    return
  }

  // 否则先获取再复制
  try {
    const result = await revealEndpointKey(key.id)
    let textToCopy: string

    if (result.auth_type === 'vertex_ai' && result.auth_config) {
      // Vertex AI 类型：复制 auth_config JSON
      textToCopy = typeof result.auth_config === 'string'
        ? result.auth_config
        : JSON.stringify(result.auth_config, null, 2)
    } else {
      // API Key 类型：复制 api_key
      textToCopy = result.api_key || ''
    }

    revealedKeys.value.set(key.id, textToCopy)
    copyToClipboard(textToCopy)
  } catch (err: any) {
    showError(err.response?.data?.detail || '获取密钥失败', '错误')
  }
}

function handleDeleteKey(key: EndpointAPIKey) {
  keyToDelete.value = key
  deleteKeyConfirmOpen.value = true
}

async function confirmDeleteKey() {
  if (!keyToDelete.value) return

  const keyId = keyToDelete.value.id
  deleteKeyConfirmOpen.value = false
  keyToDelete.value = null

  try {
    await deleteEndpointKey(keyId)
    showSuccess('密钥已删除')
    // 并行刷新：端点列表、模型列表、模型映射（删除 Key 触发自动解除模型关联）
    await Promise.all([
      loadEndpoints(),
      modelsTabRef.value?.reload(),
      modelMappingTabRef.value?.reload()
    ])
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '删除密钥失败', '错误')
  }
}

async function handleRecoverKey(key: EndpointAPIKey) {
  try {
    const result = await recoverKeyHealth(key.id)
    showSuccess(result.message || 'Key已完全恢复')
    await loadEndpoints()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || 'Key恢复失败', '错误')
  }
}

async function handleRefreshOAuth(key: EndpointAPIKey) {
  if (refreshingOAuthKeyId.value) return
  refreshingOAuthKeyId.value = key.id
  try {
    const result = await refreshProviderOAuth(key.id)
    showSuccess('Token 刷新成功')
    // 更新本地数据
    const keyInList = providerKeys.value.find(k => k.id === key.id)
    if (keyInList) {
      keyInList.oauth_expires_at = result.expires_at
    }
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || 'Token 刷新失败', '错误')
  } finally {
    refreshingOAuthKeyId.value = null
  }
}

// 刷新 Codex 所有账号限额
async function handleRefreshQuota() {
  if (refreshingQuota.value || !props.providerId) return
  refreshingQuota.value = true
  try {
    const result = await refreshProviderQuota(props.providerId)
    if (result.success > 0) {
      showSuccess(`成功刷新 ${result.success}/${result.total} 个账号的限额`)
      // 重新加载数据以更新 UI
      await loadEndpoints()
    } else if (result.failed > 0) {
      showError(`刷新失败: ${result.results.map(r => r.message).filter(Boolean).join(', ')}`, '错误')
    } else {
      showError('没有获取到限额信息', '警告')
    }
  } catch (err: any) {
    showError(err.response?.data?.detail || '刷新限额失败', '错误')
  } finally {
    refreshingQuota.value = false
  }
}

async function handleKeyChanged() {
  await loadEndpoints()
  // 并行刷新模型列表和模型映射（因为模型权限会影响正则映射预览）
  await Promise.all([
    modelsTabRef.value?.reload(),
    modelMappingTabRef.value?.reload()
  ])
  emit('refresh')
}

// 切换密钥启用状态
async function toggleKeyActive(key: EndpointAPIKey) {
  if (togglingKeyId.value) return

  togglingKeyId.value = key.id
  try {
    const newStatus = !key.is_active
    await updateProviderKey(key.id, { is_active: newStatus })
    key.is_active = newStatus
    showSuccess(newStatus ? '密钥已启用' : '密钥已停用')
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '操作失败', '错误')
  } finally {
    togglingKeyId.value = null
  }
}

// ===== 模型事件处理 =====
// 处理编辑模型
function handleEditModel(model: Model) {
  editingModel.value = model
  modelFormDialogOpen.value = true
}

// 处理打开批量关联对话框
function handleBatchAssign() {
  batchAssignDialogOpen.value = true
}

// 处理批量关联完成
async function handleBatchAssignChanged() {
  await loadProvider()
  emit('refresh')
}

// 处理模型映射变更
async function handleModelMappingChanged() {
  emit('refresh')
}

// 处理模型保存完成
async function handleModelSaved() {
  editingModel.value = null
  await loadProvider()
  emit('refresh')
}

// ===== 点击编辑优先级 =====
function startEditPriority(key: EndpointAPIKey) {
  editingPriorityKey.value = key.id
  editingPriorityValue.value = key.internal_priority ?? 0
  prioritySaving.value = false
  nextTick(() => {
    // v-for 中的 ref 是数组，取第一个元素
    const input = Array.isArray(priorityInputRef.value) ? priorityInputRef.value[0] : priorityInputRef.value
    input?.focus()
    input?.select()
  })
}

function cancelEditPriority() {
  editingPriorityKey.value = null
  prioritySaving.value = false
}

function handlePriorityKeydown(e: KeyboardEvent, key: EndpointAPIKey) {
  if (e.key === 'Enter') {
    e.preventDefault()
    e.stopPropagation()
    if (!prioritySaving.value) {
      prioritySaving.value = true
      savePriority(key)
    }
  } else if (e.key === 'Escape') {
    e.preventDefault()
    cancelEditPriority()
  }
}

function handlePriorityBlur(key: EndpointAPIKey) {
  // 如果已经在保存中（Enter触发），不重复保存
  if (prioritySaving.value) return
  savePriority(key)
}

async function savePriority(key: EndpointAPIKey) {
  const keyId = editingPriorityKey.value
  const newPriority = parseInt(String(editingPriorityValue.value), 10) || 0

  if (!keyId || newPriority < 0) {
    cancelEditPriority()
    return
  }

  // 如果优先级没有变化，直接取消编辑
  if (key.internal_priority === newPriority) {
    cancelEditPriority()
    return
  }

  cancelEditPriority()

  try {
    await updateProviderKey(keyId, { internal_priority: newPriority })
    showSuccess('优先级已更新')
    // 更新本地数据 - 更新 providerKeys 中的数据
    const keyToUpdate = providerKeys.value.find(k => k.id === keyId)
    if (keyToUpdate) {
      keyToUpdate.internal_priority = newPriority
    }
    // 重新排序
    providerKeys.value.sort((a, b) => (a.internal_priority ?? 0) - (b.internal_priority ?? 0))
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '更新优先级失败', '错误')
  }
}

// ===== 点击编辑倍率 =====
function startEditMultiplier(key: EndpointAPIKey, format: string) {
  editingMultiplierKey.value = key.id
  editingMultiplierFormat.value = format
  editingMultiplierValue.value = getKeyRateMultiplier(key, format)
  multiplierSaving.value = false
  nextTick(() => {
    const input = Array.isArray(multiplierInputRef.value) ? multiplierInputRef.value[0] : multiplierInputRef.value
    input?.focus()
    input?.select()
  })
}

function cancelEditMultiplier() {
  editingMultiplierKey.value = null
  editingMultiplierFormat.value = null
  multiplierSaving.value = false
}

function handleMultiplierKeydown(e: KeyboardEvent, key: EndpointAPIKey, format: string) {
  if (e.key === 'Enter') {
    e.preventDefault()
    e.stopPropagation()
    if (!multiplierSaving.value) {
      multiplierSaving.value = true
      saveMultiplier(key, format)
    }
  } else if (e.key === 'Escape') {
    e.preventDefault()
    cancelEditMultiplier()
  }
}

function handleMultiplierBlur(key: EndpointAPIKey, format: string) {
  if (multiplierSaving.value) return
  saveMultiplier(key, format)
}

async function saveMultiplier(key: EndpointAPIKey, format: string) {
  // 防止重复调用
  if (multiplierSaving.value) return
  multiplierSaving.value = true

  const keyId = editingMultiplierKey.value
  const newMultiplier = parseFloat(String(editingMultiplierValue.value))

  // 验证输入有效性
  if (!keyId || isNaN(newMultiplier)) {
    showError('请输入有效的倍率值')
    cancelEditMultiplier()
    return
  }

  // 验证合理范围
  if (newMultiplier <= 0 || newMultiplier > 100) {
    showError('倍率必须在 0.01 到 100 之间')
    cancelEditMultiplier()
    return
  }

  // 如果倍率没有变化,直接取消编辑（使用精度容差比较浮点数）
  const currentMultiplier = getKeyRateMultiplier(key, format)
  if (Math.abs(currentMultiplier - newMultiplier) < 0.0001) {
    cancelEditMultiplier()
    return
  }

  cancelEditMultiplier()

  try {
    // 构建 rate_multipliers 对象
    const rateMultipliers = { ...(key.rate_multipliers || {}) }
    rateMultipliers[format] = newMultiplier

    await updateProviderKey(keyId, { rate_multipliers: rateMultipliers })
    showSuccess('倍率已更新')

    // 更新本地数据
    const keyToUpdate = providerKeys.value.find(k => k.id === keyId)
    if (keyToUpdate) {
      keyToUpdate.rate_multipliers = rateMultipliers
    }
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '更新倍率失败', '错误')
  } finally {
    multiplierSaving.value = false
  }
}

// ===== 密钥列表拖拽排序 =====
function handleKeyDragStart(event: DragEvent, index: number) {
  keyDragState.value.isDragging = true
  keyDragState.value.draggedIndex = index
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', String(index))
  }
}

function handleKeyDragEnd() {
  keyDragState.value.isDragging = false
  keyDragState.value.draggedIndex = null
  keyDragState.value.targetIndex = null
}

function handleKeyDragOver(event: DragEvent, index: number) {
  event.preventDefault()
  if (event.dataTransfer) {
    event.dataTransfer.dropEffect = 'move'
  }
  if (keyDragState.value.draggedIndex !== index) {
    keyDragState.value.targetIndex = index
  }
}

function handleKeyDragLeave() {
  keyDragState.value.targetIndex = null
}

async function handleKeyDrop(event: DragEvent, targetIndex: number) {
  event.preventDefault()

  const draggedIndex = keyDragState.value.draggedIndex
  if (draggedIndex === null || draggedIndex === targetIndex) {
    handleKeyDragEnd()
    return
  }

  const keys = allKeys.value.map(item => item.key)
  if (draggedIndex < 0 || draggedIndex >= keys.length || targetIndex < 0 || targetIndex >= keys.length) {
    handleKeyDragEnd()
    return
  }

  const draggedKey = keys[draggedIndex]
  const targetKey = keys[targetIndex]
  const draggedPriority = draggedKey.internal_priority ?? 0
  const targetPriority = targetKey.internal_priority ?? 0

  // 如果是同组内拖拽（同优先级），忽略操作
  if (draggedPriority === targetPriority) {
    handleKeyDragEnd()
    return
  }

  handleKeyDragEnd()

  try {
    // 记录每个 key 的原始优先级
    const originalPriorityMap = new Map<string, number>()
    keys.forEach(k => {
      originalPriorityMap.set(k.id, k.internal_priority ?? 0)
    })

    // 重排数组：将被拖动项移到目标位置
    const items = [...keys]
    items.splice(draggedIndex, 1)
    items.splice(targetIndex, 0, draggedKey)

    // 按新顺序分配优先级：被拖动项单独成组，其他同组项保持在一起
    const groupNewPriority = new Map<number, number>()
    let currentPriority = 1
    const newPriorityMap = new Map<string, number>()

    items.forEach(key => {
      const originalPriority = originalPriorityMap.get(key.id)!

      if (key === draggedKey) {
        // 被拖动的项单独成组
        newPriorityMap.set(key.id, currentPriority)
        currentPriority++
      } else {
        if (groupNewPriority.has(originalPriority)) {
          // 同组的其他项使用相同的新优先级
          newPriorityMap.set(key.id, groupNewPriority.get(originalPriority)!)
        } else {
          // 新组，分配新优先级
          groupNewPriority.set(originalPriority, currentPriority)
          newPriorityMap.set(key.id, currentPriority)
          currentPriority++
        }
      }
    })

    // 更新所有优先级发生变化的 key
    const updatePromises = keys.map(key => {
      const oldPriority = key.internal_priority ?? 0
      const newPriority = newPriorityMap.get(key.id)
      if (newPriority !== undefined && oldPriority !== newPriority) {
        return updateProviderKey(key.id, { internal_priority: newPriority })
      }
      return Promise.resolve()
    })

    await Promise.all(updatePromises)
    showSuccess('优先级已更新')
    await loadEndpoints()
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '更新优先级失败', '错误')
    await loadEndpoints()
  }
}

// 获取密钥的 API 格式列表（按指定顺序排序）
function getKeyApiFormats(key: EndpointAPIKey, endpoint?: ProviderEndpointWithKeys): string[] {
  let formats: string[] = []
  if (key.api_formats && key.api_formats.length > 0) {
    formats = [...key.api_formats]
  } else if (endpoint) {
    formats = [endpoint.api_format]
  }
  // 使用统一的排序函数
  return sortApiFormats(formats)
}

// 获取密钥在指定 API 格式下的成本倍率
function getKeyRateMultiplier(key: EndpointAPIKey, format: string): number {
  if (key.rate_multipliers && key.rate_multipliers[format] !== undefined) {
    return key.rate_multipliers[format]
  }
  return 1.0
}

// OAuth 订阅类型格式化
function formatOAuthPlanType(planType: string): string {
  const labels: Record<string, string> = {
    plus: 'Plus',
    pro: 'Pro',
    free: 'Free',
    team: 'Team',
    enterprise: 'Enterprise',
  }
  return labels[planType] || planType
}

// Codex 剩余额度样式（基于已用百分比计算剩余）
function getQuotaRemainingClass(usedPercent: number): string {
  const remaining = 100 - usedPercent
  if (remaining <= 10) return 'text-red-600 dark:text-red-400'
  if (remaining <= 30) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-green-600 dark:text-green-400'
}

// Codex 剩余额度进度条颜色
function getQuotaRemainingBarColor(usedPercent: number): string {
  const remaining = 100 - usedPercent
  if (remaining <= 10) return 'bg-red-500 dark:bg-red-400'
  if (remaining <= 30) return 'bg-yellow-500 dark:bg-yellow-400'
  return 'bg-green-500 dark:bg-green-400'
}

// 检查是否有 Codex 额度数据
function hasCodexQuotaData(metadata: any): boolean {
  if (!metadata) return false
  return metadata.primary_used_percent !== undefined ||
         metadata.secondary_used_percent !== undefined ||
         (metadata.has_credits && metadata.credits_balance !== undefined)
}

// 格式化重置时间
function formatResetTime(seconds: number): string {
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)

  if (days > 0) {
    return `${days}天 ${hours}小时`
  }
  if (hours > 0) {
    return `${hours}小时 ${minutes}分钟`
  }
  return `${minutes}分钟`
}

// OAuth 订阅类型样式
function getOAuthPlanTypeClass(planType: string): string {
  const classes: Record<string, string> = {
    plus: 'border-green-500/50 text-green-600 dark:text-green-400',
    pro: 'border-blue-500/50 text-blue-600 dark:text-blue-400',
    free: 'border-primary/50 text-primary',
    team: 'border-purple-500/50 text-purple-600 dark:text-purple-400',
    enterprise: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
  }
  return classes[planType] || ''
}

// OAuth 状态信息（包括失效和过期）
function getKeyOAuthExpires(key: EndpointAPIKey) {
  if (key.auth_type !== 'oauth') return null
  // 即使没有 expires_at，也要检查 invalid_at
  if (!key.oauth_expires_at && !key.oauth_invalid_at) return null
  return getOAuthExpiresCountdown(
    key.oauth_expires_at,
    countdownTick.value,
    key.oauth_invalid_at,
    key.oauth_invalid_reason
  )
}

// OAuth 状态的 title 提示
function getOAuthStatusTitle(key: EndpointAPIKey): string {
  const status = getKeyOAuthExpires(key)
  if (!status) return ''
  if (status.isInvalid) {
    return status.invalidReason ? `Token 已失效: ${status.invalidReason}` : 'Token 已失效'
  }
  if (status.isExpired) {
    return 'Token 已过期，请重新授权'
  }
  return `Token 剩余有效期: ${status.text}`
}

// 健康度颜色
function getHealthScoreColor(score: number): string {
  if (score >= 0.8) return 'text-green-600 dark:text-green-400'
  if (score >= 0.5) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-red-600 dark:text-red-400'
}

function getHealthScoreBarColor(score: number): string {
  if (score >= 0.8) return 'bg-green-500 dark:bg-green-400'
  if (score >= 0.5) return 'bg-yellow-500 dark:bg-yellow-400'
  return 'bg-red-500 dark:bg-red-400'
}

// 获取自动获取模型状态的 title 提示
function getAutoFetchStatusTitle(key: EndpointAPIKey): string {
  const parts: string[] = ['自动获取模型已启用']

  if (key.last_models_fetch_at) {
    const date = new Date(key.last_models_fetch_at)
    parts.push(`上次同步: ${date.toLocaleString()}`)
  }

  if (key.last_models_fetch_error) {
    parts.push(`错误: ${key.last_models_fetch_error}`)
  }

  return parts.join('\n')
}

// 检查指定格式是否熔断
function isFormatCircuitOpen(key: EndpointAPIKey, format: string): boolean {
  if (!key.circuit_breaker_by_format) return false
  const formatData = key.circuit_breaker_by_format[format]
  return formatData?.open === true
}

// 获取指定格式的探测倒计时（如果熔断，返回带空格前缀的倒计时文本）
function getFormatProbeCountdown(key: EndpointAPIKey, format: string): string {
  // 触发响应式更新
  void countdownTick.value

  if (!key.circuit_breaker_by_format) return ''
  const formatData = key.circuit_breaker_by_format[format]
  if (!formatData?.open) return ''

  // 半开状态
  if (formatData.half_open_until) {
    const halfOpenUntil = new Date(formatData.half_open_until)
    const now = new Date()
    if (halfOpenUntil > now) {
      return ' 探测中'
    }
  }
  // 等待探测
  if (formatData.next_probe_at) {
    const nextProbe = new Date(formatData.next_probe_at)
    const now = new Date()
    const diffMs = nextProbe.getTime() - now.getTime()
    if (diffMs > 0) {
      return ` ${formatCountdown(diffMs)}`
    } else {
      return ' 探测中'
    }
  }
  return ''
}

// 加载系统级格式转换配置
async function loadSystemFormatConversionConfig() {
  try {
    const result = await adminApi.getSystemConfig('enable_format_conversion')
    systemFormatConversionEnabled.value = result.value === true
  } catch {
    // 获取失败时默认为关闭
    systemFormatConversionEnabled.value = false
  }
}

// 加载 Provider 信息
async function loadProvider() {
  if (!props.providerId) return

  try {
    loading.value = true
    // 并行加载 Provider 信息和系统级格式转换配置
    const [providerData] = await Promise.all([
      getProvider(props.providerId),
      loadSystemFormatConversionConfig(),
    ])
    provider.value = providerData

    if (!provider.value) {
      throw new Error('Provider 不存在')
    }
  } catch (err: any) {
    showError(err.response?.data?.detail || err.message || '加载失败', '错误')
  } finally {
    loading.value = false
  }
}

// 加载端点列表
async function loadEndpoints() {
  if (!props.providerId) return

  try {
    // 并行加载端点列表和 Provider 级别的 keys
    const [endpointsList, providerKeysResult] = await Promise.all([
      getProviderEndpoints(props.providerId),
      getProviderKeys(props.providerId).catch(() => []),
    ])

    providerKeys.value = providerKeysResult
    // 按 API 格式排序
    endpoints.value = endpointsList.sort((a, b) => {
      const aIdx = API_FORMAT_ORDER.indexOf(a.api_format)
      const bIdx = API_FORMAT_ORDER.indexOf(b.api_format)
      if (aIdx === -1 && bIdx === -1) return 0
      if (aIdx === -1) return 1
      if (bIdx === -1) return -1
      return aIdx - bIdx
    })
  } catch (err: any) {
    showError(err.response?.data?.detail || '加载端点失败', '错误')
  }
}

// 添加 ESC 键监听
useEscapeKey(() => {
  if (props.open) {
    handleClose()
  }
}, {
  disableOnInput: true,
  once: false
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
