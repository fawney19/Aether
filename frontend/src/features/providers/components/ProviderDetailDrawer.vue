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
                  ref="keysListRef"
                  class="divide-y divide-border/40"
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
                        <div class="flex flex-col min-w-0">
                          <div class="flex items-center gap-1.5">
                            <span
                              class="text-sm font-medium truncate"
                              :class="key.name ? 'cursor-pointer hover:text-primary transition-colors' : ''"
                              :title="key.name ? '点击复制' : ''"
                              @click.stop="key.name && copyToClipboard(key.name)"
                            >{{ key.name || '未命名密钥' }}</span>
                            <!-- OAuth 订阅类型标签 (Codex) -->
                            <Badge
                              v-if="key.oauth_plan_type"
                              variant="outline"
                              class="text-[10px] px-1.5 py-0 shrink-0"
                              :class="getOAuthPlanTypeClass(key.oauth_plan_type)"
                            >
                              {{ formatOAuthPlanType(key.oauth_plan_type) }}
                            </Badge>
                            <!-- Kiro 订阅类型标签 -->
                            <Badge
                              v-if="provider.provider_type === 'kiro' && key.upstream_metadata?.kiro?.subscription_title"
                              variant="outline"
                              class="text-[10px] px-1.5 py-0 shrink-0"
                              :class="getOAuthPlanTypeClass(formatKiroSubscription(key.upstream_metadata?.kiro?.subscription_title))"
                            >
                              {{ formatKiroSubscription(key.upstream_metadata?.kiro?.subscription_title) }}
                            </Badge>
                          </div>
                          <div class="flex items-center gap-1">
                            <span class="text-[11px] font-mono text-muted-foreground">
                              {{ key.auth_type === 'oauth' ? '[Refresh Token]' : (key.auth_type === 'vertex_ai' ? 'Vertex AI' : key.api_key_masked) }}
                            </span>
                            <Button
                              v-if="key.auth_type === 'oauth'"
                              variant="ghost"
                              size="icon"
                              class="h-4 w-4 shrink-0"
                              title="下载 Refresh Token 授权文件"
                              @click.stop="downloadRefreshToken(key)"
                            >
                              <Download class="w-2.5 h-2.5" />
                            </Button>
                            <Button
                              v-else
                              variant="ghost"
                              size="icon"
                              class="h-4 w-4 shrink-0"
                              title="复制密钥"
                              @click.stop="copyFullKey(key)"
                            >
                              <Copy class="w-2.5 h-2.5" />
                            </Button>
                            <!-- OAuth 状态（失效/过期/倒计时）和刷新按钮 -->
                            <template v-if="getKeyOAuthExpires(key)">
                              <!-- 账号级别异常：醒目提示 + 清除按钮 -->
                              <template v-if="getKeyOAuthExpires(key)?.isInvalid && isAccountLevelBlock(key)">
                                <Badge
                                  variant="destructive"
                                  class="text-[10px] px-1.5 py-0 shrink-0 gap-0.5"
                                  :title="getOAuthStatusTitle(key)"
                                >
                                  <ShieldX class="w-2.5 h-2.5" />
                                  账号异常
                                </Badge>
                                <Button
                                  variant="ghost"
                                  size="icon"
                                  class="h-4 w-4 shrink-0 text-destructive hover:text-destructive"
                                  :disabled="clearingOAuthInvalidKeyId === key.id"
                                  title="清除异常标记（确认账号已完成验证后使用）"
                                  @click.stop="handleClearOAuthInvalid(key)"
                                >
                                  <RefreshCw
                                    class="w-2.5 h-2.5"
                                    :class="{ 'animate-spin': clearingOAuthInvalidKeyId === key.id }"
                                  />
                                </Button>
                              </template>
                              <!-- 普通 OAuth 状态 -->
                              <template v-else>
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
                            </template>
                            <!-- Antigravity 账号未激活提示 -->
                            <span
                              v-if="provider.provider_type === 'antigravity' && key.is_active && key.auth_type === 'oauth' && (!key.upstream_metadata || !hasAntigravityQuotaData(key.upstream_metadata))"
                              class="text-[10px] text-orange-500 dark:text-orange-400"
                              title="该账号尚未完成 Gemini Code Assist 激活，无法获取配额和使用模型"
                            >
                              账号未激活
                            </span>
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
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="模型权限"
                          @click="handleKeyPermissions(key)"
                        >
                          <Shield class="w-3.5 h-3.5" />
                        </Button>
                        <!-- 代理节点配置（仅非 custom 类型显示） -->
                        <Popover
                          v-if="provider.provider_type !== 'custom'"
                          :open="proxyPopoverOpenKeyId === key.id"
                          @update:open="(v: boolean) => handleProxyPopoverToggle(key.id, v)"
                        >
                          <PopoverTrigger as-child>
                            <Button
                              variant="ghost"
                              size="icon"
                              class="h-7 w-7"
                              :class="key.proxy?.node_id ? 'text-blue-500' : ''"
                              :disabled="savingProxyKeyId === key.id"
                              :title="key.proxy?.node_id ? `代理: ${getKeyProxyNodeName(key)}` : '设置代理节点'"
                              @click.stop
                            >
                              <Globe class="w-3.5 h-3.5" />
                            </Button>
                          </PopoverTrigger>
                          <PopoverContent
                            class="w-72 p-3"
                            side="bottom"
                            align="end"
                          >
                            <div class="space-y-2">
                              <div class="flex items-center justify-between">
                                <span class="text-xs font-medium">代理节点</span>
                                <Button
                                  v-if="key.proxy?.node_id"
                                  variant="ghost"
                                  size="sm"
                                  class="h-6 px-2 text-[10px] text-muted-foreground"
                                  :disabled="savingProxyKeyId === key.id"
                                  @click="clearKeyProxy(key)"
                                >
                                  清除
                                </Button>
                              </div>
                              <ProxyNodeSelect
                                :model-value="key.proxy?.node_id || ''"
                                trigger-class="h-8"
                                @update:model-value="(v: string) => setKeyProxy(key, v)"
                              />
                              <p class="text-[10px] text-muted-foreground">
                                {{ key.proxy?.node_id ? '当前使用独立代理' : '未设置，使用提供商级别代理' }}
                              </p>
                            </div>
                          </PopoverContent>
                        </Popover>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="编辑密钥"
                          @click="handleEditKey(endpoint, key)"
                        >
                          <Edit class="w-3.5 h-3.5" />
                        </Button>
                        <Button
                          v-if="provider.provider_type === 'antigravity'"
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          title="配额详情"
                          @click="openAntigravityQuotaDialog(key)"
                        >
                          <BarChart3 class="w-3.5 h-3.5" />
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
                      <div class="flex items-center justify-between mb-1">
                        <span class="text-[10px] text-muted-foreground">账号配额</span>
                        <div class="flex items-center gap-1">
                          <RefreshCw
                            v-if="refreshingQuota"
                            class="w-3 h-3 text-muted-foreground/70 animate-spin"
                          />
                          <span
                            v-if="key.upstream_metadata.codex?.updated_at"
                            class="text-[9px] text-muted-foreground/70"
                          >
                            {{ formatCodexUpdatedAt(key.upstream_metadata.codex.updated_at) }}
                          </span>
                        </div>
                      </div>
                      <!-- 限额并排显示：Team/Plus/Enterprise 账号 3列, Free 账号 2列 -->
                      <div
                        class="grid gap-3"
                        :class="isCodexTeamPlan(key) ? 'grid-cols-3' : 'grid-cols-2'"
                      >
                        <!-- 周限额 -->
                        <div v-if="key.upstream_metadata.codex?.primary_used_percent !== undefined">
                          <div class="flex items-center justify-between text-[10px] mb-0.5">
                            <span class="text-muted-foreground">周限额</span>
                            <span :class="getQuotaRemainingClass(key.upstream_metadata.codex.primary_used_percent)">
                              {{ (100 - key.upstream_metadata.codex.primary_used_percent).toFixed(1) }}%
                            </span>
                          </div>
                          <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                            <div
                              class="absolute left-0 top-0 h-full transition-all duration-300"
                              :class="getQuotaRemainingBarColor(key.upstream_metadata.codex.primary_used_percent)"
                              :style="{ width: `${Math.max(100 - key.upstream_metadata.codex.primary_used_percent, 0)}%` }"
                            />
                          </div>
                          <div
                            v-if="key.upstream_metadata.codex.primary_reset_seconds"
                            class="text-[9px] text-muted-foreground/70 mt-0.5"
                          >
                            {{ formatResetTime(key.upstream_metadata.codex.primary_reset_seconds) }}后重置
                          </div>
                        </div>
                        <!-- 5H限额（仅 Team/Plus/Enterprise 显示） -->
                        <div v-if="isCodexTeamPlan(key) && key.upstream_metadata.codex?.secondary_used_percent !== undefined">
                          <div class="flex items-center justify-between text-[10px] mb-0.5">
                            <span class="text-muted-foreground">5H限额</span>
                            <span :class="getQuotaRemainingClass(key.upstream_metadata.codex.secondary_used_percent)">
                              {{ (100 - key.upstream_metadata.codex.secondary_used_percent).toFixed(1) }}%
                            </span>
                          </div>
                          <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                            <div
                              class="absolute left-0 top-0 h-full transition-all duration-300"
                              :class="getQuotaRemainingBarColor(key.upstream_metadata.codex.secondary_used_percent)"
                              :style="{ width: `${Math.max(100 - key.upstream_metadata.codex.secondary_used_percent, 0)}%` }"
                            />
                          </div>
                          <div class="text-[9px] text-muted-foreground/70 mt-0.5">
                            <template v-if="key.upstream_metadata.codex.secondary_reset_seconds">
                              {{ formatResetTime(key.upstream_metadata.codex.secondary_reset_seconds) }}后重置
                            </template>
                            <template v-else>
                              已重置
                            </template>
                          </div>
                        </div>
                        <!-- 代码审查限额 -->
                        <div v-if="key.upstream_metadata.codex?.code_review_used_percent !== undefined">
                          <div class="flex items-center justify-between text-[10px] mb-0.5">
                            <span class="text-muted-foreground">审查限额</span>
                            <span :class="getQuotaRemainingClass(key.upstream_metadata.codex.code_review_used_percent)">
                              {{ (100 - key.upstream_metadata.codex.code_review_used_percent).toFixed(1) }}%
                            </span>
                          </div>
                          <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                            <div
                              class="absolute left-0 top-0 h-full transition-all duration-300"
                              :class="getQuotaRemainingBarColor(key.upstream_metadata.codex.code_review_used_percent)"
                              :style="{ width: `${Math.max(100 - key.upstream_metadata.codex.code_review_used_percent, 0)}%` }"
                            />
                          </div>
                          <div
                            v-if="key.upstream_metadata.codex.code_review_reset_seconds"
                            class="text-[9px] text-muted-foreground/70 mt-0.5"
                          >
                            {{ formatResetTime(key.upstream_metadata.codex.code_review_reset_seconds) }}后重置
                          </div>
                        </div>
                      </div>
                    </div>
                    <!-- Antigravity 上游额度摘要（按家族分组展示关键配额） -->
                    <div
                      v-if="provider.provider_type === 'antigravity' && key.upstream_metadata && (hasAntigravityQuotaData(key.upstream_metadata) || isAntigravityForbidden(key.upstream_metadata))"
                      class="mt-2 p-2 rounded-md"
                      :class="isAntigravityForbidden(key.upstream_metadata) ? 'bg-destructive/10 border border-destructive/30' : 'bg-muted/30'"
                    >
                      <!-- 封禁状态显示 -->
                      <div
                        v-if="isAntigravityForbidden(key.upstream_metadata)"
                        class="flex items-center gap-2 text-destructive"
                      >
                        <ShieldX class="w-4 h-4 shrink-0" />
                        <div class="flex-1 min-w-0">
                          <div class="text-[11px] font-medium">
                            账户访问被禁止
                          </div>
                          <div
                            v-if="key.upstream_metadata.antigravity?.forbidden_reason"
                            class="text-[10px] text-destructive/80 truncate"
                            :title="key.upstream_metadata.antigravity?.forbidden_reason"
                          >
                            {{ key.upstream_metadata.antigravity?.forbidden_reason }}
                          </div>
                        </div>
                        <span
                          v-if="key.upstream_metadata.antigravity?.forbidden_at"
                          class="text-[9px] text-destructive/60 shrink-0"
                        >
                          {{ formatBanTimestamp(key.upstream_metadata.antigravity?.forbidden_at) }}
                        </span>
                      </div>
                      <!-- 正常配额显示 -->
                      <template v-else>
                        <div class="flex items-center justify-between mb-1">
                          <span class="text-[10px] text-muted-foreground">模型配额</span>
                          <div class="flex items-center gap-1">
                            <RefreshCw
                              v-if="refreshingQuota"
                              class="w-3 h-3 text-muted-foreground/70 animate-spin"
                            />
                            <span
                              v-if="key.upstream_metadata.antigravity?.updated_at"
                              class="text-[9px] text-muted-foreground/70"
                            >
                              {{ formatAntigravityUpdatedAt(key.upstream_metadata.antigravity.updated_at) }}
                            </span>
                          </div>
                        </div>
                        <div class="grid grid-cols-2 gap-3">
                          <div
                            v-for="group in getAntigravityQuotaSummary(key.upstream_metadata)"
                            :key="group.key"
                          >
                            <div class="flex items-center justify-between text-[10px] mb-0.5">
                              <span class="text-muted-foreground truncate mr-2 min-w-0 flex-1">
                                {{ group.label }}
                              </span>
                              <span :class="getQuotaRemainingClass(group.usedPercent)">
                                {{ group.remainingPercent.toFixed(1) }}%
                              </span>
                            </div>
                            <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                              <div
                                class="absolute left-0 top-0 h-full transition-all duration-300"
                                :class="getQuotaRemainingBarColor(group.usedPercent)"
                                :style="{ width: `${Math.max(group.remainingPercent, 0)}%` }"
                              />
                            </div>
                            <div
                              v-if="group.resetSeconds !== null || group.usedPercent > 0"
                              class="text-[9px] text-muted-foreground/70 mt-0.5"
                            >
                              <template v-if="group.resetSeconds !== null && group.resetSeconds > 0">
                                {{ formatResetTime(group.resetSeconds) }}后重置
                              </template>
                              <template v-else-if="group.resetSeconds !== null && group.resetSeconds <= 0">
                                已重置
                              </template>
                              <template v-else>
                                重置时间未知
                              </template>
                            </div>
                          </div>
                        </div>
                      </template>
                    </div>
                    <!-- Kiro 上游额度信息（仅当有元数据时显示） -->
                    <div
                      v-if="provider.provider_type === 'kiro' && key.upstream_metadata && (hasKiroQuotaData(key.upstream_metadata) || isKiroBanned(key.upstream_metadata))"
                      class="mt-2 p-2 rounded-md"
                      :class="isKiroBanned(key.upstream_metadata) ? 'bg-destructive/10 border border-destructive/30' : 'bg-muted/30'"
                    >
                      <!-- 封禁状态显示 -->
                      <div
                        v-if="isKiroBanned(key.upstream_metadata)"
                        class="flex items-center gap-2 text-destructive"
                      >
                        <ShieldX class="w-4 h-4 shrink-0" />
                        <div class="flex-1 min-w-0">
                          <div class="text-[11px] font-medium">
                            账户已封禁
                          </div>
                          <div
                            v-if="key.upstream_metadata.kiro?.ban_reason"
                            class="text-[10px] text-destructive/80 truncate"
                            :title="key.upstream_metadata.kiro?.ban_reason"
                          >
                            {{ key.upstream_metadata.kiro?.ban_reason }}
                          </div>
                        </div>
                        <span
                          v-if="key.upstream_metadata.kiro?.banned_at"
                          class="text-[9px] text-destructive/60 shrink-0"
                        >
                          {{ formatBanTimestamp(key.upstream_metadata.kiro?.banned_at) }}
                        </span>
                      </div>
                      <!-- 正常配额显示 -->
                      <template v-else>
                        <div class="flex items-center justify-between mb-1">
                          <span class="text-[10px] text-muted-foreground">账号配额</span>
                          <div class="flex items-center gap-1">
                            <RefreshCw
                              v-if="refreshingQuota"
                              class="w-3 h-3 text-muted-foreground/70 animate-spin"
                            />
                            <span
                              v-if="key.upstream_metadata.kiro?.updated_at"
                              class="text-[9px] text-muted-foreground/70"
                            >
                              {{ formatKiroUpdatedAt(key.upstream_metadata.kiro?.updated_at) }}
                            </span>
                          </div>
                        </div>
                        <!-- Kiro 额度显示：使用进度 -->
                        <div>
                          <!-- 使用额度进度条 -->
                          <div>
                            <div class="flex items-center justify-between text-[10px] mb-0.5">
                              <span class="text-muted-foreground">使用额度</span>
                              <span :class="getQuotaRemainingClass(key.upstream_metadata.kiro?.usage_percentage || 0)">
                                {{ (100 - (key.upstream_metadata.kiro?.usage_percentage || 0)).toFixed(1) }}%
                              </span>
                            </div>
                            <div class="relative w-full h-1.5 bg-border rounded-full overflow-hidden">
                              <div
                                class="absolute left-0 top-0 h-full transition-all duration-300"
                                :class="getQuotaRemainingBarColor(key.upstream_metadata.kiro?.usage_percentage || 0)"
                                :style="{ width: `${Math.max(100 - (key.upstream_metadata.kiro?.usage_percentage || 0), 0)}%` }"
                              />
                            </div>
                            <div class="flex items-center justify-between text-[9px] text-muted-foreground/70 mt-0.5">
                              <span>
                                {{ formatKiroUsage(key.upstream_metadata.kiro?.current_usage) }} /
                                {{ formatKiroUsage(key.upstream_metadata.kiro?.usage_limit) }}
                              </span>
                              <span v-if="key.upstream_metadata.kiro?.next_reset_at">
                                {{ formatKiroResetTime(key.upstream_metadata.kiro?.next_reset_at) }}重置
                              </span>
                            </div>
                          </div>
                        </div>
                      </template>
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
                  <!-- 分页控制 -->
                  <div
                    v-if="shouldPaginateKeys"
                    class="px-4 py-2 flex items-center justify-between text-xs text-muted-foreground"
                  >
                    <span>共 {{ allKeys.length }} 个{{ provider.provider_type === 'custom' ? '密钥' : '账号' }}</span>
                    <div class="flex items-center gap-1.5">
                      <Button
                        variant="ghost"
                        size="sm"
                        class="h-6 px-2 text-xs"
                        :disabled="currentKeyPage <= 1"
                        @click="currentKeyPage--"
                      >
                        ‹
                      </Button>
                      <span class="tabular-nums">{{ currentKeyPage }} / {{ totalKeyPages }}</span>
                      <Button
                        variant="ghost"
                        size="sm"
                        class="h-6 px-2 text-xs"
                        :disabled="currentKeyPage >= totalKeyPages"
                        @click="currentKeyPage++"
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

  <!-- Antigravity 配额详情弹窗 -->
  <AntigravityQuotaDialog
    v-if="antigravityQuotaDialogKey"
    :open="antigravityQuotaDialogOpen"
    :metadata="antigravityQuotaDialogKey.upstream_metadata"
    :key-name="antigravityQuotaDialogKey.name || '未命名密钥'"
    :provider-id="providerId"
    :key-id="antigravityQuotaDialogKey.id"
    @update:open="antigravityQuotaDialogOpen = $event"
  />
