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
            <ProviderDetailHeader
              v-model:provider-proxy-popover-open="providerProxyPopoverOpen"
              :provider="provider"
              :endpoints="endpoints"
              :loading-provider-endpoints="loadingProviderEndpoints"
              :system-format-conversion-enabled="systemFormatConversionEnabled"
              :has-failover-rules="hasFailoverRules"
              :provider-proxy-node-name="getProviderProxyNodeName()"
              :saving-provider-proxy="savingProviderProxy"
              @toggle-format-conversion="toggleFormatConversion"
              @open-failover-rules="failoverRulesDialogOpen = true"
              @set-provider-proxy="setProviderProxy"
              @clear-provider-proxy="clearProviderProxy"
              @edit="$emit('edit', $event)"
              @toggle-status="$emit('toggleStatus', $event)"
              @close="handleClose"
              @edit-endpoint="handleEditEndpoint"
              @add-endpoint="showAddEndpointDialog"
            />

            <div class="space-y-6 p-4 sm:p-6">
              <!-- 配额使用情况 -->
              <ProviderMonthlyQuotaCard
                v-if="provider.billing_type === 'monthly_quota' && provider.monthly_quota_usd"
                :used="provider.monthly_used_usd"
                :quota="provider.monthly_quota_usd"
                :reset-day="provider.quota_reset_day"
              />

              <!-- 密钥管理 -->
              <Card class="overflow-hidden">
                <div class="p-4 border-b border-border/60">
                  <div class="flex items-center justify-between">
                    <h3 class="text-sm font-semibold">
                      {{ legacyT(isKeyManagedProviderType(provider.provider_type) ? '密钥管理' : '账号管理') }}
                    </h3>
                    <div class="flex flex-wrap items-center justify-end gap-2">
                      <Button
                        v-if="endpoints.length > 0 && provider.provider_type === 'custom'"
                        variant="outline"
                        size="sm"
                        class="h-9"
                        @click="keyBatchImportDialogOpen = true"
                      >
                        <ListPlus class="mr-1.5 h-3.5 w-3.5" />
                        批量导入
                      </Button>
                      <Button
                        v-if="endpoints.length > 0"
                        variant="outline"
                        size="sm"
                        class="h-9"
                        @click="handleAddKeyToFirstEndpoint"
                      >
                        <Plus class="w-3.5 h-3.5 mr-1.5" />
                        {{ legacyT(isKeyManagedProviderType(provider.provider_type) ? '添加密钥' : '添加账号') }}
                      </Button>
                    </div>
                  </div>
                </div>

                <!-- 密钥列表 -->
                <div
                  v-if="loadingProviderKeys && allKeys.length === 0"
                  class="flex items-center justify-center gap-2 py-12 text-sm text-muted-foreground"
                >
                  <Loader2 class="w-4 h-4 animate-spin" />
                  {{ legacyT('正在加载') }}{{ legacyT(isKeyManagedProviderType(provider.provider_type) ? '密钥' : '账号') }}
                </div>

                <div
                  v-else-if="allKeys.length > 0"
                  class="divide-y divide-border/40"
                  :class="shouldPaginateKeys && 'flex flex-col'"
                >
                  <div
                    v-for="({ key, endpoint }, localIdx) in paginatedKeys"
                    :key="key.id"
                    class="px-4 py-2.5 hover:bg-muted/30 transition-colors group/item"
                    :class="{
                      'opacity-50': keyDragState.isDragging && keyDragState.draggedIndex === getGlobalKeyIndex(localIdx),
                      'bg-primary/5 border-l-2 border-l-primary': keyDragState.targetIndex === getGlobalKeyIndex(localIdx) && keyDragState.isDragging,
                      'opacity-40 bg-muted/20': !key.is_active
                    }"
                    draggable="true"
                    @dragstart="handleKeyDragStart($event, getGlobalKeyIndex(localIdx))"
                    @dragend="handleKeyDragEnd"
                    @dragover="handleKeyDragOver($event, getGlobalKeyIndex(localIdx))"
                    @dragleave="handleKeyDragLeave"
                    @drop="handleKeyDrop($event, getGlobalKeyIndex(localIdx))"
                  >
                    <!-- 第一行：名称 + 状态 + 操作按钮 -->
                    <div class="flex items-center justify-between gap-2">
                      <div class="flex items-center gap-2 flex-1 min-w-0">
                        <!-- 拖拽手柄 -->
                        <div class="cursor-grab active:cursor-grabbing text-muted-foreground/30 group-hover/item:text-muted-foreground transition-colors shrink-0">
                          <GripVertical class="w-4 h-4" />
                        </div>
                        <ProviderKeyIdentityBlock
                          :api-key="key"
                          :masked-secret-label="getProviderMaskedSecretLabel(key, provider.provider_type)"
                          :oauth-plan-label="key.oauth_plan_type ? formatOAuthPlanType(key.oauth_plan_type) : null"
                          :oauth-plan-class="key.oauth_plan_type ? getOAuthPlanTypeClass(key.oauth_plan_type) : ''"
                          :oauth-org-badge="getOAuthOrgBadge(key)"
                          :kiro-subscription-label="shouldShowKiroSubscriptionBadge(key) ? getKiroSubscriptionBadgeLabel(key) : null"
                          :kiro-subscription-class="shouldShowKiroSubscriptionBadge(key) ? getOAuthPlanTypeClass(getKiroSubscriptionBadgeLabel(key)) : ''"
                          :can-export-credential="canExportOAuthCredential(key)"
                          :show-o-auth-refresh-control="shouldShowOAuthRefreshControl(key, provider.provider_type)"
                          :account-level-block="isAccountLevelBlock(key)"
                          :oauth-status="getKeyOAuthExpires(key)"
                          :oauth-status-title="getOAuthStatusTitle(key)"
                          :oauth-refresh-button-title="getOAuthRefreshButtonTitle(key)"
                          :can-refresh-credential="canRefreshOAuthCredential(key)"
                          :clearing-o-auth-invalid="clearingOAuthInvalidKeyId === key.id"
                          :refreshing-o-auth="refreshingOAuthKeyId === key.id"
                          :antigravity-inactive="provider.provider_type === 'antigravity' && key.is_active && isOAuthManagedCredential(key) && !hasAntigravityQuotaDisplayData(key)"
                          @copy-name="copyToClipboard"
                          @download-credential="downloadRefreshToken(key)"
                          @copy-full-key="copyFullKey(key)"
                          @clear-o-auth-invalid="handleClearOAuthInvalid(key)"
                          @refresh-o-auth="handleRefreshOAuth(key)"
                        />
                      </div>
                      <ProviderKeyActionCluster
                        :api-key="key"
                        :provider-type="provider.provider_type"
                        :recoverable="isKeyRecoverable(key)"
                        :recover-title="getRecoverKeyTitle(key)"
                        :circuit-breaker-title="getKeyCircuitBreakerTitle(key)"
                        :circuit-probe-countdown="getKeyCircuitProbeCountdown(key)"
                        :health-score-bar-class="getHealthScoreBarColor(key.health_score || 0)"
                        :health-score-text-class="getHealthScoreColor(key.health_score || 0)"
                        :proxy-popover-open="proxyPopoverOpenKeyId === key.id"
                        :proxy-node-name="getKeyProxyNodeName(key)"
                        :saving-proxy="savingProxyKeyId === key.id"
                        :toggling="togglingKeyId === key.id"
                        @recover="handleRecoverKey(key)"
                        @permissions="handleKeyPermissions(key)"
                        @update:proxy-popover-open="(v: boolean) => handleProxyPopoverToggle(key.id, v)"
                        @clear-proxy="clearKeyProxy(key)"
                        @set-proxy="(v: string) => setKeyProxy(key, v)"
                        @edit="handleEditKey(endpoint, key)"
                        @open-antigravity-quota="openAntigravityQuotaDialog(key)"
                        @toggle-active="toggleKeyActive(key)"
                        @delete="handleDeleteKey(key)"
                      />
                    </div>
                    <!-- Codex 上游额度信息（仅当有元数据时显示） -->
                    <div
                      v-if="hasCodexQuotaDisplayData(key)"
                      class="mt-2 p-2 bg-muted/30 rounded-md"
                    >
                      <ProviderQuotaSectionHeader
                        :title="legacyT('账号配额')"
                        :loading="refreshingQuota"
                        :updated-text="getCodexQuotaDisplay(key)?.updated_at ? formatCodexUpdatedAt(getCodexQuotaDisplay(key)?.updated_at || 0) : null"
                      />
                      <!-- 普通 Codex 限额并排显示：Team/Plus/Enterprise 账号 2列, Free 账号 1列 -->
                      <div
                        class="grid gap-3"
                        :class="isCodexTeamPlan(key) ? 'grid-cols-2' : 'grid-cols-1'"
                      >
                        <!-- 周限额 -->
                        <ProviderQuotaProgressRow
                          v-if="getCodexQuotaDisplay(key)?.primary_used_percent !== undefined"
                          :label="legacyT('周限额')"
                          :used-percent="getCodexQuotaDisplay(key)?.primary_used_percent || 0"
                          :remaining-percent="toCodexRemainingPercent(getCodexQuotaDisplay(key)?.primary_used_percent)"
                          :meter-class="getQuotaRemainingClass(getCodexQuotaDisplay(key)?.primary_used_percent || 0)"
                          :bar-class="getQuotaRemainingBarColor(getCodexQuotaDisplay(key)?.primary_used_percent || 0)"
                          :reset-text="(getCodexQuotaDisplay(key)?.primary_reset_at || getCodexQuotaDisplay(key)?.primary_reset_seconds) && shouldStartCodexResetCountdown(getCodexQuotaDisplay(key)?.primary_used_percent || 0)
                            ? getResetCountdownText(
                              getCodexQuotaDisplay(key)?.primary_reset_at,
                              getCodexQuotaDisplay(key)?.primary_reset_seconds,
                              getCodexQuotaDisplay(key)?.updated_at,
                              getCodexQuotaDisplay(key)?.primary_used_percent
                            )
                            : null"
                          :footer-class="getResetCountdownClass(
                            getCodexQuotaDisplay(key)?.primary_reset_at,
                            getCodexQuotaDisplay(key)?.primary_reset_seconds,
                            getCodexQuotaDisplay(key)?.updated_at,
                            getCodexQuotaDisplay(key)?.primary_used_percent
                          )"
                        />
                        <!-- 5H限额（仅 Team/Plus/Enterprise 显示） -->
                        <ProviderQuotaProgressRow
                          v-if="isCodexTeamPlan(key) && getCodexQuotaDisplay(key)?.secondary_used_percent !== undefined"
                          :label="legacyT('5H限额')"
                          :used-percent="getCodexQuotaDisplay(key)?.secondary_used_percent || 0"
                          :remaining-percent="toCodexRemainingPercent(getCodexQuotaDisplay(key)?.secondary_used_percent)"
                          :meter-class="getQuotaRemainingClass(getCodexQuotaDisplay(key)?.secondary_used_percent || 0)"
                          :bar-class="getQuotaRemainingBarColor(getCodexQuotaDisplay(key)?.secondary_used_percent || 0)"
                          :reset-text="shouldStartCodexResetCountdown(getCodexQuotaDisplay(key)?.secondary_used_percent || 0)
                            ? getCodexQuotaResetText(
                              getCodexQuotaDisplay(key)?.secondary_reset_at,
                              getCodexQuotaDisplay(key)?.secondary_reset_seconds,
                              getCodexQuotaDisplay(key)?.updated_at,
                              getCodexQuotaDisplay(key)?.secondary_used_percent
                            )
                            : null"
                          :footer-class="getResetCountdownClass(
                            getCodexQuotaDisplay(key)?.secondary_reset_at,
                            getCodexQuotaDisplay(key)?.secondary_reset_seconds,
                            getCodexQuotaDisplay(key)?.updated_at,
                            getCodexQuotaDisplay(key)?.secondary_used_percent
                          )"
                        />
                      </div>
                      <!-- Spark 限额独立一行展示，避免与普通 Codex 周/5H 混淆 -->
                      <div
                        v-if="hasCodexSparkQuotaDisplayData(key)"
                        class="mt-3 border-t border-border/60 pt-2"
                      >
                        <div class="mb-1 text-[10px] text-muted-foreground">
                          GPT-5.3 Codex Spark
                        </div>
                        <div class="grid gap-3 grid-cols-2">
                          <ProviderQuotaProgressRow
                            v-if="getCodexQuotaDisplay(key)?.spark_secondary_used_percent !== undefined"
                            :label="legacyT('Spark 周')"
                            :used-percent="getCodexQuotaDisplay(key)?.spark_secondary_used_percent || 0"
                            :remaining-percent="toCodexRemainingPercent(getCodexQuotaDisplay(key)?.spark_secondary_used_percent)"
                            :meter-class="getQuotaRemainingClass(getCodexQuotaDisplay(key)?.spark_secondary_used_percent || 0)"
                            :bar-class="getQuotaRemainingBarColor(getCodexQuotaDisplay(key)?.spark_secondary_used_percent || 0)"
                            :reset-text="shouldStartCodexResetCountdown(getCodexQuotaDisplay(key)?.spark_secondary_used_percent || 0)
                              ? getCodexQuotaResetText(
                                getCodexQuotaDisplay(key)?.spark_secondary_reset_at,
                                getCodexQuotaDisplay(key)?.spark_secondary_reset_seconds,
                                getCodexQuotaDisplay(key)?.updated_at,
                                getCodexQuotaDisplay(key)?.spark_secondary_used_percent
                              )
                              : null"
                            :footer-class="getResetCountdownClass(
                              getCodexQuotaDisplay(key)?.spark_secondary_reset_at,
                              getCodexQuotaDisplay(key)?.spark_secondary_reset_seconds,
                              getCodexQuotaDisplay(key)?.updated_at,
                              getCodexQuotaDisplay(key)?.spark_secondary_used_percent
                            )"
                          />
                          <ProviderQuotaProgressRow
                            v-if="getCodexQuotaDisplay(key)?.spark_primary_used_percent !== undefined"
                            :label="legacyT('Spark 5H')"
                            :used-percent="getCodexQuotaDisplay(key)?.spark_primary_used_percent || 0"
                            :remaining-percent="toCodexRemainingPercent(getCodexQuotaDisplay(key)?.spark_primary_used_percent)"
                            :meter-class="getQuotaRemainingClass(getCodexQuotaDisplay(key)?.spark_primary_used_percent || 0)"
                            :bar-class="getQuotaRemainingBarColor(getCodexQuotaDisplay(key)?.spark_primary_used_percent || 0)"
                            :reset-text="shouldStartCodexResetCountdown(getCodexQuotaDisplay(key)?.spark_primary_used_percent || 0)
                              ? getCodexQuotaResetText(
                                getCodexQuotaDisplay(key)?.spark_primary_reset_at,
                                getCodexQuotaDisplay(key)?.spark_primary_reset_seconds,
                                getCodexQuotaDisplay(key)?.updated_at,
                                getCodexQuotaDisplay(key)?.spark_primary_used_percent
                              )
                              : null"
                            :footer-class="getResetCountdownClass(
                              getCodexQuotaDisplay(key)?.spark_primary_reset_at,
                              getCodexQuotaDisplay(key)?.spark_primary_reset_seconds,
                              getCodexQuotaDisplay(key)?.updated_at,
                              getCodexQuotaDisplay(key)?.spark_primary_used_percent
                            )"
                          />
                        </div>
                      </div>
                      <div
                        v-if="getCodexResetCreditAvailableCount(key) !== null"
                        class="mt-3 border-t border-border/60 pt-2"
                      >
                        <div class="flex flex-wrap items-center gap-x-1 gap-y-1 text-[10px] leading-4 text-muted-foreground">
                          <button
                            v-if="canConsumeCodexResetCredit(key)"
                            type="button"
                            class="font-medium text-primary underline-offset-2 transition-colors hover:text-primary/80 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 disabled:pointer-events-none disabled:opacity-60"
                            :disabled="consumingCodexResetCreditKeyId === key.id"
                            @click="handleConsumeCodexResetCredit(key)"
                          >
                            {{ consumingCodexResetCreditKeyId === key.id ? legacyT('重置中...') : legacyT('点击以进行重置') }}
                          </button>
                          <span
                            v-else
                            class="font-medium"
                          >
                            {{ legacyT('点击以进行重置') }}
                          </span>
                          <span>{{ formatCodexResetCreditCount(key) }}</span>
                          <template v-if="getVisibleCodexResetCreditItems(key).length > 0">
                            <span aria-hidden="true">|</span>
                            <span>{{ legacyT('临近过期') }}</span>
                            <template
                              v-for="(item, itemIndex) in getVisibleCodexResetCreditItems(key)"
                              :key="item.id || `${item.displayKey}-${item.expiresAt}`"
                            >
                              <span
                                :title="item.title"
                                class="tabular-nums"
                              >
                                {{ item.displayKey }} {{ formatCodexResetCreditExpiresAt(item.expiresAt) }}
                              </span>
                              <span
                                v-if="itemIndex < getVisibleCodexResetCreditItems(key).length - 1"
                                aria-hidden="true"
                              >·</span>
                            </template>
                          </template>
                        </div>
                      </div>
                    </div>
                    <!-- Antigravity 上游额度摘要（按家族分组展示关键配额） -->
                    <div
                      v-if="provider.provider_type === 'antigravity' && (hasAntigravityQuotaDisplayData(key) || isAntigravityForbiddenKey(key))"
                      class="mt-2 p-2 rounded-md"
                      :class="isAntigravityForbiddenKey(key) ? 'bg-destructive/10 border border-destructive/30' : 'bg-muted/30'"
                    >
                      <!-- 封禁状态显示 -->
                      <div
                        v-if="isAntigravityForbiddenKey(key)"
                        class="flex items-center gap-2 text-destructive"
                      >
                        <ShieldX class="w-4 h-4 shrink-0" />
                        <div class="flex-1 min-w-0">
                          <div class="text-[11px] font-medium">
                            {{ legacyT('账户访问被禁止') }}
                          </div>
                          <div
                            v-if="getAntigravityForbiddenReason(key)"
                            class="text-[10px] text-destructive/80 truncate"
                            :title="getAntigravityForbiddenReason(key)"
                          >
                            {{ getAntigravityForbiddenReason(key) }}
                          </div>
                        </div>
                        <span
                          v-if="getAntigravityForbiddenAt(key)"
                          class="text-[9px] text-destructive/60 shrink-0"
                        >
                          {{ formatBanTimestamp(getAntigravityForbiddenAt(key)) }}
                        </span>
                      </div>
                      <!-- 正常配额显示 -->
                      <template v-else>
                        <ProviderQuotaSectionHeader
                          :title="legacyT('模型配额')"
                          :loading="refreshingQuota"
                          :updated-text="getAntigravityQuotaUpdatedAt(key) ? formatAntigravityUpdatedAt(getAntigravityQuotaUpdatedAt(key) || 0) : null"
                        />
                        <div class="grid grid-cols-2 gap-3">
                          <ProviderQuotaProgressRow
                            v-for="item in getAntigravityQuotaPreviewForKey(key)"
                            :key="item.model"
                            :label="item.label"
                            :title="item.model"
                            :used-percent="item.usedPercent"
                            :remaining-percent="item.remainingPercent"
                            :meter-class="getQuotaRemainingClass(item.usedPercent)"
                            :bar-class="getQuotaRemainingBarColor(item.usedPercent)"
                          >
                            <template #footer>
                              <div class="mt-0.5 space-y-0.5">
                                <div
                                  v-if="item.resetSeconds !== null"
                                  class="text-[9px] text-muted-foreground/70"
                                >
                                  <template v-if="item.resetSeconds !== null && item.resetSeconds > 0">
                                    {{ formatResetTime(item.resetSeconds) }}{{ legacyT('后重置') }}
                                  </template>
                                  <template v-else-if="item.resetSeconds !== null && item.resetSeconds <= 0">
                                    {{ legacyT('已重置') }}
                                  </template>
                                </div>
                              </div>
                            </template>
                          </ProviderQuotaProgressRow>
                          <button
                            v-if="getAntigravityQuotaHiddenCountForKey(key) > 0"
                            type="button"
                            class="col-span-2 text-left text-[10px] text-muted-foreground hover:text-foreground transition-colors"
                            @click="openAntigravityQuotaDialog(key)"
                          >
                            {{ legacyT('另有') }} {{ getAntigravityQuotaHiddenCountForKey(key) }} {{ legacyT('个模型，查看全部') }}
                          </button>
                        </div>
                      </template>
                    </div>
                    <!-- Gemini CLI 上游模型配额 -->
                    <div
                      v-if="provider.provider_type === 'gemini_cli' && hasGeminiCliQuotaDisplayData(key)"
                      class="mt-2 p-2 rounded-md bg-muted/30"
                    >
                      <ProviderQuotaSectionHeader
                        :title="legacyT('模型配额')"
                        :loading="refreshingQuota"
                        :updated-text="getGeminiCliQuotaUpdatedAt(key) ? formatUpdatedAt(getGeminiCliQuotaUpdatedAt(key) || 0) : null"
                      />
                      <div
                        v-if="getGeminiCliAccountCreditsText(key, 'gemini_cli')"
                        class="mb-2 text-[10px] font-medium text-foreground/90"
                      >
                        {{ getGeminiCliAccountCreditsText(key, 'gemini_cli') }}
                      </div>
                      <div
                        v-if="getGeminiCliQuotaItems(key).length > 0"
                        class="grid grid-cols-2 gap-3"
                      >
                        <ProviderQuotaProgressRow
                          v-for="item in getGeminiCliQuotaItems(key)"
                          :key="item.model"
                          :label="item.label"
                          :title="item.model"
                          :used-percent="item.usedPercent"
                          :remaining-percent="item.remainingPercent"
                          :meter-class="getQuotaRemainingClass(item.usedPercent)"
                          :bar-class="getQuotaRemainingBarColor(item.usedPercent)"
                        >
                          <template #footer>
                            <div
                              v-if="item.resetSeconds !== null && item.remainingPercent < 100"
                              class="text-[9px] text-muted-foreground/70 mt-0.5"
                            >
                              <template v-if="item.resetSeconds > 0">
                                {{ formatResetTime(item.resetSeconds) }}{{ legacyT('后重置') }}
                              </template>
                              <template v-else>
                                {{ legacyT('已重置') }}
                              </template>
                            </div>
                          </template>
                        </ProviderQuotaProgressRow>
                      </div>
                    </div>
                    <!-- Kiro 上游额度信息（仅当有元数据时显示） -->
                    <div
                      v-if="provider.provider_type === 'kiro' && (hasKiroQuotaDisplayData(key) || isKiroBannedKey(key))"
                      class="mt-2 p-2 rounded-md"
                      :class="isKiroBannedKey(key) ? 'bg-destructive/10 border border-destructive/30' : 'bg-muted/30'"
                    >
                      <!-- 封禁状态显示 -->
                      <div
                        v-if="isKiroBannedKey(key)"
                        class="flex items-center gap-2 text-destructive"
                      >
                        <ShieldX class="w-4 h-4 shrink-0" />
                        <div class="flex-1 min-w-0">
                          <div class="text-[11px] font-medium">
                            {{ legacyT('账户已封禁') }}
                          </div>
                          <div
                            v-if="getKiroQuotaDisplay(key)?.ban_reason"
                            class="text-[10px] text-destructive/80 truncate"
                            :title="getKiroQuotaDisplay(key)?.ban_reason"
                          >
                            {{ getKiroQuotaDisplay(key)?.ban_reason }}
                          </div>
                        </div>
                        <span
                          v-if="getKiroQuotaDisplay(key)?.banned_at"
                          class="text-[9px] text-destructive/60 shrink-0"
                        >
                          {{ formatBanTimestamp(getKiroQuotaDisplay(key)?.banned_at) }}
                        </span>
                      </div>
                      <!-- 正常配额显示 -->
                      <template v-else>
                        <ProviderQuotaSectionHeader
                          :title="legacyT('账号配额')"
                          :loading="refreshingQuota"
                          :updated-text="getKiroQuotaDisplay(key)?.updated_at ? formatKiroUpdatedAt(getKiroQuotaDisplay(key)?.updated_at || 0) : null"
                        />
                        <!-- Kiro 额度显示：使用进度 -->
                        <div>
                          <!-- 使用额度进度条 -->
                          <ProviderQuotaProgressRow
                            :label="legacyT('使用额度')"
                            :used-percent="getKiroQuotaDisplay(key)?.usage_percentage || 0"
                            :meter-class="getQuotaRemainingClass(getKiroQuotaDisplay(key)?.usage_percentage || 0)"
                            :bar-class="getQuotaRemainingBarColor(getKiroQuotaDisplay(key)?.usage_percentage || 0)"
                          >
                            <template #footer>
                              <div class="flex items-center justify-between text-[9px] text-muted-foreground/70 mt-0.5">
                                <span>
                                  {{ formatKiroUsage(getKiroQuotaDisplay(key)?.current_usage) }} /
                                  {{ formatKiroUsage(getKiroQuotaDisplay(key)?.usage_limit) }}
                                </span>
                                <span v-if="getKiroQuotaDisplay(key)?.next_reset_at">
                                  {{ formatKiroResetTime(getKiroQuotaDisplay(key)?.next_reset_at) }}{{ legacyT('重置') }}
                                </span>
                              </div>
                            </template>
                          </ProviderQuotaProgressRow>
                        </div>
                      </template>
                    </div>
                    <!-- Windsurf 上游额度信息 -->
                    <div
                      v-if="provider.provider_type === 'windsurf' && (hasWindsurfQuotaDisplayData(key) || isWindsurfUnavailableKey(key) || isWindsurfExhaustedKey(key))"
                      class="mt-2 p-2 rounded-md"
                      :class="isWindsurfUnavailableKey(key) ? 'bg-destructive/10 border border-destructive/30' : (isWindsurfExhaustedKey(key) ? 'bg-amber-50 dark:bg-amber-950/20 border border-amber-200 dark:border-amber-900/50' : 'bg-muted/30')"
                    >
                      <div
                        v-if="isWindsurfUnavailableKey(key)"
                        class="flex items-center gap-2 text-destructive"
                      >
                        <ShieldX class="w-4 h-4 shrink-0" />
                        <div class="flex-1 min-w-0">
                          <div class="text-[11px] font-medium">
                            {{ legacyT('账号不可用') }}
                          </div>
                          <div
                            v-if="getWindsurfQuotaDisplay(key)?.last_error"
                            class="text-[10px] text-destructive/80 truncate"
                            :title="getWindsurfQuotaDisplay(key)?.last_error || ''"
                          >
                            {{ getWindsurfQuotaDisplay(key)?.last_error }}
                          </div>
                        </div>
                      </div>
                      <template v-else>
                        <div
                          v-if="isWindsurfExhaustedKey(key)"
                          class="mb-2 flex items-center gap-2 text-amber-700 dark:text-amber-300"
                        >
                          <ShieldX class="w-4 h-4 shrink-0" />
                          <div class="flex-1 min-w-0">
                            <div class="text-[11px] font-medium">
                              {{ getWindsurfQuotaStatusLabel(key) }}
                            </div>
                            <div
                              v-if="getWindsurfQuotaDisplay(key)?.last_error"
                              class="text-[10px] text-amber-700/80 dark:text-amber-300/80 truncate"
                              :title="getWindsurfQuotaDisplay(key)?.last_error || ''"
                            >
                              {{ getWindsurfQuotaDisplay(key)?.last_error }}
                            </div>
                          </div>
                        </div>
                        <ProviderQuotaSectionHeader
                          :title="legacyT('账号配额')"
                          :loading="refreshingQuota"
                          :updated-text="getWindsurfQuotaDisplay(key)?.updated_at ? formatKiroUpdatedAt(getWindsurfQuotaDisplay(key)?.updated_at || 0) : null"
                        />
                        <div class="grid grid-cols-2 gap-3">
                          <ProviderQuotaProgressRow
                            v-if="getWindsurfQuotaDisplay(key)?.daily_remaining_percent !== undefined"
                            :label="legacyT('日额度')"
                            :used-percent="getWindsurfQuotaDisplay(key)?.daily_used_percent || 0"
                            :remaining-percent="getWindsurfQuotaDisplay(key)?.daily_remaining_percent || 0"
                            :meter-class="getQuotaRemainingClass(getWindsurfQuotaDisplay(key)?.daily_used_percent || 0)"
                            :bar-class="getQuotaRemainingBarColor(getWindsurfQuotaDisplay(key)?.daily_used_percent || 0)"
                            :reset-text="getWindsurfQuotaDisplay(key)?.daily_reset_at
                              ? `${formatKiroResetTime(getWindsurfQuotaDisplay(key)?.daily_reset_at || 0)}${legacyT('重置')}`
                              : null"
                          />
                          <ProviderQuotaProgressRow
                            v-if="getWindsurfQuotaDisplay(key)?.weekly_remaining_percent !== undefined"
                            :label="legacyT('周额度')"
                            :used-percent="getWindsurfQuotaDisplay(key)?.weekly_used_percent || 0"
                            :remaining-percent="getWindsurfQuotaDisplay(key)?.weekly_remaining_percent || 0"
                            :meter-class="getQuotaRemainingClass(getWindsurfQuotaDisplay(key)?.weekly_used_percent || 0)"
                            :bar-class="getQuotaRemainingBarColor(getWindsurfQuotaDisplay(key)?.weekly_used_percent || 0)"
                            :reset-text="getWindsurfQuotaDisplay(key)?.weekly_reset_at
                              ? `${formatKiroResetTime(getWindsurfQuotaDisplay(key)?.weekly_reset_at || 0)}${legacyT('重置')}`
                              : null"
                          />
                        </div>
                        <div
                          v-if="hasWindsurfPromptQuota(key) || hasWindsurfFlexQuota(key)"
                          class="mt-2 flex items-center gap-3 text-[9px] text-muted-foreground/70"
                        >
                          <span v-if="hasWindsurfPromptQuota(key)">
                            Prompt {{ formatKiroUsage(getWindsurfQuotaDisplay(key)?.prompt_used || 0) }} /
                            {{ formatKiroUsage(getWindsurfQuotaDisplay(key)?.prompt_limit || 0) }}
                          </span>
                          <span v-if="hasWindsurfFlexQuota(key)">
                            Flex {{ formatKiroUsage(getWindsurfQuotaDisplay(key)?.flex_used || 0) }} /
                            {{ formatKiroUsage(getWindsurfQuotaDisplay(key)?.flex_limit || 0) }}
                          </span>
                        </div>
                        <div
                          v-if="hasWindsurfModelCount(key) || hasWindsurfModelPreview(key)"
                          class="mt-2 flex items-center justify-between gap-2 text-[9px] text-muted-foreground/70"
                        >
                          <span>
                            {{ legacyT('模型') }} {{ getWindsurfQuotaDisplay(key)?.allowed_models_count ?? getWindsurfQuotaDisplay(key)?.models?.length }} {{ legacyT('个') }}
                          </span>
                          <span
                            v-if="getWindsurfModelPreview(key)"
                            class="truncate"
                            :title="getWindsurfModelPreview(key) || ''"
                          >
                            {{ getWindsurfModelPreview(key) }}
                          </span>
                        </div>
                      </template>
                    </div>
                    <!-- ChatGPT Web 上游额度信息（生图配额） -->
                    <div
                      v-if="provider.provider_type === 'chatgpt_web' && hasChatGPTWebQuotaDisplayData(key)"
                      class="mt-2 p-2 rounded-md bg-muted/30"
                    >
                      <ProviderQuotaSectionHeader
                        :title="legacyT('账号配额')"
                        :loading="refreshingQuota"
                        :updated-text="getChatGPTWebQuotaDisplay(key)?.updated_at ? formatKiroUpdatedAt(getChatGPTWebQuotaDisplay(key)?.updated_at || 0) : null"
                      />
                      <div>
                        <ProviderQuotaProgressRow
                          :label="legacyT('剩余额度')"
                          :used-percent="getChatGPTWebQuotaUsedPercent(key)"
                          :remaining-percent="getChatGPTWebQuotaRemainingPercent(key)"
                          :meter-class="getQuotaRemainingClass(getChatGPTWebQuotaUsedPercent(key))"
                          :bar-class="getQuotaRemainingBarColor(getChatGPTWebQuotaUsedPercent(key))"
                        >
                          <template #footer>
                            <div class="flex items-center justify-between text-[9px] text-muted-foreground/70 mt-0.5">
                              <span>
                                {{ formatChatGPTWebUsage(getChatGPTWebQuotaDisplay(key)?.image_quota_remaining) }} /
                                {{ formatChatGPTWebUsage(getChatGPTWebQuotaDisplay(key)?.image_quota_total) }}
                              </span>
                              <span v-if="getChatGPTWebQuotaDisplay(key)?.image_quota_reset_at">
                                {{ formatKiroResetTime(getChatGPTWebQuotaDisplay(key)?.image_quota_reset_at) }}{{ legacyT('重置') }}
                              </span>
                            </div>
                          </template>
                        </ProviderQuotaProgressRow>
                      </div>
                    </div>
                    <!-- 第二行：优先级 + API 格式（展开显示） + 统计信息 -->
                    <div class="flex items-center gap-1.5 mt-1 text-[11px] text-muted-foreground">
                      <!-- 优先级放最前面，支持点击编辑 -->
                      <span
                        v-if="editingPriorityKey !== key.id"
                        :title="legacyT('点击编辑优先级')"
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
                          {{ legacyT(key.last_models_fetch_error ? '同步失败' : '自动同步') }}
                        </span>
                      </template>
                      <!-- RPM 限制信息（第二位） -->
                      <template v-if="key.rpm_limit || key.is_adaptive">
                        <span class="text-muted-foreground/40">|</span>
                        <span v-if="key.is_adaptive">
                          {{ key.learned_rpm_limit != null ? `${key.learned_rpm_limit}` : legacyT('探测中') }} RPM
                          <span class="text-muted-foreground/60">({{ legacyT('自适应') }})</span>
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
                          {{ formatApiFormatShort(format) }}
                        </span>
                        <span
                          v-if="editingMultiplierKey !== key.id || editingMultiplierFormat !== format"
                          :title="legacyT('点击编辑倍率')"
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
                  <!-- 分页控制 -->
                  <div
                    v-if="shouldPaginateKeys"
                    class="px-4 py-2 flex items-center justify-between text-xs text-muted-foreground mt-auto"
                  >
                    <span>{{ legacyT('共') }} {{ allKeys.length }} {{ legacyT('个') }}{{ legacyT(isKeyManagedProviderType(provider.provider_type) ? '密钥' : '账号') }}</span>
                    <div class="flex items-center gap-1.5">
                      <Button
                        variant="ghost"
                        size="sm"
                        class="h-6 px-2 text-xs"
                        :disabled="loadingProviderKeys || currentKeyPage <= 1"
                        @click="goToKeyPage(currentKeyPage - 1)"
                      >
                        ‹
                      </Button>
                      <span class="tabular-nums">{{ currentKeyPage }} / {{ totalKeyPages }}</span>
                      <Button
                        variant="ghost"
                        size="sm"
                        class="h-6 px-2 text-xs"
                        :disabled="loadingProviderKeys || currentKeyPage >= totalKeyPages"
                        @click="goToKeyPage(currentKeyPage + 1)"
                      >
                        ›
                      </Button>
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
                    {{ legacyT(isKeyManagedProviderType(provider.provider_type) ? '暂无密钥配置' : '暂无账号配置') }}
                  </p>
                  <p class="text-xs mt-1">
                    {{ endpoints.length > 0
                      ? legacyT(isKeyManagedProviderType(provider.provider_type) ? '点击上方"添加密钥"按钮创建第一个密钥' : '点击上方"添加账号"按钮添加第一个账号')
                      : legacyT('请先添加端点，然后再添加密钥') }}
                  </p>
                </div>
              </Card>

              <!-- 模型查看 -->
              <ModelsTab
                v-if="provider"
                :key="`models-${provider.id}`"
                :provider="provider"
                :models="providerModels"
                :endpoints="endpoints"
                :provider-keys="providerKeys"
                :loading="loadingProviderModels"
                @edit-model="handleEditModel"
                @batch-assign="handleBatchAssign"
                @refresh="loadEndpoints"
              />

              <!-- 模型映射 -->
              <ModelMappingTab
                v-if="provider"
                ref="modelMappingTabRef"
                :key="`mapping-${provider.id}`"
                :provider="provider"
                :endpoints="endpoints"
                :provider-keys="providerKeys"
                :models="providerModels"
                :mapping-preview="providerMappingPreview"
                :loading="loadingProviderMappingPreview"
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
    :available-api-formats="availableKeyApiFormats"
    @close="keyFormDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <ProviderKeyBatchImportDialog
    v-if="open && provider?.provider_type === 'custom'"
    :open="keyBatchImportDialogOpen"
    :provider-id="provider.id"
    :provider-name="provider.name"
    :available-api-formats="availableKeyApiFormats"
    @close="keyBatchImportDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- OAuth 账号对话框 -->
  <OAuthAccountDialog
    v-if="open && provider"
    :open="oauthAccountDialogOpen"
    :provider-id="provider.id"
    :provider-type="provider.provider_type"
    @close="oauthAccountDialogOpen = false"
    @saved="handleKeyChanged"
  />

  <!-- OAuth 密钥编辑对话框 -->
  <OAuthKeyEditDialog
    v-if="open"
    :open="oauthKeyEditDialogOpen"
    :editing-key="editingKey"
    @close="oauthKeyEditDialogOpen = false"
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
    :title="legacyT('删除密钥')"
    :description="formatDeleteKeyConfirmDescription()"
    :confirm-text="legacyT('删除')"
    :cancel-text="legacyT('取消')"
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
    @update:open="handleBatchAssignDialogOpenUpdate"
    @changed="handleBatchAssignChanged"
  />

  <!-- Antigravity 配额详情弹窗 -->
  <AntigravityQuotaDialog
    v-if="antigravityQuotaDialogKey"
    :open="antigravityQuotaDialogOpen"
    :metadata="antigravityQuotaDialogKey.upstream_metadata"
    :quota-snapshot="antigravityQuotaDialogKey.status_snapshot?.quota ?? null"
    :key-name="antigravityQuotaDialogKey.name || legacyT('未命名密钥')"
    :provider-id="providerId"
    :key-id="antigravityQuotaDialogKey.id"
    @update:open="antigravityQuotaDialogOpen = $event"
  />

  <!-- 故障转移规则弹窗 -->
  <FailoverRulesDialog
    :open="failoverRulesDialogOpen"
    :provider="provider ?? null"
    @update:open="failoverRulesDialogOpen = $event"
    @saved="loadProvider()"
  />
</template>

<script setup lang="ts">
import { ref, watch, computed, nextTick } from 'vue'
import {
  Plus,
  Key,
  ListPlus,
  Loader2,
  GripVertical,
  ShieldX,
} from 'lucide-vue-next'
import { parseApiError } from '@/utils/errorParser'
import { useEscapeKey } from '@/composables/useEscapeKey'
import { useI18n } from '@/i18n'
import Button from '@/components/ui/button.vue'
import Card from '@/components/ui/card.vue'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useClipboard } from '@/composables/useClipboard'
import { useCountdownTimer, formatCountdown, getCodexResetCountdown } from '@/composables/useCountdownTimer'
import {
  getProvider,
  getProviderEndpoints,
  updateProvider,
  getProviderModels,
  getProviderMappingPreview,
  type ProviderMappingPreviewResponse,
  type ProviderWithEndpointsSummary,
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import {
  KeyFormDialog,
  KeyAllowedModelsEditDialog,
  ModelsTab,
  BatchAssignModelsDialog,
  OAuthAccountDialog,
  OAuthKeyEditDialog
} from '@/features/providers/components'
import ModelMappingTab from '@/features/providers/components/provider-tabs/ModelMappingTab.vue'
import EndpointFormDialog from '@/features/providers/components/EndpointFormDialog.vue'
import ProviderModelFormDialog from '@/features/providers/components/ProviderModelFormDialog.vue'
import AlertDialog from '@/components/common/AlertDialog.vue'
import AntigravityQuotaDialog from '@/features/providers/components/AntigravityQuotaDialog.vue'
import FailoverRulesDialog from '@/features/providers/components/FailoverRulesDialog.vue'
import ProviderDetailHeader from '@/features/providers/components/ProviderDetailHeader.vue'
import ProviderKeyBatchImportDialog from '@/features/providers/components/ProviderKeyBatchImportDialog.vue'
import ProviderKeyActionCluster from '@/features/providers/components/ProviderKeyActionCluster.vue'
import ProviderKeyIdentityBlock from '@/features/providers/components/ProviderKeyIdentityBlock.vue'
import ProviderMonthlyQuotaCard from '@/features/providers/components/ProviderMonthlyQuotaCard.vue'
import ProviderQuotaProgressRow from '@/features/providers/components/ProviderQuotaProgressRow.vue'
import ProviderQuotaSectionHeader from '@/features/providers/components/ProviderQuotaSectionHeader.vue'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import {
  compareAntigravityQuotaItems,
  dedupeAntigravityQuotaItemsByLabel,
  resolveAntigravityQuotaLabel,
} from '@/features/providers/utils/antigravityQuota'
import {
  deleteEndpointKey,
  recoverKeyHealth,
  getProviderKeysPage,
  updateProviderKey,
  revealEndpointKey,
  exportKey,
  refreshProviderOAuth,
  refreshProviderQuota,
  consumeCodexResetCredit,
  clearOAuthInvalid,
  type ProviderEndpoint,
  type EndpointAPIKey,
  type Model,
  API_FORMAT_ORDER,
  sortApiFormats,
} from '@/api/endpoints'
import type {
  UpstreamMetadata,
  AntigravityModelQuota,
  CodexUpstreamMetadata,
  ChatGPTWebUpstreamMetadata,
  GrokUpstreamMetadata,
  KiroUpstreamMetadata,
  WindsurfUpstreamMetadata,
  QuotaResetCreditsSnapshot,
  QuotaStatusSnapshot,
  QuotaWindowSnapshot,
} from '@/api/endpoints/types'
import { formatApiFormatShort } from '@/api/endpoints/types/api-format'
import { isOAuthAccountProviderType, isKeyManagedProviderType } from '../utils/providerTypeUtils'
import { getOAuthOrgBadge } from '@/utils/oauthIdentity'
import { getOAuthRefreshFeedback } from '@/utils/oauthRefreshFeedback'
import { formatCompactNumber } from '@/utils/format'
import {
  canEditOAuthCredential,
  canExportOAuthCredential,
  canRefreshOAuthCredential,
  isOAuthManagedCredential,
  getProviderMaskedSecretLabel,
  shouldShowOAuthRefreshControl,
} from '@/utils/providerKeyAuth'
import {
  getAccountStatusDisplay,
  getAccountStatusTitle,
  getOAuthRefreshButtonTitle as resolveOAuthRefreshButtonTitle,
  getOAuthStatusDisplay,
  getOAuthStatusDisplayWithFallback,
  getOAuthStatusTitle as resolveOAuthStatusTitle,
} from '@/utils/providerKeyStatus'
import { getGeminiCliAccountCreditsText } from '@/utils/providerKeyQuota'
import {
  createCodexResetCreditIdempotencyKey,
  formatCodexResetCreditCount as formatCodexResetCreditCountLabel,
  formatCodexResetCreditExpiresAt,
  getCodexResetCreditAvailableCount as getCodexResetCreditAvailableCountFromSnapshot,
  getVisibleCodexResetCreditItems as getVisibleCodexResetCreditItemsFromSnapshot,
  mergeCodexQuotaDisplays,
} from './codex-reset-credit-display'

// 扩展端点类型,包含密钥列表
interface ProviderEndpointWithKeys extends ProviderEndpoint {
  keys?: EndpointAPIKey[]
  rpm_limit?: number
}

interface Props {
  providerId: string | null
  open: boolean
  initialProvider?: ProviderWithEndpointsSummary | null
}

const props = defineProps<Props>()
const emit = defineEmits<{
  (e: 'update:open', value: boolean): void
  (e: 'edit', provider: ProviderWithEndpointsSummary): void
  (e: 'toggleStatus', provider: ProviderWithEndpointsSummary): void
  (e: 'refresh'): void
}>()

const { error: showError, success: showSuccess, warning: showWarning } = useToast()
const { confirm } = useConfirm()
const { copyToClipboard } = useClipboard()
const { tick: countdownTick, start: startCountdownTimer, stop: stopCountdownTimer } = useCountdownTimer()
const { legacyT, locale } = useI18n()

function localizedApiError(err: unknown, fallback: string): string {
  return legacyT(parseApiError(err, fallback))
}

const loading = ref(false)
const provider = ref<ProviderWithEndpointsSummary | null>(null)
const endpoints = ref<ProviderEndpointWithKeys[]>([])
const providerKeys = ref<EndpointAPIKey[]>([])  // Provider 级别的 keys
const providerModels = ref<Model[]>([])  // Provider 级别的 models
const providerMappingPreview = ref<ProviderMappingPreviewResponse | null>(null)  // 映射预览
const loadingProviderEndpoints = ref(false)
const loadingProviderKeys = ref(false)
const loadingProviderModels = ref(false)
const loadingProviderMappingPreview = ref(false)
let providerLoadRequestId = 0
let endpointsLoadRequestId = 0
let keysLoadRequestId = 0
let mappingPreviewLoadRequestId = 0
const DEFAULT_PROVIDER_KEYS_PAGE_SIZE = 3
const CUSTOM_PROVIDER_KEYS_PAGE_SIZE = 4

function getProviderKeysPageSize(providerType?: string | null): number {
  return (providerType || '').trim().toLowerCase() === 'custom'
    ? CUSTOM_PROVIDER_KEYS_PAGE_SIZE
    : DEFAULT_PROVIDER_KEYS_PAGE_SIZE
}

// 系统级格式转换配置
const systemFormatConversionEnabled = ref(false)

// 端点相关状态
const endpointDialogOpen = ref(false)

// 密钥相关状态
const keyFormDialogOpen = ref(false)
const keyBatchImportDialogOpen = ref(false)
const keyPermissionsDialogOpen = ref(false)
const oauthAccountDialogOpen = ref(false)
const oauthKeyEditDialogOpen = ref(false)
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

// OAuth 失效清除状态
const clearingOAuthInvalidKeyId = ref<string | null>(null)

// Codex reset credit 消费状态
const consumingCodexResetCreditKeyId = ref<string | null>(null)

// 限额刷新状态（Codex / Antigravity）
const refreshingQuota = ref(false)

// Antigravity 配额详情弹窗状态
const antigravityQuotaDialogOpen = ref(false)
const antigravityQuotaDialogKey = ref<EndpointAPIKey | null>(null)

// 故障转移规则
const failoverRulesDialogOpen = ref(false)
const FAILOVER_RULE_ARRAY_KEYS = [
  'success_failover_patterns',
  'error_stop_patterns',
  'stop_status_codes',
  'stop_on_status_codes',
  'early_stop_status_codes',
  'non_retryable_status_codes',
  'continue_on_status_codes',
  'retryable_status_codes',
  'retry_on_status_codes',
  'continue_status_codes',
] as const
const hasFailoverRules = computed(() => {
  const rules = provider.value?.failover_rules
  if (!rules) return false
  return FAILOVER_RULE_ARRAY_KEYS.some(key => (rules[key]?.length || 0) > 0)
    || typeof rules.max_retries === 'number'
})

// Provider 级别代理配置状态
const proxyNodesStore = useProxyNodesStore()
const providerProxyPopoverOpen = ref(false)
const savingProviderProxy = ref(false)

// Key 级别代理配置状态
const savingProxyKeyId = ref<string | null>(null)
const proxyPopoverOpenKeyId = ref<string | null>(null)

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
  keyBatchImportDialogOpen.value ||
  keyPermissionsDialogOpen.value ||
  oauthAccountDialogOpen.value ||
  oauthKeyEditDialogOpen.value ||
  deleteKeyConfirmOpen.value ||
  modelFormDialogOpen.value ||
  batchAssignDialogOpen.value ||
  antigravityQuotaDialogOpen.value ||
  modelMappingTabRef.value?.dialogOpen
)