</template>

<script setup lang="ts">
import { ref, watch, computed, nextTick } from 'vue'
import { useSmartPagination } from '@/composables/useSmartPagination'
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
  Download,
  Shield,
  Shuffle,
  ExternalLink,
  BarChart3,
  ShieldX,
  Globe,
} from 'lucide-vue-next'
import { useEscapeKey } from '@/composables/useEscapeKey'
import Button from '@/components/ui/button.vue'
import Badge from '@/components/ui/badge.vue'
import Card from '@/components/ui/card.vue'
import { Popover, PopoverTrigger, PopoverContent } from '@/components/ui'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useClipboard } from '@/composables/useClipboard'
import { useCountdownTimer, formatCountdown, getOAuthExpiresCountdown } from '@/composables/useCountdownTimer'
import { getProvider, getProviderEndpoints, updateProvider } from '@/api/endpoints'
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
import ProxyNodeSelect from '@/features/providers/components/ProxyNodeSelect.vue'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import {
  deleteEndpointKey,
  recoverKeyHealth,
  getProviderKeys,
  updateProviderKey,
  revealEndpointKey,
  exportKey,
  refreshProviderOAuth,
  refreshProviderQuota,
  clearOAuthInvalid,
  type ProviderEndpoint,
  type EndpointAPIKey,
  type Model,
  API_FORMAT_LABELS,
  API_FORMAT_ORDER,
  API_FORMAT_SHORT,
  sortApiFormats,
} from '@/api/endpoints'
import type { UpstreamMetadata, AntigravityModelQuota } from '@/api/endpoints/types'

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
const { confirm } = useConfirm()
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