// 当前后端分页页内的密钥列表。key 通过 api_formats 字段确定支持的格式，endpoint 可能为 undefined。
const allKeys = computed(() => {
  return providerKeys.value.map(key => ({ key, endpoint: undefined as ProviderEndpointWithKeys | undefined }))
})

const availableKeyApiFormats = computed(() => {
  const formatSet = new Set<string>()

  for (const format of provider.value?.api_formats || []) {
    if (format) {
      formatSet.add(format)
    }
  }

  for (const endpoint of endpoints.value) {
    if (endpoint.api_format) {
      formatSet.add(endpoint.api_format)
    }
  }

  return sortApiFormats([...formatSet])
})

function syncCurrentSelections(
  nextEndpoints: ProviderEndpointWithKeys[] = endpoints.value,
  nextProviderKeys: EndpointAPIKey[] = providerKeys.value
) {
  if (currentEndpoint.value) {
    currentEndpoint.value = nextEndpoints.find(endpoint => endpoint.id === currentEndpoint.value?.id) ?? null
  }

  if (!editingKey.value) {
    return
  }

  const latestKeys: EndpointAPIKey[] = []
  const seenKeyIds = new Set<string>()

  for (const key of nextProviderKeys) {
    if (!seenKeyIds.has(key.id)) {
      seenKeyIds.add(key.id)
      latestKeys.push(key)
    }
  }

  for (const endpoint of nextEndpoints) {
    for (const key of endpoint.keys || []) {
      if (!seenKeyIds.has(key.id)) {
        seenKeyIds.add(key.id)
        latestKeys.push(key)
      }
    }
  }

  const latestEditingKey = latestKeys.find(key => key.id === editingKey.value?.id) || null
  editingKey.value = latestEditingKey

  if (!latestEditingKey) {
    keyFormDialogOpen.value = false
    keyPermissionsDialogOpen.value = false
    oauthKeyEditDialogOpen.value = false
  }
}

// ===== 账号列表后端分页 =====
const providerKeysTotal = ref(0)
const currentKeyPage = ref(1)
const keyPageSize = ref(DEFAULT_PROVIDER_KEYS_PAGE_SIZE)
const totalKeyPages = computed(() => Math.max(1, Math.ceil(providerKeysTotal.value / keyPageSize.value)))
const shouldPaginateKeys = computed(() => totalKeyPages.value > 1)
const paginatedKeys = computed(() => allKeys.value)

function getGlobalKeyIndex(localIdx: number): number {
  return localIdx
}

async function goToKeyPage(page: number) {
  const nextPage = Math.min(Math.max(page, 1), totalKeyPages.value)
  if (nextPage === currentKeyPage.value && providerKeys.value.length > 0) return
  await loadProviderKeysPage(nextPage)
}

// 合并监听 providerId 和 open，避免同一 tick 内两个 watcher 都触发导致重复请求
watch(
  [() => props.providerId, () => props.open],
  async ([newId, newOpen], [_oldId, oldOpen]) => {
    if (newOpen && newId) {
      if (!oldOpen || provider.value?.id !== newId) {
        currentKeyPage.value = 1
        providerKeysTotal.value = 0
      }
      const hasInitialProvider = props.initialProvider?.id === newId
      if (hasInitialProvider) {
        provider.value = props.initialProvider
        keyPageSize.value = getProviderKeysPageSize(provider.value?.provider_type)
        loading.value = false
      }
      void loadSystemFormatConversionConfig()
      if (!hasInitialProvider) {
        await loadProvider()
      }
      const endpointsPromise = loadEndpoints()
      // 仅在抽屉刚打开时启动倒计时
      if (newOpen && !oldOpen) {
        startCountdownTimer()
      }
      // 优先完成端点、密钥和模型的首屏数据，再请求计算量较大的映射预览。
      // 同时校验抽屉状态，避免关闭或切换 Provider 后启动无用请求。
      void endpointsPromise.then(() => {
        if (!props.open || props.providerId !== newId) return
        void loadMappingPreview()
        void autoRefreshQuotaInBackground()
      })
    } else if (!newOpen && oldOpen) {
      // 使在途请求失效，避免关闭后旧响应回写
      providerLoadRequestId += 1
      endpointsLoadRequestId += 1
      keysLoadRequestId += 1
      mappingPreviewLoadRequestId += 1

      // 停止倒计时定时器
      stopCountdownTimer()
      // 重置所有状态
      loading.value = false
      provider.value = null
      endpoints.value = []
      providerKeys.value = []  // 清空 Provider 级别的 keys
      providerKeysTotal.value = 0
      currentKeyPage.value = 1
      keyPageSize.value = DEFAULT_PROVIDER_KEYS_PAGE_SIZE
      providerModels.value = []
      providerMappingPreview.value = null
      loadingProviderEndpoints.value = false
      loadingProviderKeys.value = false
      loadingProviderModels.value = false
      loadingProviderMappingPreview.value = false

      // 重置所有对话框状态
      endpointDialogOpen.value = false
      keyFormDialogOpen.value = false
      keyBatchImportDialogOpen.value = false
      keyPermissionsDialogOpen.value = false
      oauthAccountDialogOpen.value = false
      oauthKeyEditDialogOpen.value = false
      deleteKeyConfirmOpen.value = false
      batchAssignDialogOpen.value = false
      antigravityQuotaDialogOpen.value = false
      antigravityQuotaDialogKey.value = null

      // 重置临时数据
      currentEndpoint.value = null
      editingKey.value = null
      keyToDelete.value = null

      // 清除已显示的密钥（安全考虑）
      revealedKeys.value.clear()
    }
  },
  { immediate: true },
)

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
    const updated = await updateProvider(provider.value.id, { enable_format_conversion: newValue })
    provider.value = updated
    showSuccess(legacyT(newValue ? '已启用格式转换' : '已禁用格式转换'))
    emit('refresh')
  } catch {
    showError(legacyT('切换格式转换失败'))
  }
}

function getProviderProxyNodeName(): string {
  const nodeId = provider.value?.proxy?.node_id
  if (!nodeId) return legacyT('未知节点')
  const node = proxyNodesStore.nodes.find(n => n.id === nodeId)
  return node ? node.name : `${nodeId.slice(0, 8)}...`
}

async function setProviderProxy(nodeId: string) {
  if (!provider.value) return
  savingProviderProxy.value = true
  try {
    const updated = await updateProvider(provider.value.id, {
      proxy: { node_id: nodeId, enabled: true },
    })
    provider.value = updated
    providerProxyPopoverOpen.value = false
    showSuccess(legacyT('代理节点已设置'))
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '设置代理失败'))
  } finally {
    savingProviderProxy.value = false
  }
}

async function clearProviderProxy() {
  if (!provider.value) return
  savingProviderProxy.value = true
  try {
    const updated = await updateProvider(provider.value.id, { proxy: null })
    provider.value = updated
    providerProxyPopoverOpen.value = false
    showSuccess(legacyT('已清除提供商代理'))
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '清除代理失败'))
  } finally {
    savingProviderProxy.value = false
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

  // OAuth 账号型提供商：打开 OAuth 账号对话框
  if (isOAuthAccountProviderType(provider.value?.provider_type)) {
    oauthAccountDialogOpen.value = true
  } else {
    // 密钥型提供商（custom/vertex_ai）：打开密钥表单对话框
    handleAddKey(endpoints.value[0])
  }
}

function handleEditKey(endpoint: ProviderEndpoint | undefined, key: EndpointAPIKey) {
  currentEndpoint.value = endpoint || null
  editingKey.value = key
  // OAuth 密钥使用专门的编辑对话框
  if (canEditOAuthCredential(key)) {
    oauthKeyEditDialogOpen.value = true
  } else {
    keyFormDialogOpen.value = true
  }
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

    if (result.auth_type === 'service_account' && result.auth_config) {
      // Service Account 类型：复制 auth_config JSON
      textToCopy = typeof result.auth_config === 'string'
        ? result.auth_config
        : JSON.stringify(result.auth_config, null, 2)
    } else {
      // API Key 类型：复制 api_key
      textToCopy = result.api_key || ''
    }

    revealedKeys.value.set(key.id, textToCopy)
    copyToClipboard(textToCopy)
  } catch (err: unknown) {
    showError(localizedApiError(err, '获取密钥失败'), legacyT('错误'))
  }
}

// 下载 OAuth 凭据文件（后端统一导出，前端只负责下载）
async function downloadRefreshToken(key: EndpointAPIKey) {
  try {
    const data = await exportKey(key.id)
    const providerType = provider.value?.provider_type || 'unknown'
    const safeName = (data.email || key.name || key.id.slice(0, 8)).replace(/[^a-zA-Z0-9_\-@.]/g, '_')

    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `aether_${providerType}_${safeName}.json`
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
    URL.revokeObjectURL(url)
  } catch (err: unknown) {
    showError(localizedApiError(err, '导出失败'), legacyT('错误'))
  }
}


function handleDeleteKey(key: EndpointAPIKey) {
  keyToDelete.value = key
  deleteKeyConfirmOpen.value = true
}

function formatDeleteKeyConfirmDescription(): string {
  const keyName = keyToDelete.value?.api_key_masked || keyToDelete.value?.name || ''
  return locale.value === 'en-US'
    ? `Delete key ${keyName}?`
    : `确定要删除密钥 ${keyName} 吗？`
}

async function confirmDeleteKey() {
  if (!keyToDelete.value) return

  const keyId = keyToDelete.value.id
  deleteKeyConfirmOpen.value = false
  keyToDelete.value = null

  try {
    await deleteEndpointKey(keyId)
    showSuccess(legacyT('密钥已删除'))
    // 刷新端点列表及模型数据（删除 Key 触发自动解除模型关联）
    await loadEndpoints()
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '删除密钥失败'), legacyT('错误'))
  }
}

async function handleRecoverKey(key: EndpointAPIKey) {
  try {
    const result = await recoverKeyHealth(key.id)
    showSuccess(legacyT(result.message || 'Key已完全恢复'))
    await loadEndpoints()
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, 'Key恢复失败'), legacyT('错误'))
  }
}

async function handleRefreshOAuth(key: EndpointAPIKey) {
  if (refreshingOAuthKeyId.value) return
  refreshingOAuthKeyId.value = key.id
  try {
    const result = await refreshProviderOAuth(key.id)
    const refreshedExpiresAt = typeof result.expires_at === 'number' ? result.expires_at : null
    let refreshedKey: EndpointAPIKey | null = null
    // 更新本地数据
    const keyInList = providerKeys.value.find(k => k.id === key.id)
    if (keyInList) {
      keyInList.oauth_expires_at = refreshedExpiresAt
    }
    // 只重新加载当前 keys 页，避免整个表格刷新
    if (props.providerId) {
      const freshPage = await getProviderKeysPage(props.providerId, {
        page: currentKeyPage.value,
        page_size: keyPageSize.value,
      }).catch(() => null)
      if (freshPage) {
        const mergedKeys = freshPage.keys.map((item) => {
          if (item.id !== key.id) return item
          if (refreshedExpiresAt == null) return item
          if (typeof item.oauth_expires_at === 'number' && item.oauth_expires_at >= refreshedExpiresAt) {
            return item
          }
          return { ...item, oauth_expires_at: refreshedExpiresAt }
        })
        providerKeys.value = mergedKeys
        providerKeysTotal.value = freshPage.total
        currentKeyPage.value = freshPage.page
        keyPageSize.value = freshPage.page_size
        syncCurrentSelections(endpoints.value, mergedKeys)
        refreshedKey = mergedKeys.find(item => item.id === key.id) ?? null
      }
    }
    const feedback = getOAuthRefreshFeedback({
      accountStateRecheckAttempted: result.account_state_recheck_attempted,
      accountStateRecheckError: result.account_state_recheck_error,
      snapshot: refreshedKey,
    })
    if (feedback.tone === 'warning') {
      showWarning(legacyT(feedback.message))
    } else {
      showSuccess(legacyT(feedback.message))
    }
    // Antigravity：token 刷新后可能完成了账号激活，触发配额获取
    // （不 emit('refresh')，避免触发全局 provider 余额刷新）
    void autoRefreshQuotaInBackground()
  } catch (err: unknown) {
    showError(localizedApiError(err, 'Token 刷新失败'), legacyT('错误'))
  } finally {
    refreshingOAuthKeyId.value = null
  }
}

// 判断是否为账号级别的封禁（刷新 token 无法修复）
function isAccountLevelBlock(key: EndpointAPIKey): boolean {
  const account = getAccountStatusDisplay(key)
  const oauth = getOAuthStatusDisplay(key, countdownTick.value)
  return account.blocked && !oauth?.isInvalid
}

function formatClearOAuthInvalidConfirmMessage(key: EndpointAPIKey): string {
  const keyName = key.name || key.id.slice(0, 8)
  return locale.value === 'en-US'
    ? `Confirm account "${keyName}" has completed manual verification? After clearing, the system will re-evaluate this key using the current manual switch and scheduling state.`
    : `确认账号 "${keyName}" 已手动完成验证？清除后系统会按当前手动开关和调度状态重新评估该 Key。`
}

// 清除 OAuth 失效标记
async function handleClearOAuthInvalid(key: EndpointAPIKey) {
  if (clearingOAuthInvalidKeyId.value) return

  const confirmed = await confirm({
    title: legacyT('清除账号异常标记'),
    message: formatClearOAuthInvalidConfirmMessage(key),
    confirmText: legacyT('确认清除'),
    variant: 'default',
  })
  if (!confirmed) return

  clearingOAuthInvalidKeyId.value = key.id
  try {
    await clearOAuthInvalid(key.id)
    showSuccess(legacyT('已清除 OAuth 异常标记'))
    // 更新本地数据
    const keyInList = providerKeys.value.find(k => k.id === key.id)
    if (keyInList) {
      keyInList.oauth_invalid_at = null
      keyInList.oauth_invalid_reason = null
      if (keyInList.status_snapshot) {
        keyInList.status_snapshot = {
          ...keyInList.status_snapshot,
          oauth: {
            ...keyInList.status_snapshot.oauth,
            code: 'none',
            label: null,
            reason: null,
            invalid_at: null,
            requires_reauth: false,
          },
          account: {
            ...keyInList.status_snapshot.account,
            code: 'ok',
            label: null,
            reason: null,
            blocked: false,
            recoverable: false,
          },
        }
      }
    }
    await loadEndpoints()
  } catch (err: unknown) {
    showError(localizedApiError(err, '清除失败'), legacyT('错误'))
  } finally {
    clearingOAuthInvalidKeyId.value = null
  }
}

function codexResetCreditOutcomeFeedback(
  result: Awaited<ReturnType<typeof consumeCodexResetCredit>>,
): { tone: 'success' | 'warning'; message: string } {
  switch (result.outcome) {
    case 'reset':
      return { tone: 'success', message: '已使用 Codex 重置机会，并刷新账号配额' }
    case 'already_redeemed':
      return { tone: 'success', message: '本次重置请求已处理，账号配额已刷新' }
    case 'nothing_to_reset':
      return { tone: 'warning', message: '当前没有需要重置的 Codex 额度窗口' }
    case 'no_credit':
      return { tone: 'warning', message: '当前没有可用的 Codex 重置机会' }
    default:
      return { tone: 'warning', message: '重置请求已返回，但结果类型未知，请查看最新账号配额' }
  }
}

async function handleConsumeCodexResetCredit(key: EndpointAPIKey) {
  if (!canConsumeCodexResetCredit(key)) return

  const earliest = getVisibleCodexResetCreditItems(key)[0]
  const detailMessage = earliest
    ? `\n当前最早过期项：${earliest.displayKey}，${formatCodexResetCreditExpiresAt(earliest.expiresAt)} 过期。`
    : ''
  const confirmed = await confirm({
    title: legacyT('确认使用 Codex 重置机会'),
    message: `${legacyT('将消耗 1 次 Codex 重置机会。操作完成后会重新刷新账号配额状态。')}${detailMessage}`,
    confirmText: legacyT('确认重置'),
    cancelText: legacyT('取消'),
    variant: 'warning',
  })
  if (!confirmed) return

  consumingCodexResetCreditKeyId.value = key.id
  try {
    const idempotencyKey = createCodexResetCreditIdempotencyKey()
    const result = await consumeCodexResetCredit(key.id, {
      idempotency_key: idempotencyKey,
    })
    applyQuotaResults([{
      key_id: result.key_id,
      status: result.refresh_status === 'success' ? 'success' : result.status,
      metadata: result.metadata,
      quota_snapshot: result.quota_snapshot,
    }])

    const feedback = codexResetCreditOutcomeFeedback(result)
    if (feedback.tone === 'success') {
      showSuccess(legacyT(feedback.message))
    } else {
      showWarning(legacyT(feedback.message))
    }
    if (result.refresh_status === 'failed') {
      showWarning(legacyT(result.refresh_error || '重置请求已处理，但最新配额刷新失败'))
    }
  } catch (err: unknown) {
    showError(localizedApiError(err, 'Codex 重置机会使用失败'), legacyT('错误'))
  } finally {
    consumingCodexResetCreditKeyId.value = null
  }
}

// Codex / Gemini CLI / Antigravity / Kiro / Windsurf / ChatGPT Web：打开抽屉后自动后台刷新（配额缓存缺失/过期，或 Token 即将过期时触发）
const AUTO_QUOTA_REFRESH_STALE_SECONDS = 5 * 60
// 与后端 OAuth 懒刷新阈值对齐：到期前 2 分钟内视为需要刷新
const AUTO_TOKEN_REFRESH_SKEW_SECONDS = 2 * 60

function quotaSnapshotHasDisplayData(quota: QuotaStatusSnapshot | null | undefined): boolean {
  if (!quota) return false
  return Boolean(
    (typeof quota.code === 'string' && quota.code.trim().toLowerCase() !== 'unknown')
    || quota.updated_at != null
    || quota.observed_at != null
    || quota.usage_ratio != null
    || (Array.isArray(quota.windows) && quota.windows.length > 0)
    || quota.credits,
  )
}

function getQuotaSnapshotForProvider(
  key: EndpointAPIKey,
  providerType: 'codex' | 'kiro' | 'windsurf' | 'antigravity' | 'chatgpt_web' | 'gemini_cli' | 'grok',
): QuotaStatusSnapshot | null {
  const quota = key.status_snapshot?.quota
  if (!quota) return null

  const snapshotProviderType = quota.provider_type?.trim().toLowerCase()
  if (snapshotProviderType) {
    return snapshotProviderType === providerType ? quota : null
  }

  return quotaSnapshotHasDisplayData(quota) ? quota : null
}

function getQuotaSnapshotUpdatedAt(quota: QuotaStatusSnapshot | null | undefined): number | undefined {
  const updatedAt = quota?.updated_at ?? quota?.observed_at
  return typeof updatedAt === 'number' ? updatedAt : undefined
}

function getQuotaWindow(
  quota: QuotaStatusSnapshot | null | undefined,
  code: string,
): QuotaWindowSnapshot | null {
  const windows = quota?.windows
  if (!Array.isArray(windows)) return null
  return windows.find(window => String(window?.code || '').trim().toLowerCase() === code.trim().toLowerCase()) ?? null
}

function getQuotaWindowUsedPercent(window: QuotaWindowSnapshot | null | undefined): number | undefined {
  if (!window) return undefined
  if (typeof window.used_ratio === 'number') {
    return Math.max(Math.min(window.used_ratio * 100, 100), 0)
  }
  if (typeof window.remaining_ratio === 'number') {
    return Math.max(Math.min((1 - window.remaining_ratio) * 100, 100), 0)
  }
  return undefined
}

function getQuotaWindowRemainingPercent(window: QuotaWindowSnapshot | null | undefined): number | undefined {
  if (!window) return undefined
  if (typeof window.remaining_ratio === 'number') {
    return Math.max(Math.min(window.remaining_ratio * 100, 100), 0)
  }
  if (typeof window.used_ratio === 'number') {
    return Math.max(Math.min((1 - window.used_ratio) * 100, 100), 0)
  }
  return undefined
}

function getQuotaWindowResetAt(window: QuotaWindowSnapshot | null | undefined): number | undefined {
  return typeof window?.reset_at === 'number' ? window.reset_at : undefined
}

function getQuotaWindowResetSeconds(window: QuotaWindowSnapshot | null | undefined): number | undefined {
  return typeof window?.reset_seconds === 'number' ? window.reset_seconds : undefined
}