// OAuth 失效清除状态
const clearingOAuthInvalidKeyId = ref<string | null>(null)

// 限额刷新状态（Codex / Antigravity）
const refreshingQuota = ref(false)

// Antigravity 配额详情弹窗状态
const antigravityQuotaDialogOpen = ref(false)
const antigravityQuotaDialogKey = ref<EndpointAPIKey | null>(null)

// Key 级别代理配置状态
const proxyNodesStore = useProxyNodesStore()
const savingProxyKeyId = ref<string | null>(null)
const proxyPopoverOpenKeyId = ref<string | null>(null)

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
  oauthKeyEditDialogOpen.value ||
  deleteKeyConfirmOpen.value ||
  modelFormDialogOpen.value ||
  batchAssignDialogOpen.value ||
  antigravityQuotaDialogOpen.value ||
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

// ===== 账号列表智能分页 =====
const keysListRef = ref<HTMLElement | null>(null)
const {
  currentPage: currentKeyPage,
  totalPages: totalKeyPages,
  shouldPaginate: shouldPaginateKeys,
  paginatedItems: paginatedKeys,
  getGlobalIndex: getGlobalKeyIndex,
  reset: resetKeysPagination,
} = useSmartPagination(allKeys, keysListRef)

// 合并监听 providerId 和 open，避免同一 tick 内两个 watcher 都触发导致重复请求
watch(
  [() => props.providerId, () => props.open],
  async ([newId, newOpen], [_oldId, oldOpen]) => {
    if (newOpen && newId) {
      await Promise.all([
        loadProvider(),
        loadEndpoints(),
      ])
      // 仅在抽屉刚打开时启动倒计时
      if (newOpen && !oldOpen) {
        startCountdownTimer()
      }
      void autoRefreshQuotaInBackground()
    } else if (!newOpen && oldOpen) {
      // 停止倒计时定时器
      stopCountdownTimer()
      // 重置所有状态
      provider.value = null
      endpoints.value = []
      providerKeys.value = []  // 清空 Provider 级别的 keys

      // 重置分页状态
      resetKeysPagination()

      // 重置所有对话框状态
      endpointDialogOpen.value = false
      keyFormDialogOpen.value = false
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
  // OAuth 密钥使用专门的编辑对话框
  if (key.auth_type === 'oauth') {
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
  } catch (err: any) {
    showError(err.response?.data?.detail || '导出失败', '错误')
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
    // 重新加载 key 数据（token 刷新可能补上了 project_id 等信息）
    await loadEndpoints()
    // Antigravity：token 刷新后可能完成了账号激活，触发配额获取
    // （不 emit('refresh')，避免触发全局 provider 余额刷新）
    void autoRefreshQuotaInBackground()
  } catch (err: any) {
    showError(err.response?.data?.detail || 'Token 刷新失败', '错误')
  } finally {
    refreshingOAuthKeyId.value = null
  }
}

// 判断是否为账号级别的封禁（刷新 token 无法修复）
function isAccountLevelBlock(key: EndpointAPIKey): boolean {
  if (!key.oauth_invalid_reason) return false
  return key.oauth_invalid_reason.startsWith('[ACCOUNT_BLOCK]')
}

// 清除 OAuth 失效标记
async function handleClearOAuthInvalid(key: EndpointAPIKey) {
  if (clearingOAuthInvalidKeyId.value) return

  const confirmed = await confirm({
    title: '清除账号异常标记',
    message: `确认账号 "${key.name || key.id.slice(0, 8)}" 已手动完成验证？清除后该 Key 将恢复正常调度。`,
    confirmText: '确认清除',
    variant: 'default',
  })
  if (!confirmed) return

  clearingOAuthInvalidKeyId.value = key.id
  try {
    await clearOAuthInvalid(key.id)
    showSuccess('已清除 OAuth 异常标记，Key 已自动启用')
    // 更新本地数据
    const keyInList = providerKeys.value.find(k => k.id === key.id)
    if (keyInList) {
      keyInList.oauth_invalid_at = null
      keyInList.oauth_invalid_reason = null
      keyInList.is_active = true
    }
    await loadEndpoints()
  } catch (err: any) {
    showError(err.response?.data?.detail || '清除失败', '错误')
  } finally {
    clearingOAuthInvalidKeyId.value = null
  }
}

// Codex / Antigravity / Kiro：打开抽屉后自动后台刷新（配额缓存缺失/过期，或 Token 即将过期时触发）
const AUTO_QUOTA_REFRESH_STALE_SECONDS = 5 * 60
// 与后端 OAuth 懒刷新阈值对齐：到期前 2 分钟内视为需要刷新
const AUTO_TOKEN_REFRESH_SKEW_SECONDS = 2 * 60

// 检查 Codex 是否有配额数据
function hasCodexQuotaData(meta: UpstreamMetadata | null | undefined): boolean {
  if (!meta?.codex) return false
  // Codex 配额数据存储在 codex 子对象中
  return meta.codex.primary_used_percent !== undefined || meta.codex.secondary_used_percent !== undefined
}

// 检查 Kiro 是否有配额数据
function hasKiroQuotaData(meta: UpstreamMetadata | null | undefined): boolean {
  if (!meta?.kiro) return false
  return meta.kiro.usage_percentage !== undefined || meta.kiro.usage_limit !== undefined
}

// 检查 Kiro 账户是否被封禁
function isKiroBanned(meta: UpstreamMetadata | null | undefined): boolean {
  if (!meta?.kiro) return false
  return meta.kiro.is_banned === true
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

// 检查 Antigravity 账户是否被禁止访问
function isAntigravityForbidden(meta: UpstreamMetadata | null | undefined): boolean {
  if (!meta?.antigravity) return false
  return meta.antigravity.is_forbidden === true
}

// 格式化 Kiro 更新时间
const formatKiroUpdatedAt = formatUpdatedAt

// 格式化 Kiro 使用量（带单位）
function formatKiroUsage(value: number | undefined): string {
  if (value === undefined || value === null) return '-'
  if (value >= 1000000) {
    return `${(value / 1000000).toFixed(1)}M`
  }
  if (value >= 1000) {
    return `${(value / 1000).toFixed(1)}K`
  }
  return value.toFixed(1)
}

// 格式化 Kiro 重置时间
function formatKiroResetTime(timestamp: number | undefined): string {
  if (!timestamp) return ''
  // timestamp 可能是毫秒或秒，需要判断
  const ts = timestamp > 1e12 ? timestamp : timestamp * 1000
  const now = Date.now()
  const diff = ts - now

  if (diff <= 0) {
    return '已重置'
  }

  const days = Math.floor(diff / (1000 * 60 * 60 * 24))
  const hours = Math.floor((diff % (1000 * 60 * 60 * 24)) / (1000 * 60 * 60))

  if (days > 0) {
    return `${days}天${hours}小时后`
  }

  const minutes = Math.floor((diff % (1000 * 60 * 60)) / (1000 * 60))
  if (hours > 0) {
    return `${hours}小时${minutes}分钟后`
  }

  return `${minutes}分钟后`
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

function shouldAutoRefreshCodexQuota(): boolean {
  if (provider.value?.provider_type !== 'codex') return false

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    const meta: UpstreamMetadata | null | undefined = key.upstream_metadata
    // 只要有一个活跃 key 没有配额数据，就刷新一次
    if (!hasCodexQuotaData(meta)) {
      return true
    }
  }

  return false
}

// 检查 OAuth Token 是否即将过期（Antigravity / Kiro ）
function isTokenExpiringSoon(key: EndpointAPIKey, now: number): boolean {
  return key.oauth_invalid_at == null
    && typeof key.oauth_expires_at === 'number'
    && (key.oauth_expires_at - now) <= AUTO_TOKEN_REFRESH_SKEW_SECONDS
}

function shouldAutoRefreshAntigravityQuota(): boolean {
  if (provider.value?.provider_type !== 'antigravity') return false
  const now = Math.floor(Date.now() / 1000)

  for (const { key } of allKeys.value) {
    if (!key.is_active) continue

    if (isTokenExpiringSoon(key, now)) return true

    const meta = key.upstream_metadata
    const updatedAt = meta?.antigravity?.updated_at
    const quotaByModel = meta?.antigravity?.quota_by_model

    // 只要有一个活跃 key 没有配额/为空/过期，就刷新一次（接口会批量刷新所有活跃 key）
    if (!quotaByModel || typeof quotaByModel !== 'object' || Object.keys(quotaByModel).length === 0) {
      return true
    }
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
    if (!hasKiroQuotaData(key.upstream_metadata)) {
      return true
    }
  }

  return false
}

// 通用的自动刷新配额函数（支持 Codex、Antigravity 和 Kiro）
async function autoRefreshQuotaInBackground() {
  if (!props.providerId) return
  if (refreshingQuota.value) return

  const providerType = provider.value?.provider_type
  if (providerType !== 'codex' && providerType !== 'antigravity' && providerType !== 'kiro') return

  // 检查是否需要刷新
  let shouldRefresh = false
  if (providerType === 'codex') {
    shouldRefresh = shouldAutoRefreshCodexQuota()
  } else if (providerType === 'antigravity') {
    shouldRefresh = shouldAutoRefreshAntigravityQuota()
  } else if (providerType === 'kiro') {
    shouldRefresh = shouldAutoRefreshKiroQuota()
  }
  if (!shouldRefresh) return

  let hadCachedQuota = false
  if (providerType === 'codex') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasCodexQuotaData(key.upstream_metadata))
  } else if (providerType === 'antigravity') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && key.upstream_metadata && hasAntigravityQuotaData(key.upstream_metadata))
  } else if (providerType === 'kiro') {
    hadCachedQuota = allKeys.value.some(({ key }) => key.is_active && hasKiroQuotaData(key.upstream_metadata))
  }

  refreshingQuota.value = true
  try {
    const result = await refreshProviderQuota(props.providerId)
    if (result.success > 0) {
      // 重新加载 keys 以更新配额显示
      await loadEndpoints()
    } else if (!hadCachedQuota && providerType === 'antigravity') {
      showError('没有获取到配额信息（请检查账号是否已授权、project_id 是否存在）', '提示')
    }
  } catch (err: any) {
    if (!hadCachedQuota && providerType === 'antigravity') {
      showError(err.response?.data?.detail || '后台刷新配额失败', '错误')
    }
  } finally {
    refreshingQuota.value = false
  }
}