function getQuotaWindowByScope(
  quota: QuotaStatusSnapshot | null | undefined,
  scope: string,
): QuotaWindowSnapshot[] {
  const windows = quota?.windows
  if (!Array.isArray(windows)) return []
  return windows.filter(window => String(window?.scope || '').trim().toLowerCase() === scope.trim().toLowerCase())
}

function getQuotaWindowLiveResetSeconds(
  quota: QuotaStatusSnapshot | null | undefined,
  window: QuotaWindowSnapshot | null | undefined,
): number | null {
  if (!window) return null

  const now = Math.floor(Date.now() / 1000)
  if (typeof window.reset_at === 'number') {
    return Math.max(window.reset_at - now, 0)
  }

  if (typeof window.reset_seconds === 'number') {
    const updatedAt = getQuotaSnapshotUpdatedAt(quota)
    const elapsed = typeof updatedAt === 'number' ? Math.max(now - updatedAt, 0) : 0
    return Math.max(window.reset_seconds - elapsed, 0)
  }

  return null
}

function copyCodexNumberField(
  target: CodexUpstreamMetadata,
  source: CodexUpstreamMetadata,
  field: keyof CodexUpstreamMetadata,
) {
  const value = source[field]
  if (typeof value === 'number' && Number.isFinite(value)) {
    ;(target[field] as number | undefined) = value
  }
}

function getCodexQuotaDisplayFromMetadata(metadata: CodexUpstreamMetadata | null | undefined): CodexUpstreamMetadata | null {
  if (!metadata) return null

  const display: CodexUpstreamMetadata = {}
  if (metadata.plan_type) display.plan_type = metadata.plan_type

  const numberFields: (keyof CodexUpstreamMetadata)[] = [
    'updated_at',
    'primary_used_percent',
    'primary_reset_seconds',
    'primary_reset_after_seconds',
    'primary_reset_at',
    'primary_window_minutes',
    'secondary_used_percent',
    'secondary_reset_seconds',
    'secondary_reset_after_seconds',
    'secondary_reset_at',
    'secondary_window_minutes',
    'spark_primary_used_percent',
    'spark_primary_reset_seconds',
    'spark_primary_reset_after_seconds',
    'spark_primary_reset_at',
    'spark_primary_window_minutes',
    'spark_secondary_used_percent',
    'spark_secondary_reset_seconds',
    'spark_secondary_reset_after_seconds',
    'spark_secondary_reset_at',
    'spark_secondary_window_minutes',
    'credits_balance',
  ]
  numberFields.forEach(field => copyCodexNumberField(display, metadata, field))
  if (metadata.has_credits !== undefined) display.has_credits = metadata.has_credits
  if (metadata.reset_credits) display.reset_credits = metadata.reset_credits

  return Object.keys(display).length > 0 ? display : null
}

function getCodexQuotaDisplayFromSnapshot(quota: QuotaStatusSnapshot | null | undefined): CodexUpstreamMetadata | null {
  if (!quota) return null

  const display: CodexUpstreamMetadata = {}
  const updatedAt = getQuotaSnapshotUpdatedAt(quota)
  if (updatedAt !== undefined) display.updated_at = updatedAt
  if (quota.plan_type) display.plan_type = quota.plan_type

  const primaryWindow = getQuotaWindow(quota, 'weekly')
  const primaryUsedPercent = getQuotaWindowUsedPercent(primaryWindow)
  if (primaryUsedPercent !== undefined) display.primary_used_percent = primaryUsedPercent
  const primaryResetAt = getQuotaWindowResetAt(primaryWindow)
  if (primaryResetAt !== undefined) display.primary_reset_at = primaryResetAt
  const primaryResetSeconds = getQuotaWindowResetSeconds(primaryWindow)
  if (primaryResetSeconds !== undefined) display.primary_reset_seconds = primaryResetSeconds
  if (typeof primaryWindow?.window_minutes === 'number') {
    display.primary_window_minutes = primaryWindow.window_minutes
  }

  const secondaryWindow = getQuotaWindow(quota, '5h')
  const secondaryUsedPercent = getQuotaWindowUsedPercent(secondaryWindow)
  if (secondaryUsedPercent !== undefined) display.secondary_used_percent = secondaryUsedPercent
  const secondaryResetAt = getQuotaWindowResetAt(secondaryWindow)
  if (secondaryResetAt !== undefined) display.secondary_reset_at = secondaryResetAt
  const secondaryResetSeconds = getQuotaWindowResetSeconds(secondaryWindow)
  if (secondaryResetSeconds !== undefined) display.secondary_reset_seconds = secondaryResetSeconds
  if (typeof secondaryWindow?.window_minutes === 'number') {
    display.secondary_window_minutes = secondaryWindow.window_minutes
  }

  const sparkPrimaryWindow = getQuotaWindow(quota, 'spark_5h')
  const sparkPrimaryUsedPercent = getQuotaWindowUsedPercent(sparkPrimaryWindow)
  if (sparkPrimaryUsedPercent !== undefined) display.spark_primary_used_percent = sparkPrimaryUsedPercent
  const sparkPrimaryResetAt = getQuotaWindowResetAt(sparkPrimaryWindow)
  if (sparkPrimaryResetAt !== undefined) display.spark_primary_reset_at = sparkPrimaryResetAt
  const sparkPrimaryResetSeconds = getQuotaWindowResetSeconds(sparkPrimaryWindow)
  if (sparkPrimaryResetSeconds !== undefined) display.spark_primary_reset_seconds = sparkPrimaryResetSeconds
  if (typeof sparkPrimaryWindow?.window_minutes === 'number') {
    display.spark_primary_window_minutes = sparkPrimaryWindow.window_minutes
  }

  const sparkSecondaryWindow = getQuotaWindow(quota, 'spark_weekly')
  const sparkSecondaryUsedPercent = getQuotaWindowUsedPercent(sparkSecondaryWindow)
  if (sparkSecondaryUsedPercent !== undefined) display.spark_secondary_used_percent = sparkSecondaryUsedPercent
  const sparkSecondaryResetAt = getQuotaWindowResetAt(sparkSecondaryWindow)
  if (sparkSecondaryResetAt !== undefined) display.spark_secondary_reset_at = sparkSecondaryResetAt
  const sparkSecondaryResetSeconds = getQuotaWindowResetSeconds(sparkSecondaryWindow)
  if (sparkSecondaryResetSeconds !== undefined) display.spark_secondary_reset_seconds = sparkSecondaryResetSeconds
  if (typeof sparkSecondaryWindow?.window_minutes === 'number') {
    display.spark_secondary_window_minutes = sparkSecondaryWindow.window_minutes
  }
  if (quota.reset_credits) display.reset_credits = quota.reset_credits

  return Object.keys(display).length > 0 ? display : null
}

function codexDisplayHasResetCredits(display: CodexUpstreamMetadata | null | undefined): boolean {
  const count = display?.reset_credits?.available_count
  return typeof count === 'number' && Number.isFinite(count)
}

function getCodexQuotaDisplay(key: EndpointAPIKey): CodexUpstreamMetadata | null {
  const snapshotDisplay = getCodexQuotaDisplayFromSnapshot(getQuotaSnapshotForProvider(key, 'codex'))
  const metadataDisplay = getCodexQuotaDisplayFromMetadata(key.upstream_metadata?.codex)
  return mergeCodexQuotaDisplays(snapshotDisplay, metadataDisplay)
}

function hasCodexQuotaDisplayData(key: EndpointAPIKey): boolean {
  const codex = getCodexQuotaDisplay(key)
  return !!codex && (
    codex.primary_used_percent !== undefined
    || codex.secondary_used_percent !== undefined
    || codex.spark_primary_used_percent !== undefined
    || codex.spark_secondary_used_percent !== undefined
    || codexDisplayHasResetCredits(codex)
  )
}

function hasCodexSparkQuotaDisplayData(key: EndpointAPIKey): boolean {
  const codex = getCodexQuotaDisplay(key)
  return !!codex && (
    codex.spark_primary_used_percent !== undefined
    || codex.spark_secondary_used_percent !== undefined
  )
}

function getCodexResetCreditsDisplay(key: EndpointAPIKey): QuotaResetCreditsSnapshot | null {
  return getCodexQuotaDisplay(key)?.reset_credits ?? null
}

function getCodexResetCreditAvailableCount(key: EndpointAPIKey): number | null {
  return getCodexResetCreditAvailableCountFromSnapshot(getCodexResetCreditsDisplay(key))
}

function formatCodexResetCreditCount(key: EndpointAPIKey): string {
  return formatCodexResetCreditCountLabel(getCodexResetCreditAvailableCount(key))
}

function getVisibleCodexResetCreditItems(key: EndpointAPIKey) {
  return getVisibleCodexResetCreditItemsFromSnapshot(getCodexResetCreditsDisplay(key))
}

function canConsumeCodexResetCredit(key: EndpointAPIKey): boolean {
  return provider.value?.provider_type === 'codex'
    && getCodexResetCreditAvailableCount(key) !== null
    && (getCodexResetCreditAvailableCount(key) ?? 0) > 0
    && !consumingCodexResetCreditKeyId.value
}

function getKiroQuotaDisplay(key: EndpointAPIKey): KiroUpstreamMetadata | null {
  const quota = getQuotaSnapshotForProvider(key, 'kiro')
  if (!quota) return null

  const display: KiroUpstreamMetadata = {}
  const updatedAt = getQuotaSnapshotUpdatedAt(quota)
  if (updatedAt !== undefined) display.updated_at = updatedAt
  if (quota.plan_type) display.subscription_title = quota.plan_type

  if (String(quota.code || '').trim().toLowerCase() === 'banned') {
    display.is_banned = true
    if (quota.reason) display.ban_reason = quota.reason
    if (updatedAt !== undefined) display.banned_at = updatedAt
  }

  const usageWindow =
    getQuotaWindow(quota, 'usage')
    ?? getQuotaWindowByScope(quota, 'account')[0]
    ?? null
  if (usageWindow) {
    const usedPercent = getQuotaWindowUsedPercent(usageWindow)
    if (usedPercent !== undefined) display.usage_percentage = usedPercent
    if (typeof usageWindow.used_value === 'number') display.current_usage = usageWindow.used_value
    if (typeof usageWindow.limit_value === 'number') display.usage_limit = usageWindow.limit_value
    if (typeof usageWindow.remaining_value === 'number') display.remaining = usageWindow.remaining_value

    const nextResetAt =
      getQuotaWindowResetAt(usageWindow)
      ?? (() => {
        const resetSeconds = getQuotaWindowResetSeconds(usageWindow)
        if (updatedAt === undefined || resetSeconds === undefined) return undefined
        return updatedAt + resetSeconds
      })()
    if (nextResetAt !== undefined) display.next_reset_at = nextResetAt
  }

  return Object.keys(display).length > 0 ? display : null
}

function hasKiroQuotaDisplayData(key: EndpointAPIKey): boolean {
  const kiro = getKiroQuotaDisplay(key)
  return !!kiro && (kiro.usage_percentage !== undefined || kiro.usage_limit !== undefined)
}

type GrokQuotaDisplay = GrokUpstreamMetadata & {
  usage_percentage?: number
  usage_limit?: number
  current_usage?: number
  remaining?: number
  next_reset_at?: number
}

function getGrokQuotaDisplay(key: EndpointAPIKey): GrokQuotaDisplay | null {
  const quota = getQuotaSnapshotForProvider(key, 'grok')
  if (!quota) return null

  const display: GrokQuotaDisplay = {}
  const updatedAt = getQuotaSnapshotUpdatedAt(quota)
  if (updatedAt !== undefined) display.updated_at = updatedAt
  if (quota.plan_type) display.plan_type = quota.plan_type
  if (quota.pool_tier) display.pool_tier = quota.pool_tier

  const code = String(quota.code || '').trim().toLowerCase()
  if (code === 'banned' || code === 'forbidden') {
    display.is_banned = true
    if (quota.reason) display.ban_reason = quota.reason
  }

  const usageWindow =
    getQuotaWindow(quota, 'usage')
    ?? getQuotaWindowByScope(quota, 'account')[0]
    ?? getQuotaWindowByScope(quota, 'model')
      .map(window => ({
        window,
        remainingPercent: getQuotaWindowRemainingPercent(window),
      }))
      .filter((item): item is { window: QuotaWindowSnapshot, remainingPercent: number } => item.remainingPercent !== undefined)
      .sort((a, b) => a.remainingPercent - b.remainingPercent)[0]?.window
    ?? null
  if (usageWindow) {
    const usedPercent = getQuotaWindowUsedPercent(usageWindow)
    if (usedPercent !== undefined) display.usage_percentage = usedPercent
    if (typeof usageWindow.used_value === 'number') display.current_usage = usageWindow.used_value
    if (typeof usageWindow.limit_value === 'number') display.usage_limit = usageWindow.limit_value
    if (typeof usageWindow.remaining_value === 'number') display.remaining = usageWindow.remaining_value

    const nextResetAt =
      getQuotaWindowResetAt(usageWindow)
      ?? (() => {
        const resetSeconds = getQuotaWindowResetSeconds(usageWindow)
        if (updatedAt === undefined || resetSeconds === undefined) return undefined
        return updatedAt + resetSeconds
      })()
    if (nextResetAt !== undefined) display.next_reset_at = nextResetAt
  }

  return Object.keys(display).length > 0 ? display : null
}

type WindsurfQuotaDisplay = WindsurfUpstreamMetadata & {
  daily_used_percent?: number
  weekly_used_percent?: number
}

function getWindsurfQuotaDisplay(key: EndpointAPIKey): WindsurfQuotaDisplay | null {
  const quota = getQuotaSnapshotForProvider(key, 'windsurf')
  const upstream = key.upstream_metadata?.windsurf
  if (!quota && !upstream) return null

  const display: WindsurfQuotaDisplay = {}
  const updatedAt = getQuotaSnapshotUpdatedAt(quota) ?? upstream?.updated_at
  if (updatedAt !== undefined) display.updated_at = updatedAt
  if (quota?.plan_type) display.plan_name = quota.plan_type
  else if (upstream?.plan_name) display.plan_name = upstream.plan_name
  if (quota?.reason) display.last_error = quota.reason
  else if (upstream?.last_error) display.last_error = upstream.last_error
  if (typeof quota?.allowed_models_count === 'number') display.allowed_models_count = quota.allowed_models_count
  else if (typeof upstream?.allowed_models_count === 'number') display.allowed_models_count = upstream.allowed_models_count
  if (quota?.rate_limit) display.rate_limit = quota.rate_limit
  else if (upstream?.rate_limit) display.rate_limit = upstream.rate_limit
  if (Array.isArray(upstream?.models)) display.models = upstream.models

  const dailyWindow = getQuotaWindow(quota, 'daily')
  const dailyRemaining = getQuotaWindowRemainingPercent(dailyWindow)
  const dailyUsed = getQuotaWindowUsedPercent(dailyWindow)
  if (dailyRemaining !== undefined) display.daily_remaining_percent = dailyRemaining
  else if (typeof upstream?.daily_remaining_percent === 'number') display.daily_remaining_percent = upstream.daily_remaining_percent
  if (dailyUsed !== undefined) display.daily_used_percent = dailyUsed
  else if (typeof upstream?.daily_remaining_percent === 'number') display.daily_used_percent = Math.max(100 - upstream.daily_remaining_percent, 0)
  const dailyResetAt = getQuotaWindowResetAt(dailyWindow)
  if (dailyResetAt !== undefined) display.daily_reset_at = dailyResetAt
  else if (typeof upstream?.daily_reset_at === 'number') display.daily_reset_at = upstream.daily_reset_at

  const weeklyWindow = getQuotaWindow(quota, 'weekly')
  const weeklyRemaining = getQuotaWindowRemainingPercent(weeklyWindow)
  const weeklyUsed = getQuotaWindowUsedPercent(weeklyWindow)
  if (weeklyRemaining !== undefined) display.weekly_remaining_percent = weeklyRemaining
  else if (typeof upstream?.weekly_remaining_percent === 'number') display.weekly_remaining_percent = upstream.weekly_remaining_percent
  if (weeklyUsed !== undefined) display.weekly_used_percent = weeklyUsed
  else if (typeof upstream?.weekly_remaining_percent === 'number') display.weekly_used_percent = Math.max(100 - upstream.weekly_remaining_percent, 0)
  const weeklyResetAt = getQuotaWindowResetAt(weeklyWindow)
  if (weeklyResetAt !== undefined) display.weekly_reset_at = weeklyResetAt
  else if (typeof upstream?.weekly_reset_at === 'number') display.weekly_reset_at = upstream.weekly_reset_at

  const promptWindow = getQuotaWindow(quota, 'prompt')
  if (typeof promptWindow?.used_value === 'number') display.prompt_used = promptWindow.used_value
  else if (typeof upstream?.prompt_used === 'number') display.prompt_used = upstream.prompt_used
  if (typeof promptWindow?.limit_value === 'number') display.prompt_limit = promptWindow.limit_value
  else if (typeof upstream?.prompt_limit === 'number') display.prompt_limit = upstream.prompt_limit
  if (typeof promptWindow?.remaining_value === 'number') display.prompt_remaining = promptWindow.remaining_value
  else if (typeof upstream?.prompt_remaining === 'number') display.prompt_remaining = upstream.prompt_remaining

  const flexWindow = getQuotaWindow(quota, 'flex')
  if (typeof flexWindow?.used_value === 'number') display.flex_used = flexWindow.used_value
  else if (typeof upstream?.flex_used === 'number') display.flex_used = upstream.flex_used
  if (typeof flexWindow?.limit_value === 'number') display.flex_limit = flexWindow.limit_value
  else if (typeof upstream?.flex_limit === 'number') display.flex_limit = upstream.flex_limit
  if (typeof flexWindow?.remaining_value === 'number') display.flex_remaining = flexWindow.remaining_value
  else if (typeof upstream?.flex_remaining === 'number') display.flex_remaining = upstream.flex_remaining

  return Object.keys(display).length > 0 ? display : null
}

function hasGrokQuotaDisplayData(key: EndpointAPIKey): boolean {
  const grok = getGrokQuotaDisplay(key)
  return !!grok && (grok.usage_percentage !== undefined || grok.usage_limit !== undefined)
}

function hasWindsurfQuotaDisplayData(key: EndpointAPIKey): boolean {
  const windsurf = getWindsurfQuotaDisplay(key)
  return !!windsurf && (
    windsurf.daily_remaining_percent !== undefined
    || windsurf.weekly_remaining_percent !== undefined
    || windsurf.prompt_limit !== undefined
    || windsurf.flex_limit !== undefined
    || windsurf.allowed_models_count !== undefined
    || windsurf.rate_limit !== undefined
    || !!windsurf.last_error
    || (Array.isArray(windsurf.models) && windsurf.models.length > 0)
  )
}

function isWindsurfUnavailableKey(key: EndpointAPIKey): boolean {
  const code = String(getQuotaSnapshotForProvider(key, 'windsurf')?.code || '').trim().toLowerCase()
  return code === 'banned' || code === 'forbidden' || code === 'quarantined'
}

function getPositiveQuotaNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : undefined
}

function windsurfCooldownHasPositiveReset(key: EndpointAPIKey): boolean {
  const quota = getQuotaSnapshotForProvider(key, 'windsurf')
  const rateLimit = quota?.rate_limit
  if (rateLimit && typeof rateLimit === 'object') {
    const retryAfterMs =
      getPositiveQuotaNumber(rateLimit.retry_after_ms)
      ?? getPositiveQuotaNumber(rateLimit.retryAfterMs)
    if (retryAfterMs !== undefined) return true
  }

  const rateLimitWindow = getQuotaWindow(quota, 'rate_limit')
  return (
    getPositiveQuotaNumber(rateLimitWindow?.reset_seconds) !== undefined
    || getPositiveQuotaNumber(rateLimitWindow?.reset_at) !== undefined
  )
}

function isWindsurfExhaustedKey(key: EndpointAPIKey): boolean {
  const code = String(getQuotaSnapshotForProvider(key, 'windsurf')?.code || '').trim().toLowerCase()
  if (code === 'cooldown') return windsurfCooldownHasPositiveReset(key)
  return code === 'exhausted' || code === 'rate_limited' || code === 'rate_limit'
}

function getWindsurfQuotaStatusLabel(key: EndpointAPIKey): string {
  const quota = getQuotaSnapshotForProvider(key, 'windsurf')
  const label = quota?.label?.trim()
  if (label) return label
  const code = String(quota?.code || '').trim().toLowerCase()
  if (code === 'cooldown') return legacyT('冷却中')
  return code === 'rate_limited' || code === 'rate_limit' ? legacyT('速率受限') : legacyT('额度耗尽')
}

function getWindsurfModelPreview(key: EndpointAPIKey): string | null {
  const models = getWindsurfQuotaDisplay(key)?.models
  if (!Array.isArray(models) || models.length === 0) return null
  return models
    .slice(0, 3)
    .map(model => (model.label || model.model_uid || '').trim())
    .filter(Boolean)
    .join(' / ') || null
}

function hasFiniteNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value)
}

function hasWindsurfPromptQuota(key: EndpointAPIKey): boolean {
  return hasFiniteNumber(getWindsurfQuotaDisplay(key)?.prompt_limit)
}

function hasWindsurfFlexQuota(key: EndpointAPIKey): boolean {
  return hasFiniteNumber(getWindsurfQuotaDisplay(key)?.flex_limit)
}

function hasWindsurfModelCount(key: EndpointAPIKey): boolean {
  return hasFiniteNumber(getWindsurfQuotaDisplay(key)?.allowed_models_count)
}

function hasWindsurfModelPreview(key: EndpointAPIKey): boolean {
  return !!getWindsurfModelPreview(key)
}

type ChatGPTWebQuotaDisplay = ChatGPTWebUpstreamMetadata & {
  image_quota_remaining_percent?: number
  image_quota_used_percent?: number
}

function getChatGPTWebQuotaDisplay(key: EndpointAPIKey): ChatGPTWebQuotaDisplay | null {
  const quota = getQuotaSnapshotForProvider(key, 'chatgpt_web')
  if (!quota) return null

  const display: ChatGPTWebQuotaDisplay = {}
  const updatedAt = getQuotaSnapshotUpdatedAt(quota)
  if (updatedAt !== undefined) display.updated_at = updatedAt
  if (quota.plan_type) display.plan_type = quota.plan_type
  if (quota.code === 'exhausted' || quota.code === 'banned') display.image_quota_blocked = true

  const imageWindow =
    getQuotaWindow(quota, 'image_gen')
    ?? getQuotaWindowByScope(quota, 'account')[0]
    ?? null
  if (imageWindow) {
    const remainingValue = typeof imageWindow.remaining_value === 'number' ? imageWindow.remaining_value : undefined
    const limitValue = typeof imageWindow.limit_value === 'number' ? imageWindow.limit_value : undefined
    const usedValue = typeof imageWindow.used_value === 'number' ? imageWindow.used_value : undefined
    const remainingPercent = getQuotaWindowRemainingPercent(imageWindow)
    const usedPercent = getQuotaWindowUsedPercent(imageWindow)

    if (remainingValue !== undefined) display.image_quota_remaining = remainingValue
    if (limitValue !== undefined) display.image_quota_total = limitValue
    if (usedValue !== undefined) display.image_quota_used = usedValue
    if (remainingPercent !== undefined) display.image_quota_remaining_percent = remainingPercent
    if (usedPercent !== undefined) display.image_quota_used_percent = usedPercent
    if (typeof imageWindow.reset_at === 'number') display.image_quota_reset_at = imageWindow.reset_at
    if (typeof imageWindow.reset_seconds === 'number') {
      const resetAt = updatedAt === undefined ? undefined : updatedAt + imageWindow.reset_seconds
      if (resetAt !== undefined && display.image_quota_reset_at === undefined) {
        display.image_quota_reset_at = resetAt
      }
    }
  }

  return Object.keys(display).length > 0 ? display : null
}

function hasChatGPTWebQuotaDisplayData(key: EndpointAPIKey): boolean {
  const display = getChatGPTWebQuotaDisplay(key)
  return !!display && (
    display.image_quota_remaining_percent !== undefined
    || display.image_quota_total !== undefined
    || display.image_quota_used !== undefined
  )
}

function getChatGPTWebQuotaUsedPercent(key: EndpointAPIKey): number {
  const display = getChatGPTWebQuotaDisplay(key)
  if (!display) return 0
  if (typeof display.image_quota_used_percent === 'number') return display.image_quota_used_percent
  if (typeof display.image_quota_remaining_percent === 'number') {
    return Math.max(100 - display.image_quota_remaining_percent, 0)
  }
  return 0
}

function getChatGPTWebQuotaRemainingPercent(key: EndpointAPIKey): number {
  const display = getChatGPTWebQuotaDisplay(key)
  if (!display) return 0
  if (typeof display.image_quota_remaining_percent === 'number') return display.image_quota_remaining_percent
  if (typeof display.image_quota_used_percent === 'number') {
    return Math.max(100 - display.image_quota_used_percent, 0)
  }
  return 0
}

function formatChatGPTWebUsage(value: number | null | undefined): string {
  if (value === undefined || value === null) return '-'
  if (Math.abs(value - Math.round(value)) < 1e-6) {
    return String(Math.round(value))
  }
  return value.toFixed(1)
}

function isKiroBannedKey(key: EndpointAPIKey): boolean {
  const quota = getQuotaSnapshotForProvider(key, 'kiro')
  return String(quota?.code || '').trim().toLowerCase() === 'banned'
}

// 格式化封禁/禁止时间（后端返回秒级时间戳，Kiro/Antigravity 通用）
function formatBanTimestamp(timestamp: number | undefined): string {
  if (!timestamp) return ''
  const date = new Date(timestamp * 1000)
  return date.toLocaleString('zh-CN', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function isAntigravityForbiddenKey(key: EndpointAPIKey): boolean {
  const quota = getQuotaSnapshotForProvider(key, 'antigravity')
  return String(quota?.code || '').trim().toLowerCase() === 'forbidden'
}

function getAntigravityForbiddenReason(key: EndpointAPIKey): string | undefined {
  const quota = getQuotaSnapshotForProvider(key, 'antigravity')
  return quota?.reason || undefined
}

function getAntigravityForbiddenAt(key: EndpointAPIKey): number | undefined {
  return getQuotaSnapshotUpdatedAt(getQuotaSnapshotForProvider(key, 'antigravity'))
}

function getAntigravityQuotaUpdatedAt(key: EndpointAPIKey): number | undefined {
  return getQuotaSnapshotUpdatedAt(getQuotaSnapshotForProvider(key, 'antigravity'))
}

// 格式化 Kiro 更新时间
const formatKiroUpdatedAt = formatUpdatedAt

// 格式化 Kiro 使用量（带单位）
function formatKiroUsage(value: number | undefined): string {
  if (value === undefined || value === null) return '-'
  const normalized = Number(value)
  if (!Number.isFinite(normalized)) return '-'
  if (normalized >= 1000) return formatCompactNumber(normalized, { fractionDigits: 1 })
  return normalized.toFixed(1)
}

// 格式化 Kiro 重置时间
function formatKiroResetTime(timestamp: number | undefined): string {
  if (!timestamp) return ''
  // timestamp 可能是毫秒或秒，需要判断
  const ts = timestamp > 1e12 ? timestamp : timestamp * 1000
  const now = Date.now()
  const diff = ts - now

  if (diff <= 0) {
    return legacyT('已重置')
  }

  const days = Math.floor(diff / (1000 * 60 * 60 * 24))
  const hours = Math.floor((diff % (1000 * 60 * 60 * 24)) / (1000 * 60 * 60))

  if (days > 0) {
    return locale.value === 'en-US' ? `${days}d ${hours}h later` : `${days}天${hours}小时后`
  }

  const minutes = Math.floor((diff % (1000 * 60 * 60)) / (1000 * 60))
  if (hours > 0) {
    return locale.value === 'en-US' ? `${hours}h ${minutes}m later` : `${hours}小时${minutes}分钟后`
  }

  return locale.value === 'en-US' ? `${minutes}m later` : `${minutes}分钟后`
}

// 格式化 Kiro 订阅类型显示
function formatKiroSubscription(title: string | undefined): string {
  if (!title) return ''
  // 简化显示：KIRO PRO+ -> Pro+, KIRO FREE -> Free（首字母大写，与 Codex 保持一致）
  const upper = title.toUpperCase()
  if (upper.includes('POWER')) return 'Power'
  if (upper.includes('PRO+')) return 'Pro+'
  if (upper.includes('PRO')) return 'Pro'
  if (upper.includes('FREE')) return 'Free'
  return title
}

function getKiroSubscriptionTitle(key: EndpointAPIKey): string | undefined {
  return getKiroQuotaDisplay(key)?.subscription_title
}

function getKiroSubscriptionBadgeLabel(key: EndpointAPIKey): string {
  return formatKiroSubscription(getKiroSubscriptionTitle(key))
}

function shouldShowKiroSubscriptionBadge(key: EndpointAPIKey): boolean {
  if (provider.value?.provider_type !== 'kiro') return false

  const kiroLabel = getKiroSubscriptionBadgeLabel(key)
  if (!kiroLabel) return false

  const oauthPlanLabel = formatOAuthPlanType(key.oauth_plan_type)
  if (!oauthPlanLabel) return true

  return oauthPlanLabel.trim().toLowerCase() !== kiroLabel.trim().toLowerCase()
}

function shouldAutoRefreshCodexQuota(): boolean {
  if (provider.value?.provider_type !== 'codex') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    // reset-credit 独立于 Token 刷新；这里只按账号配额缓存决定是否后台更新
    if (!hasCodexQuotaDisplayData(key)) {
      return true
    }
    // 配额数据超过 5 分钟未更新，也触发刷新
    const updatedAt = getCodexQuotaDisplay(key)?.updated_at
    if (typeof updatedAt !== 'number' || (now - updatedAt) > AUTO_QUOTA_REFRESH_STALE_SECONDS) {
      return true
    }
  }

  return false
}

// 检查 OAuth Token 是否即将过期（Codex / Gemini CLI / Antigravity / Kiro / Windsurf / ChatGPT Web）
function isTokenExpiringSoon(key: EndpointAPIKey, now: number): boolean {
  const oauthCode = String(key.status_snapshot?.oauth?.code || '').trim().toLowerCase()
  if (oauthCode && oauthCode !== 'valid' && oauthCode !== 'expiring') {
    return false
  }
  return typeof key.oauth_expires_at === 'number'
    && (key.oauth_expires_at - now) <= AUTO_TOKEN_REFRESH_SKEW_SECONDS
}

function shouldAutoRefreshAntigravityQuota(): boolean {
  if (provider.value?.provider_type !== 'antigravity') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    // 只要有一个活跃 key 没有配额/为空/过期，就刷新一次（接口会批量刷新所有活跃 key）
    if (!hasAntigravityQuotaDisplayData(key)) {
      return true
    }
    const updatedAt = getAntigravityQuotaUpdatedAt(key)
    if (typeof updatedAt !== 'number' || (now - updatedAt) > AUTO_QUOTA_REFRESH_STALE_SECONDS) {
      return true
    }
  }

  return false
}

function shouldAutoRefreshGeminiCliQuota(): boolean {
  if (provider.value?.provider_type !== 'gemini_cli') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    if (!hasGeminiCliQuotaDisplayData(key)) {
      return true
    }

    const updatedAt = getGeminiCliQuotaUpdatedAt(key)
    if (typeof updatedAt !== 'number' || (now - updatedAt) > AUTO_QUOTA_REFRESH_STALE_SECONDS) {
      return true
    }
  }

  return false
}

function shouldAutoRefreshKiroQuota(): boolean {
  if (provider.value?.provider_type !== 'kiro') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    // 只要有一个活跃 key 没有配额数据，就刷新一次
    if (!hasKiroQuotaDisplayData(key)) {
      return true
    }
    // 配额数据超过 5 分钟未更新，也触发刷新
    const updatedAt = getKiroQuotaDisplay(key)?.updated_at
    if (typeof updatedAt !== 'number' || (now - updatedAt) > AUTO_QUOTA_REFRESH_STALE_SECONDS) {
      return true
    }
  }

  return false
}

function shouldAutoRefreshGrokQuota(): boolean {
  if (provider.value?.provider_type !== 'grok') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    if (!hasGrokQuotaDisplayData(key)) {
      return true
    }

    const updatedAt = getGrokQuotaDisplay(key)?.updated_at
    if (typeof updatedAt !== 'number' || (now - updatedAt) > AUTO_QUOTA_REFRESH_STALE_SECONDS) {
      return true
    }
  }

  return false
}

function shouldAutoRefreshWindsurfQuota(): boolean {
  if (provider.value?.provider_type !== 'windsurf') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    if (!hasWindsurfQuotaDisplayData(key)) {
      return true
    }

    const updatedAt = getWindsurfQuotaDisplay(key)?.updated_at
    if (typeof updatedAt !== 'number' || (now - updatedAt) > AUTO_QUOTA_REFRESH_STALE_SECONDS) {
      return true
    }
  }

  return false
}

function shouldAutoRefreshChatGPTWebQuota(): boolean {
  if (provider.value?.provider_type !== 'chatgpt_web') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    if (!hasChatGPTWebQuotaDisplayData(key)) {
      return true
    }

    const updatedAt = getChatGPTWebQuotaDisplay(key)?.updated_at
    if (typeof updatedAt !== 'number' || (now - updatedAt) > AUTO_QUOTA_REFRESH_STALE_SECONDS) {
      return true
    }
  }

  return false
}

function defaultQuotaSnapshot(): QuotaStatusSnapshot {
  return {
    code: 'unknown',
    exhausted: false,
    usage_ratio: null,
    updated_at: null,
    reset_seconds: null,
    plan_type: null,
  }
}

function wrapQuotaMetadataForProvider(
  providerType: string,
  metadata: Record<string, unknown> | undefined,
): UpstreamMetadata | null {
  if (!metadata) return null
  if (providerType in metadata) {
    return metadata as UpstreamMetadata
  }
  return { [providerType]: metadata } as UpstreamMetadata
}