async function openAntigravityQuotaDialog(key: EndpointAPIKey) {
  antigravityQuotaDialogKey.value = key
  antigravityQuotaDialogOpen.value = true

  // 没有配额数据时主动获取
  if (!key.upstream_metadata || !hasAntigravityQuotaData(key.upstream_metadata)) {
    if (refreshingQuota.value) return
    refreshingQuota.value = true
    try {
      const result = await refreshProviderQuota(props.providerId)
      if (result.success > 0) {
        await loadEndpoints()
        // 更新弹窗引用的 key 数据
        const updated = allKeys.value.find(({ key: k }) => k.id === key.id)
        if (updated) {
          antigravityQuotaDialogKey.value = updated.key
        }
      }
    } catch {
      // 静默失败，弹窗会显示"暂无配额数据"
    } finally {
      refreshingQuota.value = false
    }
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
  // 添加/修改 key 后自动获取 Antigravity 配额（新 key 的 upstream_metadata 为空）
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
    showSuccess(newStatus ? '密钥已启用' : '密钥已停用')
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '操作失败', '错误')
  } finally {
    togglingKeyId.value = null
  }
}

// ===== Key 级别代理配置 =====

/** 获取 Key 当前代理节点的名称（用于显示） */
function getKeyProxyNodeName(key: EndpointAPIKey): string | null {
  if (!key.proxy?.node_id) return null
  const node = proxyNodesStore.nodes.find(n => n.id === key.proxy!.node_id)
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
    showSuccess('代理节点已设置')
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '设置代理失败', '错误')
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
    showSuccess('已清除账号代理，将使用提供商级别代理')
    emit('refresh')
  } catch (err: any) {
    showError(err.response?.data?.detail || '清除代理失败', '错误')
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
    paid: 'Paid',
    team: 'Team',
    enterprise: 'Enterprise',
    ultra: 'Ultra',
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
  const planType = key.oauth_plan_type?.toLowerCase() || key.upstream_metadata?.codex?.plan_type?.toLowerCase()
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

function hasAntigravityQuotaData(metadata: UpstreamMetadata | null | undefined): boolean {
  const quotaByModel = metadata?.antigravity?.quota_by_model
  return !!quotaByModel && typeof quotaByModel === 'object' && Object.keys(quotaByModel).length > 0
}

function formatUpdatedAt(updatedAt: number): string {
  if (!updatedAt || typeof updatedAt !== 'number') return ''
  const now = Math.floor(Date.now() / 1000)
  const diff = now - updatedAt
  if (diff <= 60) return '刚刚更新'
  const minutes = Math.floor(diff / 60)
  if (minutes < 60) return `${minutes}分钟前更新`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}小时前更新`
  const days = Math.floor(hours / 24)
  return `${days}天前更新`
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

function getAntigravityQuotaItems(metadata: UpstreamMetadata | null | undefined): AntigravityQuotaItem[] {
  const quotaByModel = metadata?.antigravity?.quota_by_model
  if (!quotaByModel || typeof quotaByModel !== 'object') return []

  const items: AntigravityQuotaItem[] = []
  for (const [model, rawInfo] of Object.entries(quotaByModel)) {
    if (!model) continue
    const info: Partial<AntigravityModelQuota> = rawInfo || {}

    let usedPercent = Number(info.used_percent)
    if (!Number.isFinite(usedPercent)) {
      const remainingFraction = Number(info.remaining_fraction)
      if (Number.isFinite(remainingFraction)) {
        usedPercent = (1 - remainingFraction) * 100
      } else {
        continue
      }
    }

    if (usedPercent < 0) usedPercent = 0
    if (usedPercent > 100) usedPercent = 100

    const remainingPercent = Math.max(100 - usedPercent, 0)

    let resetSeconds: number | null = null
    if (typeof info.reset_time === 'string' && info.reset_time.trim()) {
      resetSeconds = secondsUntilReset(info.reset_time.trim())
    }

    items.push({
      model,
      label: model,
      usedPercent,
      remainingPercent,
      resetSeconds,
    })
  }

  // 按“最紧张”（已用最多）优先排序，便于快速定位额度风险；完整列表通过滚动展示
  items.sort((a, b) => (b.usedPercent - a.usedPercent) || a.model.localeCompare(b.model))
  return items
}

// Antigravity 配额分组定义（按匹配优先级排列，具体规则在前）
interface AntigravityQuotaGroup {
  key: string
  label: string
  match: (model: string) => boolean
}

const ANTIGRAVITY_QUOTA_GROUPS: AntigravityQuotaGroup[] = [
  { key: 'claude-gpt', label: 'Claude 4.5', match: m => m.includes('claude') },
  { key: 'gemini-3-pro', label: 'Gemini 3 Pro', match: m => m.includes('gemini-3-pro') && !m.includes('image') },
  { key: 'gemini-3-flash', label: 'Gemini 3 Flash', match: m => m.includes('gemini-3-flash') },
  { key: 'gemini-3-pro-image', label: 'Gemini 3 Pro Image', match: m => m.includes('gemini-3-pro-image') },
]

interface AntigravityQuotaSummaryItem {
  key: string
  label: string
  usedPercent: number       // 组内最高已用百分比（最紧张）
  remainingPercent: number  // 100 - usedPercent
  resetSeconds: number | null
}

function getAntigravityQuotaSummary(metadata: UpstreamMetadata | null | undefined): AntigravityQuotaSummaryItem[] {
  const items = getAntigravityQuotaItems(metadata)
  if (!items.length) return []

  // 将每个模型归入分组
  const groupMap = new Map<string, { label: string, maxUsed: number, resetSeconds: number | null }>()

  for (const item of items) {
    const model = item.model.toLowerCase()
    const group = ANTIGRAVITY_QUOTA_GROUPS.find(g => g.match(model))
    if (!group) continue

    const existing = groupMap.get(group.key)
    if (!existing) {
      groupMap.set(group.key, {
        label: group.label,
        maxUsed: item.usedPercent,
        resetSeconds: item.resetSeconds,
      })
    } else {
      if (item.usedPercent > existing.maxUsed) {
        existing.maxUsed = item.usedPercent
      }
      if (existing.resetSeconds === null) {
        existing.resetSeconds = item.resetSeconds
      } else if (item.resetSeconds !== null && item.resetSeconds < existing.resetSeconds) {
        existing.resetSeconds = item.resetSeconds
      }
    }
  }

  // 按 ANTIGRAVITY_QUOTA_GROUPS 定义的顺序输出
  const result: AntigravityQuotaSummaryItem[] = []
  for (const group of ANTIGRAVITY_QUOTA_GROUPS) {
    const data = groupMap.get(group.key)
    if (!data) continue
    result.push({
      key: group.key,
      label: data.label,
      usedPercent: data.maxUsed,
      remainingPercent: Math.max(100 - data.maxUsed, 0),
      resetSeconds: data.resetSeconds,
    })
  }
  return result
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
    paid: 'border-blue-500/50 text-blue-600 dark:text-blue-400',
    team: 'border-purple-500/50 text-purple-600 dark:text-purple-400',
    enterprise: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
    ultra: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
    'pro+': 'border-purple-500/50 text-purple-600 dark:text-purple-400',
    power: 'border-amber-500/50 text-amber-600 dark:text-amber-400',
  }
  return classes[planType.toLowerCase()] || ''
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