// 将配额刷新结果就地应用到现有 key 上，避免重新拉列表导致分页重置
function applyQuotaResults(
  results: { key_id: string; status: string; metadata?: Record<string, unknown>; quota_snapshot?: QuotaStatusSnapshot }[],
): number {
  const providerType = provider.value?.provider_type
  if (!providerType) return 0

  let applied = 0
  for (const r of results) {
    const target = providerKeys.value.find(k => k.id === r.key_id)
    if (!target) continue

    let changed = false
    const wrappedMetadata = wrapQuotaMetadataForProvider(providerType, r.metadata)
    if (wrappedMetadata) {
      target.upstream_metadata = { ...target.upstream_metadata, ...wrappedMetadata } as typeof target.upstream_metadata
      changed = true
    }

    if (r.quota_snapshot) {
      target.status_snapshot = {
        oauth: target.status_snapshot?.oauth ?? {
          code: 'none',
          label: null,
          reason: null,
          expires_at: null,
          invalid_at: null,
          source: null,
          requires_reauth: false,
          expiring_soon: false,
        },
        account: target.status_snapshot?.account ?? {
          code: 'ok',
          label: null,
          reason: null,
          blocked: false,
          source: null,
          recoverable: false,
        },
        quota: {
          ...defaultQuotaSnapshot(),
          ...(target.status_snapshot?.quota ?? {}),
          ...r.quota_snapshot,
        },
      }
      changed = true
    }

    if (changed) {
      applied += 1
    }
  }
  return applied
}

// 通用的自动刷新配额函数（支持 Codex、Gemini CLI、Antigravity、Kiro、Windsurf 和 ChatGPT Web）
async function autoRefreshQuotaInBackground() {
  const providerId = props.providerId
  if (!providerId) return
  if (refreshingQuota.value) return

  const providerType = provider.value?.provider_type
  if (providerType !== 'codex' && providerType !== 'gemini_cli' && providerType !== 'antigravity' && providerType !== 'kiro' && providerType !== 'windsurf' && providerType !== 'chatgpt_web' && providerType !== 'grok') return

  // 检查是否需要刷新
  let shouldRefresh = false
  if (providerType === 'codex') {
    shouldRefresh = shouldAutoRefreshCodexQuota()
  } else if (providerType === 'gemini_cli') {
    shouldRefresh = shouldAutoRefreshGeminiCliQuota()
  } else if (providerType === 'antigravity') {
    shouldRefresh = shouldAutoRefreshAntigravityQuota()
  } else if (providerType === 'kiro') {
    shouldRefresh = shouldAutoRefreshKiroQuota()
  } else if (providerType === 'grok') {
    shouldRefresh = shouldAutoRefreshGrokQuota()
  } else if (providerType === 'windsurf') {
    shouldRefresh = shouldAutoRefreshWindsurfQuota()
  } else if (providerType === 'chatgpt_web') {
    shouldRefresh = shouldAutoRefreshChatGPTWebQuota()
  }
  if (!shouldRefresh) return

  let hadCachedQuota = false
  if (providerType === 'codex') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasCodexQuotaDisplayData(key))
  } else if (providerType === 'gemini_cli') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasGeminiCliQuotaDisplayData(key))
  } else if (providerType === 'antigravity') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasAntigravityQuotaDisplayData(key))
  } else if (providerType === 'kiro') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasKiroQuotaDisplayData(key))
  } else if (providerType === 'grok') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasGrokQuotaDisplayData(key))
  } else if (providerType === 'windsurf') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasWindsurfQuotaDisplayData(key))
  } else if (providerType === 'chatgpt_web') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasChatGPTWebQuotaDisplayData(key))
  }

  refreshingQuota.value = true
  try {
    const result = await refreshProviderQuota(providerId)
    const applied = applyQuotaResults(result.results)
    if (result.success <= 0 && applied === 0 && !hadCachedQuota && providerType === 'antigravity') {
      showError(legacyT('没有获取到配额信息（请检查账号是否已授权、project_id 是否存在）'), legacyT('提示'))
    }
  } catch (err: unknown) {
    if (!hadCachedQuota && providerType === 'antigravity') {
      showError(localizedApiError(err, '后台刷新配额失败'), legacyT('错误'))
    }
  } finally {
    refreshingQuota.value = false
  }
}

async function openAntigravityQuotaDialog(key: EndpointAPIKey) {
  antigravityQuotaDialogKey.value = key
  antigravityQuotaDialogOpen.value = true

  // 没有配额数据时主动获取
  if (!hasAntigravityQuotaDisplayData(key)) {
    if (refreshingQuota.value) return
    refreshingQuota.value = true
    try {
      const result = await refreshProviderQuota(props.providerId)
      applyQuotaResults(result.results)
      // 更新弹窗引用的 key 数据
      const updated = allKeys.value.find(({ key: k }) => k.id === key.id)
      if (updated) {
        antigravityQuotaDialogKey.value = updated.key
      }
    } catch {
      // 静默失败，弹窗会显示"暂无配额数据"
    } finally {
      refreshingQuota.value = false
    }
  }
}

async function handleKeyChanged() {
  await Promise.all([loadEndpoints(), loadMappingPreview()])
  emit('refresh')
  // 添加/修改 key 后自动获取已支持 provider 的配额（新 key 的 upstream_metadata 为空）
  void autoRefreshQuotaInBackground()
}

// 切换密钥启用状态
async function toggleKeyActive(key: EndpointAPIKey) {
  if (togglingKeyId.value) return

  togglingKeyId.value = key.id
  try {
    const newStatus = !key.is_active
    await updateProviderKey(key.id, { is_active: newStatus })
    key.is_active = newStatus
    showSuccess(legacyT(newStatus ? '密钥已启用' : '密钥已停用'))
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '操作失败'), legacyT('错误'))
  } finally {
    togglingKeyId.value = null
  }
}

// ===== Key 级别代理配置 =====

/** 获取 Key 当前代理节点的名称（用于显示） */
function getKeyProxyNodeName(key: EndpointAPIKey): string | null {
  if (!key.proxy?.node_id) return null
  const node = proxyNodesStore.nodes.find(n => n.id === key.proxy?.node_id)
  return node ? node.name : `${key.proxy.node_id.slice(0, 8)  }...`
}

/** 切换代理 Popover 的打开/关闭状态 */
function handleProxyPopoverToggle(keyId: string, open: boolean) {
  proxyPopoverOpenKeyId.value = open ? keyId : null
  if (open) {
    proxyNodesStore.ensureLoaded()
  }
}

/** 设置 Key 的代理节点 */
async function setKeyProxy(key: EndpointAPIKey, nodeId: string) {
  savingProxyKeyId.value = key.id
  try {
    await updateProviderKey(key.id, {
      proxy: { node_id: nodeId, enabled: true },
    })
    key.proxy = { node_id: nodeId, enabled: true }
    proxyPopoverOpenKeyId.value = null
    showSuccess(legacyT('代理节点已设置'))
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '设置代理失败'), legacyT('错误'))
  } finally {
    savingProxyKeyId.value = null
  }
}

/** 清除 Key 的代理节点（回退到 Provider 级别代理） */
async function clearKeyProxy(key: EndpointAPIKey) {
  savingProxyKeyId.value = key.id
  try {
    await updateProviderKey(key.id, { proxy: null })
    key.proxy = null
    proxyPopoverOpenKeyId.value = null
    showSuccess(legacyT('已清除账号代理，将使用提供商级别代理'))
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '清除代理失败'), legacyT('错误'))
  } finally {
    savingProxyKeyId.value = null
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

function handleBatchAssignDialogOpenUpdate(value: boolean) {
  batchAssignDialogOpen.value = value
}

// 处理批量关联完成
async function handleBatchAssignChanged() {
  await Promise.all([loadEndpoints(), loadMappingPreview()])
  emit('refresh')
}

// 处理模型映射变更
async function handleModelMappingChanged() {
  await Promise.all([loadEndpoints(), loadMappingPreview()])
  emit('refresh')
}

// 处理模型保存完成
async function handleModelSaved() {
  editingModel.value = null
  await Promise.all([loadEndpoints(), loadMappingPreview()])
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
    showSuccess(legacyT('优先级已更新'))
    // 更新本地数据 - 更新 providerKeys 中的数据
    const keyToUpdate = providerKeys.value.find(k => k.id === keyId)
    if (keyToUpdate) {
      keyToUpdate.internal_priority = newPriority
    }
    // 重新排序
    providerKeys.value.sort((a, b) => (a.internal_priority ?? 0) - (b.internal_priority ?? 0))
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '更新优先级失败'), legacyT('错误'))
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
}

function handleMultiplierKeydown(e: KeyboardEvent, key: EndpointAPIKey, format: string) {
  if (e.key === 'Enter') {
    e.preventDefault()
    e.stopPropagation()
    saveMultiplier(key, format)
  } else if (e.key === 'Escape') {
    e.preventDefault()
    multiplierSaving.value = true // 阻止 blur 触发保存
    cancelEditMultiplier()
  }
}

function handleMultiplierBlur(key: EndpointAPIKey, format: string) {
  if (multiplierSaving.value) return
  saveMultiplier(key, format)
}

async function saveMultiplier(key: EndpointAPIKey, format: string) {
  // 防止重复调用（Enter 触发后阻止 blur 再次进入）
  if (multiplierSaving.value) return
  multiplierSaving.value = true

  const keyId = editingMultiplierKey.value
  const newMultiplier = parseFloat(String(editingMultiplierValue.value))

  // 验证输入有效性
  if (!keyId || isNaN(newMultiplier)) {
    showError(legacyT('请输入有效的倍率值'))
    cancelEditMultiplier()
    multiplierSaving.value = false
    return
  }

  // 验证合理范围
  if (newMultiplier <= 0 || newMultiplier > 100) {
    showError(legacyT('倍率必须在 0.01 到 100 之间'))
    cancelEditMultiplier()
    multiplierSaving.value = false
    return
  }

  // 如果倍率没有变化,直接取消编辑（使用精度容差比较浮点数）
  const currentMultiplier = getKeyRateMultiplier(key, format)
  if (Math.abs(currentMultiplier - newMultiplier) < 0.0001) {
    cancelEditMultiplier()
    multiplierSaving.value = false
    return
  }

  cancelEditMultiplier()

  try {
    // 构建 rate_multipliers 对象
    const rateMultipliers = { ...(key.rate_multipliers || {}) }
    rateMultipliers[format] = newMultiplier

    await updateProviderKey(keyId, { rate_multipliers: rateMultipliers })
    showSuccess(legacyT('倍率已更新'))

    // 更新本地数据
    const keyToUpdate = providerKeys.value.find(k => k.id === keyId)
    if (keyToUpdate) {
      keyToUpdate.rate_multipliers = rateMultipliers
    }
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '更新倍率失败'), legacyT('错误'))
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
      const originalPriority = originalPriorityMap.get(key.id) ?? 0

      if (key === draggedKey) {
        // 被拖动的项单独成组
        newPriorityMap.set(key.id, currentPriority)
        currentPriority++
      } else {
        if (groupNewPriority.has(originalPriority)) {
          // 同组的其他项使用相同的新优先级
          newPriorityMap.set(key.id, groupNewPriority.get(originalPriority) ?? currentPriority)
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
    showSuccess(legacyT('优先级已更新'))
    await loadEndpoints()
    emit('refresh')
  } catch (err: unknown) {
    showError(localizedApiError(err, '更新优先级失败'), legacyT('错误'))
    await loadEndpoints()
  }
}

// 获取密钥的 API 格式列表（按指定顺序排序）
function getKeyApiFormats(key: EndpointAPIKey, endpoint?: ProviderEndpointWithKeys): string[] {
  const providerType = provider.value?.provider_type
  let formats: string[] = []

  if (
    providerType
    && isOAuthAccountProviderType(providerType)
    && isOAuthManagedCredential(key)
  ) {
    formats = [...availableKeyApiFormats.value]
  } else if (key.api_formats && key.api_formats.length > 0) {
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
    paid: 'Paid',
    team: 'Team',
    enterprise: 'Enterprise',
    ultra: 'Ultra',
    basic: 'Basic',
    super: 'Super',
    heavy: 'Heavy',
  }
  return labels[planType.toLowerCase()] || planType
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

// 判断是否为 Codex Team/Plus/Enterprise 账号（有 5H 限额，显示 3 列）
function isCodexTeamPlan(key: EndpointAPIKey): boolean {
  const planType = key.oauth_plan_type?.toLowerCase() || getCodexQuotaDisplay(key)?.plan_type?.toLowerCase()
  // Free 账号返回 false（2 列），其他所有账号返回 true（3 列）
  return planType !== undefined && planType !== 'free'
}

interface AntigravityQuotaItem {
  model: string
  label: string
  usedPercent: number
  remainingPercent: number
  resetSeconds: number | null
}

interface GeminiCliQuotaItem {
  model: string
  label: string
  usedPercent: number
  remainingPercent: number
  resetSeconds: number | null
}

function hasAntigravityQuotaData(metadata: UpstreamMetadata | null | undefined): boolean {
  const quotaByModel = metadata?.antigravity?.quota_by_model
  return !!quotaByModel && typeof quotaByModel === 'object' && Object.keys(quotaByModel).length > 0
}

function hasAntigravityQuotaDisplayData(key: EndpointAPIKey): boolean {
  const quota = getQuotaSnapshotForProvider(key, 'antigravity')
  if (Array.isArray(quota?.windows) && quota.windows.length > 0) {
    return true
  }
  return hasAntigravityQuotaData(key.upstream_metadata)
}

function getGeminiCliQuotaUpdatedAt(key: EndpointAPIKey): number | undefined {
  const quota = getQuotaSnapshotForProvider(key, 'gemini_cli')
  const quotaUpdatedAt = getQuotaSnapshotUpdatedAt(quota)
  if (typeof quotaUpdatedAt === 'number') return quotaUpdatedAt
  const updatedAt = Number(key.upstream_metadata?.gemini_cli?.updated_at ?? NaN)
  return Number.isFinite(updatedAt) && updatedAt > 0 ? updatedAt : undefined
}

function hasGeminiCliQuotaDisplayData(key: EndpointAPIKey): boolean {
  if (getGeminiCliAccountCreditsText(key, 'gemini_cli')) return true
  return getGeminiCliQuotaItems(key).length > 0
}

function getGeminiCliQuotaItems(key: EndpointAPIKey): GeminiCliQuotaItem[] {
  const quota = getQuotaSnapshotForProvider(key, 'gemini_cli')
  const windows = getQuotaWindowByScope(quota, 'model')
  if (!quota || windows.length === 0) return []

  const items = windows
    .map((window) => {
      const model = String(window.model || window.label || window.code || '').trim()
      if (!model) return null

      const usedPercent = getQuotaWindowUsedPercent(window)
      const remainingPercent = getQuotaWindowRemainingPercent(window)
      if (usedPercent === undefined && remainingPercent === undefined) {
        return null
      }

      const normalizedUsedPercent =
        usedPercent !== undefined
          ? usedPercent
          : Math.max(100 - (remainingPercent ?? 0), 0)
      const normalizedRemainingPercent =
        remainingPercent !== undefined
          ? remainingPercent
          : Math.max(100 - normalizedUsedPercent, 0)

      return {
        model,
        label: String(window.label || window.model || model),
        usedPercent: normalizedUsedPercent,
        remainingPercent: normalizedRemainingPercent,
        resetSeconds: getQuotaWindowLiveResetSeconds(quota, window),
      } satisfies GeminiCliQuotaItem
    })
    .filter((item): item is GeminiCliQuotaItem => item !== null)

  items.sort((a, b) => (b.usedPercent - a.usedPercent) || a.model.localeCompare(b.model))
  return items
}

function formatUpdatedAt(updatedAt: number): string {
  if (!updatedAt || typeof updatedAt !== 'number') return ''
  const now = Math.floor(Date.now() / 1000)
  const diff = now - updatedAt
  if (diff <= 60) return legacyT('刚刚更新')
  const minutes = Math.floor(diff / 60)
  if (minutes < 60) return locale.value === 'en-US' ? `${minutes}m ago` : `${minutes}分钟前更新`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return locale.value === 'en-US' ? `${hours}h ago` : `${hours}小时前更新`
  const days = Math.floor(hours / 24)
  return locale.value === 'en-US' ? `${days}d ago` : `${days}天前更新`
}

// 兼容旧函数名
const formatCodexUpdatedAt = formatUpdatedAt
const formatAntigravityUpdatedAt = formatUpdatedAt

function secondsUntilReset(resetTime: string): number | null {
  if (!resetTime) return null
  const ts = Date.parse(resetTime)
  if (Number.isNaN(ts)) return null
  const diff = Math.floor((ts - Date.now()) / 1000)
  return diff > 0 ? diff : 0
}

function secondsUntilUnixReset(resetAt: number | string | null | undefined): number | null {
  const numericResetAt = Number(resetAt)
  if (!Number.isFinite(numericResetAt) || numericResetAt <= 0) return null
  const now = Math.floor(Date.now() / 1000)
  return Math.max(Math.floor(numericResetAt - now), 0)
}

function coerceAntigravityPercent(value: number | string | null | undefined): number | undefined {
  const numericValue = Number(value)
  if (!Number.isFinite(numericValue)) return undefined
  return Math.min(Math.max(numericValue, 0), 100)
}

function coerceAntigravityRemainingFraction(value: number | string | null | undefined): number | undefined {
  const numericValue = Number(value)
  if (!Number.isFinite(numericValue)) return undefined
  return Math.min(Math.max(numericValue, 0), 1)
}

function getAntigravityQuotaItems(metadata: UpstreamMetadata | null | undefined): AntigravityQuotaItem[] {
  const quotaByModel = metadata?.antigravity?.quota_by_model
  if (!quotaByModel || typeof quotaByModel !== 'object') return []

  const items: AntigravityQuotaItem[] = []
  const opaqueDisplayIndex = { value: 1 }
  for (const [model, rawInfo] of Object.entries(quotaByModel)) {
    if (!model) continue
    const info: Partial<AntigravityModelQuota> = rawInfo || {}

    let usedPercent = coerceAntigravityPercent(info.used_percent)
    if (usedPercent === undefined) {
      const remainingFraction = coerceAntigravityRemainingFraction(info.remaining_fraction)
      if (remainingFraction !== undefined) {
        usedPercent = (1 - remainingFraction) * 100
      } else {
        continue
      }
    }

    usedPercent = coerceAntigravityPercent(usedPercent) ?? 0

    const remainingPercent = Math.max(100 - usedPercent, 0)

    let resetSeconds = secondsUntilUnixReset(info.reset_at)
    if (typeof info.reset_time === 'string' && info.reset_time.trim()) {
      resetSeconds = secondsUntilReset(info.reset_time.trim()) ?? resetSeconds
    }

    items.push({
      model,
      label: resolveAntigravityQuotaLabel(model, info.display_name, opaqueDisplayIndex),
      usedPercent,
      remainingPercent,
      resetSeconds,
    })
  }

  items.sort(compareAntigravityQuotaItems)
  return dedupeAntigravityQuotaItemsByLabel(items)
}

function getAntigravityQuotaItemsFromSnapshot(key: EndpointAPIKey): AntigravityQuotaItem[] {
  const quota = getQuotaSnapshotForProvider(key, 'antigravity')
  const windows = getQuotaWindowByScope(quota, 'model')
  if (!quota || windows.length === 0) return []
  const opaqueDisplayIndex = { value: 1 }

  const items = windows
    .map((window) => {
      const model = String(window.model || window.label || window.code || '').trim()
      if (!model) return null

      const usedPercent = getQuotaWindowUsedPercent(window)
      const remainingPercent = getQuotaWindowRemainingPercent(window)
      if (usedPercent === undefined && remainingPercent === undefined) {
        return null
      }

      const normalizedUsedPercent =
        usedPercent !== undefined
          ? usedPercent
          : Math.max(100 - (remainingPercent ?? 0), 0)
      const normalizedRemainingPercent =
        remainingPercent !== undefined
          ? remainingPercent
          : Math.max(100 - normalizedUsedPercent, 0)

      return {
        model,
        label: resolveAntigravityQuotaLabel(
          model,
          window.label || window.model,
          opaqueDisplayIndex,
        ),
        usedPercent: normalizedUsedPercent,
        remainingPercent: normalizedRemainingPercent,
        resetSeconds: getQuotaWindowLiveResetSeconds(quota, window),
      } satisfies AntigravityQuotaItem
    })
    .filter((item): item is AntigravityQuotaItem => item !== null)

  items.sort(compareAntigravityQuotaItems)
  return dedupeAntigravityQuotaItemsByLabel(items)
}

const ANTIGRAVITY_QUOTA_PREVIEW_LIMIT = 6

function getAntigravityQuotaItemsForKey(key: EndpointAPIKey): AntigravityQuotaItem[] {
  const snapshotItems = getAntigravityQuotaItemsFromSnapshot(key)
  if (snapshotItems.length > 0) return snapshotItems
  return getAntigravityQuotaItems(key.upstream_metadata)
}

function getAntigravityQuotaPreviewForKey(key: EndpointAPIKey): AntigravityQuotaItem[] {
  return getAntigravityQuotaItemsForKey(key).slice(0, ANTIGRAVITY_QUOTA_PREVIEW_LIMIT)
}

function getAntigravityQuotaHiddenCountForKey(key: EndpointAPIKey): number {
  return Math.max(getAntigravityQuotaItemsForKey(key).length - ANTIGRAVITY_QUOTA_PREVIEW_LIMIT, 0)
}

function getResetCountdownText(
  resetAt: number | null | undefined,
  resetSecs: number | null | undefined,
  updatedAt: number | null | undefined,
  usedPercent: number | null | undefined
): string {
  const status = getCodexResetCountdown(
    resetAt,
    resetSecs,
    updatedAt,
    countdownTick.value,
    toCodexRemainingPercent(usedPercent)
  )
  if (!status) return ''
  return status.isExpired ? legacyT(status.text) : `${legacyT(status.text)} ${legacyT('后重置')}`
}

function getCodexQuotaResetText(
  resetAt: number | null | undefined,
  resetSecs: number | null | undefined,
  updatedAt: number | null | undefined,
  usedPercent: number | null | undefined
): string {
  if (resetAt || resetSecs) {
    return getResetCountdownText(resetAt, resetSecs, updatedAt, usedPercent)
  }
  return legacyT('已重置')
}

function getResetCountdownClass(
  resetAt: number | null | undefined,
  resetSecs: number | null | undefined,
  updatedAt: number | null | undefined,
  usedPercent: number | null | undefined
): string {
  const status = getCodexResetCountdown(
    resetAt,
    resetSecs,
    updatedAt,
    countdownTick.value,
    toCodexRemainingPercent(usedPercent)
  )
  if (!status || status.isExpired) return 'text-muted-foreground/70'
  if (status.isCritical) return 'text-destructive font-medium animate-pulse'
  if (status.isUrgent) return 'text-amber-500 dark:text-amber-400'
  return 'text-muted-foreground/70'
}

function toCodexRemainingPercent(usedPercent: number | null | undefined): number | null {
  const normalizedUsed = Number(usedPercent)
  if (!Number.isFinite(normalizedUsed)) return null
  const clampedUsed = Math.min(Math.max(normalizedUsed, 0), 100)
  return Math.max(100 - clampedUsed, 0)
}

function shouldStartCodexResetCountdown(usedPercent: number | null | undefined): boolean {
  const remainingPercent = toCodexRemainingPercent(usedPercent)
  if (remainingPercent == null) return true
  return remainingPercent < 100
}

// 格式化重置时间
function formatResetTime(seconds: number): string {
  const days = Math.floor(seconds / 86400)
  const hours = Math.floor((seconds % 86400) / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)

  if (days > 0) {
    return locale.value === 'en-US' ? `${days}d ${hours}h` : `${days}天 ${hours}小时`
  }
  if (hours > 0) {
    return locale.value === 'en-US' ? `${hours}h ${minutes}m` : `${hours}小时 ${minutes}分钟`
  }
  return locale.value === 'en-US' ? `${minutes}m` : `${minutes}分钟`
}

// OAuth 订阅类型样式
function getOAuthPlanTypeClass(planType: string): string {
  const classes: Record<string, string> = {
    plus: 'border-green-500/50 text-green-600 dark:text-green-400',
    pro: 'border-blue-500/50 text-blue-600 dark:text-blue-400',
    free: 'border-primary/50 text-primary',
    paid: 'border-blue-500/50 text-blue-600 dark:text-blue-400',
    team: 'border-purple-500/50 text-purple-600 dark:text-purple-400',
    enterprise: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
    ultra: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
    'pro+': 'border-purple-500/50 text-purple-600 dark:text-purple-400',
    power: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
    basic: 'border-primary/50 text-primary',
    super: 'border-green-500/50 text-green-600 dark:text-green-400',
    heavy: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
  }
  return classes[planType.toLowerCase()] || ''
}

// OAuth 状态信息（包括失效和过期）
function getKeyOAuthExpires(key: EndpointAPIKey) {
  return getOAuthStatusDisplayWithFallback(key, countdownTick.value)
}

function getOAuthRefreshButtonTitle(key: EndpointAPIKey): string {
  return resolveOAuthRefreshButtonTitle(key, countdownTick.value)
}

// OAuth 状态的 title 提示
function getOAuthStatusTitle(key: EndpointAPIKey): string {
  const accountTitle = getAccountStatusTitle(key)
  if (accountTitle && isAccountLevelBlock(key)) {
    return accountTitle
  }
  return resolveOAuthStatusTitle(key, countdownTick.value)
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

function isKeyRecoverable(key: EndpointAPIKey): boolean {
  return Boolean(
    key.circuit_breaker_open
    || (key.health_score !== undefined && key.health_score < 0.5)
  )
}

function getOpenCircuitEntries(key: EndpointAPIKey): Array<[string, NonNullable<EndpointAPIKey['circuit_breaker_by_format']>[string]]> {
  return Object.entries(key.circuit_breaker_by_format || {})
    .filter(([, value]) => value?.open === true)
}

function getKeyCircuitProbeCountdown(key: EndpointAPIKey): string {
  void countdownTick.value
  const nextProbe = getOpenCircuitEntries(key)
    .map(([, value]) => {
      if (typeof value.next_probe_at_unix_secs === 'number' && Number.isFinite(value.next_probe_at_unix_secs)) {
        return value.next_probe_at_unix_secs * 1000
      }
      if (value.next_probe_at) {
        const ms = new Date(value.next_probe_at).getTime()
        return Number.isFinite(ms) ? ms : null
      }
      return null
    })
    .filter((value): value is number => value !== null)
    .sort((a, b) => a - b)[0]
  if (!nextProbe) {
    return ''
  }
  const diffMs = nextProbe - Date.now()
  return diffMs > 0 ? ` ${formatCountdown(diffMs)}` : ` ${legacyT('探测中')}`
}

function getKeyCircuitBreakerTitle(key: EndpointAPIKey): string {
  const entries = getOpenCircuitEntries(key)
  if (entries.length === 0) return legacyT('熔断器已打开')
  const parts = entries.map(([format, value]) => {
    const label = formatApiFormatShort(format)
    const reason = value.reason ? `${legacyT('原因')}: ${value.reason}` : `${legacyT('原因')}: ${legacyT('连续失败')}`
    const interval = typeof value.probe_interval_minutes === 'number'
      ? `${legacyT('探测间隔')}: ${value.probe_interval_minutes} ${legacyT('分钟')}`
      : ''
    const countdown = getFormatProbeCountdown(key, format).trim()
    return [label, reason, interval, countdown ? `${legacyT('状态')}: ${countdown}` : '']
      .filter(Boolean)
      .join(' / ')
  })
  parts.push(legacyT('点击恢复按钮可重置熔断器'))
  return parts.join('\n')
}

function getRecoverKeyTitle(key: EndpointAPIKey): string {
  if (key.circuit_breaker_open) {
    return legacyT('重置熔断器并恢复健康状态')
  }
  return legacyT('刷新健康状态')
}

// 获取自动获取模型状态的 title 提示
function getAutoFetchStatusTitle(key: EndpointAPIKey): string {
  const parts: string[] = [legacyT('自动获取模型已启用')]

  if (key.last_models_fetch_at) {
    const date = new Date(key.last_models_fetch_at)
    parts.push(`${legacyT('上次同步')}: ${date.toLocaleString(locale.value)}`)
  }

  if (key.last_models_fetch_error) {
    parts.push(`${legacyT('错误')}: ${key.last_models_fetch_error}`)
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
      return ` ${legacyT('探测中')}`
    }
  }
  // 等待探测
  if (formatData.next_probe_at_unix_secs || formatData.next_probe_at) {
    const nextProbeMs = typeof formatData.next_probe_at_unix_secs === 'number'
      ? formatData.next_probe_at_unix_secs * 1000
      : new Date(formatData.next_probe_at || '').getTime()
    const diffMs = nextProbeMs - Date.now()
    if (diffMs > 0) {
      return ` ${formatCountdown(diffMs)}`
    } else {
      return ` ${legacyT('探测中')}`
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
  const requestId = ++providerLoadRequestId
  const shouldShowSpinner = !provider.value || provider.value.id !== props.providerId

  try {
    if (shouldShowSpinner) {
      loading.value = true
    }
    // 系统级格式转换配置只影响一个图标状态，不应阻塞详情抽屉首屏。
    void loadSystemFormatConversionConfig()
    const providerData = await getProvider(props.providerId)
    if (requestId !== providerLoadRequestId) return
    provider.value = providerData
    keyPageSize.value = getProviderKeysPageSize(providerData.provider_type)

    if (!provider.value) {
      throw new Error(legacyT('Provider 不存在'))
    }
  } catch (err: unknown) {
    if (requestId !== providerLoadRequestId) return
    showError(localizedApiError(err, '加载失败'), legacyT('错误'))
  } finally {
    if (requestId === providerLoadRequestId && shouldShowSpinner) {
      loading.value = false
    }
  }
}

async function loadProviderKeysPage(page = currentKeyPage.value) {
  if (!props.providerId) return
  const providerId = props.providerId
  const requestId = ++keysLoadRequestId
  loadingProviderKeys.value = true

  try {
    const result = await getProviderKeysPage(providerId, {
      page,
      page_size: keyPageSize.value,
    })
    if (requestId !== keysLoadRequestId || props.providerId !== providerId) return

    const nextTotalPages = Math.max(1, Math.ceil(result.total / result.page_size))
    if (result.keys.length === 0 && result.total > 0 && result.page > nextTotalPages) {
      await loadProviderKeysPage(nextTotalPages)
      return
    }

    providerKeys.value = result.keys
    providerKeysTotal.value = result.total
    currentKeyPage.value = Math.min(result.page, nextTotalPages)
    keyPageSize.value = result.page_size
    syncCurrentSelections(endpoints.value, result.keys)
  } catch (err: unknown) {
    if (requestId !== keysLoadRequestId || props.providerId !== providerId) return
    providerKeys.value = []
    providerKeysTotal.value = 0
    syncCurrentSelections(endpoints.value, [])
    showError(localizedApiError(err, '加载密钥失败'), legacyT('错误'))
  } finally {
    if (requestId === keysLoadRequestId) {
      loadingProviderKeys.value = false
    }
  }
}

// 加载端点列表
async function loadEndpoints() {
  if (!props.providerId) return
  const providerId = props.providerId
  const requestId = ++endpointsLoadRequestId
  loadingProviderEndpoints.value = true
  loadingProviderKeys.value = true
  loadingProviderModels.value = true

  const sortEndpoints = (items: ProviderEndpoint[]): ProviderEndpointWithKeys[] => {
    return [...items].sort((a, b) => {
      const aIdx = API_FORMAT_ORDER.indexOf(a.api_format)
      const bIdx = API_FORMAT_ORDER.indexOf(b.api_format)
      if (aIdx === -1 && bIdx === -1) return 0
      if (aIdx === -1) return 1
      if (bIdx === -1) return -1
      return aIdx - bIdx
    })
  }

  const endpointsPromise = getProviderEndpoints(providerId)
    .then((endpointsList) => {
      if (requestId !== endpointsLoadRequestId) return
      const sortedEndpoints = sortEndpoints(endpointsList)
      endpoints.value = sortedEndpoints
      syncCurrentSelections(sortedEndpoints, providerKeys.value)
    })
    .catch((err: unknown) => {
      if (requestId !== endpointsLoadRequestId) return
      endpoints.value = []
      syncCurrentSelections([], providerKeys.value)
      showError(localizedApiError(err, '加载端点失败'), legacyT('错误'))
    })
    .finally(() => {
      if (requestId === endpointsLoadRequestId) {
        loadingProviderEndpoints.value = false
      }
    })

  const providerKeysPromise = loadProviderKeysPage(currentKeyPage.value)

  const modelsPromise = getProviderModels(providerId)
    .catch(() => [])
    .then((modelsResult) => {
      if (requestId !== endpointsLoadRequestId) return
      providerModels.value = modelsResult
    })
    .finally(() => {
      if (requestId === endpointsLoadRequestId) {
        loadingProviderModels.value = false
      }
    })

  await Promise.allSettled([endpointsPromise, providerKeysPromise, modelsPromise])
}

// 加载映射预览（独立于 loadEndpoints，不阻塞首屏渲染）
async function loadMappingPreview() {
  if (!props.providerId) return
  const requestId = ++mappingPreviewLoadRequestId
  loadingProviderMappingPreview.value = true
  try {
    const preview = await getProviderMappingPreview(props.providerId)
    if (requestId !== mappingPreviewLoadRequestId) return
    providerMappingPreview.value = preview
  } catch {
    if (requestId !== mappingPreviewLoadRequestId) return
    providerMappingPreview.value = null
  } finally {
    if (requestId === mappingPreviewLoadRequestId) {
      loadingProviderMappingPreview.value = false
    }
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

/* 轻量滚动条（用于 Antigravity 模型配额等小区域） */
.custom-scrollbar::-webkit-scrollbar {
  width: 4px;
}
.custom-scrollbar::-webkit-scrollbar-track {
  background: transparent;
}
.custom-scrollbar::-webkit-scrollbar-thumb {
  background-color: hsl(var(--muted-foreground) / 0.2);
  border-radius: 4px;
}
.custom-scrollbar::-webkit-scrollbar-thumb:hover {
  background-color: hsl(var(--muted-foreground) / 0.4);
}
</style>
