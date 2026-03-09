<template>
  <div class="space-y-6 pb-8">
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- Header -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
        <!-- Mobile -->
        <div class="flex flex-col gap-3 sm:hidden">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <h3 class="text-base font-semibold">
                号池管理
                <span
                  v-if="poolHeaderMetaText"
                  class="ml-2 text-xs font-normal text-muted-foreground"
                >
                  | {{ poolHeaderMetaText }}
                </span>
              </h3>
            </div>
            <div class="flex items-center gap-1.5">
              <Button
                v-if="selectedProviderId"
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="添加账号"
                @click="showImportDialog = true"
              >
                <Upload class="w-3.5 h-3.5" />
              </Button>
              <ProviderProxyPopover
                v-if="selectedProviderId"
                :open="providerProxyMobilePopoverOpen"
                :node-id="selectedProviderData?.proxy?.node_id"
                :saving="savingProviderProxy"
                :title="getProviderProxyButtonTitle()"
                @update:open="(open: boolean) => handleProviderProxyPopoverToggle('mobile', open)"
                @select="setProviderProxy"
                @clear="clearProviderProxy"
              />
              <Button
                v-if="selectedProviderId"
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="高级设置"
                @click="showAdvancedDialog = true"
              >
                <Settings2 class="w-3.5 h-3.5" />
              </Button>
              <Button
                v-if="selectedProviderId"
                variant="outline"
                size="sm"
                class="h-8 px-2 text-xs gap-1"
                title="号池调度"
                @click="openSchedulingDialog()"
              >
                调度
                <ChevronDown class="w-3 h-3 text-muted-foreground" />
              </Button>
              <Button
                v-if="selectedProviderId"
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="账号"
                @click="showAccountBatchDialog = true"
              >
                <Users class="w-3.5 h-3.5" />
              </Button>
              <Button
                v-if="selectedProviderId"
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                :class="getProviderToggleButtonClass()"
                :disabled="togglingProviderStatus"
                :title="getProviderToggleButtonTitle()"
                @click="toggleSelectedProviderStatus"
              >
                <Power class="w-3.5 h-3.5" />
              </Button>
              <RefreshButton
                :loading="refreshCurrentPageLoading"
                :title="refreshButtonTitle"
                @click="refreshCurrentPage"
              />
            </div>
          </div>
          <!-- Filters (mobile) -->
          <div class="flex items-center gap-2">
            <Select
              v-model="selectedProviderIdProxy"
              :disabled="providerSelectDisabled"
            >
              <SelectTrigger
                class="flex-1 h-8 text-xs border-border/60"
                :disabled="providerSelectDisabled"
              >
                <SelectValue placeholder="选择 Provider" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="item in poolProviders"
                  :key="item.provider_id"
                  :value="item.provider_id"
                >
                  {{ item.provider_name }}
                  <span class="text-muted-foreground ml-1">({{ item.total_keys }})</span>
                  <span
                    v-if="!item.pool_enabled"
                    class="ml-1 text-[10px] text-amber-600"
                  >未启用</span>
                </SelectItem>
              </SelectContent>
            </Select>
            <Select v-model="statusFilter">
              <SelectTrigger class="w-24 h-8 text-xs border-border/60">
                <SelectValue placeholder="状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部
                </SelectItem>
                <SelectItem value="active">
                  活跃
                </SelectItem>
                <SelectItem value="cooldown">
                  冷却中
                </SelectItem>
                <SelectItem value="inactive">
                  禁用
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div
            v-if="selectedProviderId"
            class="relative"
          >
            <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
            <Input
              v-model="searchQuery"
              type="text"
              placeholder="搜索账号..."
              class="w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
            />
          </div>
        </div>

        <!-- Desktop -->
        <div class="hidden sm:flex items-center justify-between gap-4">
          <div class="flex items-center gap-2">
            <h3 class="text-base font-semibold">
              号池管理
              <span
                v-if="poolHeaderMetaText"
                class="ml-2 text-xs font-normal text-muted-foreground"
              >
                | {{ poolHeaderMetaText }}
              </span>
            </h3>
          </div>
          <div class="flex items-center gap-2">
            <Select
              v-model="selectedProviderIdProxy"
              :disabled="providerSelectDisabled"
            >
              <SelectTrigger
                class="w-36 h-8 text-xs border-border/60"
                :disabled="providerSelectDisabled"
              >
                <SelectValue placeholder="选择 Provider" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem
                  v-for="item in poolProviders"
                  :key="item.provider_id"
                  :value="item.provider_id"
                >
                  {{ item.provider_name }}
                  <span class="text-muted-foreground ml-1">({{ item.total_keys }})</span>
                  <span
                    v-if="!item.pool_enabled"
                    class="ml-1 text-[10px] text-amber-600"
                  >未启用</span>
                </SelectItem>
              </SelectContent>
            </Select>
            <div class="h-4 w-px bg-border" />
            <div
              v-if="selectedProviderId"
              class="relative"
            >
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                v-model="searchQuery"
                type="text"
                placeholder="搜索账号..."
                class="w-40 pl-8 pr-2 h-8 text-xs bg-background/50 border-border/60"
              />
            </div>
            <Select v-model="statusFilter">
              <SelectTrigger class="w-28 h-8 text-xs border-border/60">
                <SelectValue placeholder="全部状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部状态
                </SelectItem>
                <SelectItem value="active">
                  活跃
                </SelectItem>
                <SelectItem value="cooldown">
                  冷却中
                </SelectItem>
                <SelectItem value="inactive">
                  禁用
                </SelectItem>
              </SelectContent>
            </Select>
            <div
              v-if="selectedProviderId"
              class="h-4 w-px bg-border"
            />
            <button
              v-if="selectedProviderId"
              class="group inline-flex items-center gap-1.5 px-2.5 h-8 rounded-md border border-border/50 bg-muted/20 hover:bg-muted/40 hover:border-primary/40 transition-all duration-200 text-xs"
              title="点击调整号池调度"
              @click="openSchedulingDialog()"
            >
              <span class="text-muted-foreground/80 hidden lg:inline">调度:</span>
              <span class="font-medium text-foreground/90">{{ poolSchedulingLabel }}</span>
              <ChevronDown class="w-3 h-3 text-muted-foreground/70 group-hover:text-foreground transition-colors" />
            </button>
            <div
              v-if="selectedProviderId"
              class="h-4 w-px bg-border"
            />
            <Button
              v-if="selectedProviderId"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="添加账号"
              @click="showImportDialog = true"
            >
              <Upload class="w-3.5 h-3.5" />
            </Button>
            <ProviderProxyPopover
              v-if="selectedProviderId"
              :open="providerProxyDesktopPopoverOpen"
              :node-id="selectedProviderData?.proxy?.node_id"
              :saving="savingProviderProxy"
              :title="getProviderProxyButtonTitle()"
              @update:open="(open: boolean) => handleProviderProxyPopoverToggle('desktop', open)"
              @select="setProviderProxy"
              @clear="clearProviderProxy"
            />
            <Button
              v-if="selectedProviderId"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="高级设置"
              @click="showAdvancedDialog = true"
            >
              <Settings2 class="w-3.5 h-3.5" />
            </Button>
            <Button
              v-if="selectedProviderId"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="账号"
              @click="showAccountBatchDialog = true"
            >
              <Users class="w-3.5 h-3.5" />
            </Button>
            <Button
              v-if="selectedProviderId"
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              :class="getProviderToggleButtonClass()"
              :disabled="togglingProviderStatus"
              :title="getProviderToggleButtonTitle()"
              @click="toggleSelectedProviderStatus"
            >
              <Power class="w-3.5 h-3.5" />
            </Button>
            <RefreshButton
              :loading="refreshCurrentPageLoading"
              :title="refreshButtonTitle"
              @click="refreshCurrentPage"
            />
          </div>
        </div>
      </div>

      <!-- Loading (initial) -->
      <div
        v-if="overviewLoading"
        class="flex items-center justify-center py-16"
      >
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
      </div>

      <!-- No providers -->
      <div
        v-else-if="poolProviders.length === 0"
        class="flex flex-col items-center justify-center py-16 text-center"
      >
        <div class="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-muted">
          <Database class="h-8 w-8 text-muted-foreground" />
        </div>
        <p class="text-sm text-muted-foreground mt-4">
          暂无 Provider
        </p>
        <p class="text-xs text-muted-foreground mt-1">
          请先添加 Provider
        </p>
      </div>

      <!-- No provider selected -->
      <div
        v-else-if="!selectedProviderId"
        class="flex flex-col items-center justify-center py-16 text-center"
      >
        <div class="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-muted">
          <Database class="h-8 w-8 text-muted-foreground" />
        </div>
        <p class="text-sm text-muted-foreground mt-4">
          请选择一个 Provider 查看账号
        </p>
      </div>

      <!-- Loading keys -->
      <div
        v-else-if="keysLoading && keyPage.keys.length === 0"
        class="flex items-center justify-center py-16"
      >
        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-primary" />
      </div>

      <template v-else>
        <!-- Desktop table -->
        <div
          v-if="keyPage.keys.length > 0"
          class="hidden xl:block overflow-x-auto"
        >
          <Table class="w-full table-fixed">
            <TableHeader>
              <TableRow class="border-b border-border/60 hover:bg-transparent">
                <TableHead
                  class="font-semibold whitespace-nowrap"
                  :style="{ width: desktopColumnWidths.name }"
                >
                  名称
                </TableHead>
                <TableHead
                  v-if="showAccountQuotaColumn"
                  class="font-semibold whitespace-nowrap"
                  :style="{ width: desktopColumnWidths.quota }"
                >
                  配额
                </TableHead>
                <TableHead
                  class="px-2 font-semibold text-center whitespace-nowrap"
                  :style="{ width: desktopColumnWidths.stats }"
                >
                  统计
                </TableHead>
                <TableHead
                  class="font-semibold text-center whitespace-nowrap"
                  :style="{ width: desktopColumnWidths.lastUsed }"
                >
                  最后使用
                </TableHead>
                <TableHead
                  class="font-semibold text-center whitespace-nowrap"
                  :style="{ width: desktopColumnWidths.status }"
                >
                  状态
                </TableHead>
                <TableHead
                  class="px-2 font-semibold text-center whitespace-nowrap"
                  :style="{ width: desktopColumnWidths.actions }"
                >
                  操作
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              <TableRow
                v-for="key in keyPage.keys"
                :key="key.key_id"
                class="border-b border-border/40 last:border-b-0 hover:bg-muted/30 transition-colors"
                :class="getRowClass(key)"
              >
                <TableCell
                  class="py-3"
                >
                  <div class="min-w-0">
                    <div class="flex items-center gap-1.5 min-w-0">
                      <span class="text-sm truncate block">
                        {{ key.key_name || '未命名' }}
                      </span>
                    </div>
                    <div class="flex items-center flex-wrap gap-1 text-[11px] text-muted-foreground mt-0.5 min-w-0">
                      <input
                        v-if="editingPriorityKeyId === key.key_id"
                        :value="editingPriorityValue"
                        type="number"
                        min="1"
                        max="999999"
                        autofocus
                        class="h-[18px] w-10 rounded border border-primary/50 bg-background px-1 text-[10px] tabular-nums text-foreground outline-none ring-1 ring-primary/30 shrink-0 [appearance:textfield] [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:appearance-none"
                        @input="(e) => editingPriorityValue = Number((e.target as HTMLInputElement).value || 0)"
                        @blur="(e) => finishEditInternalPriority(key, e)"
                        @keydown.enter.prevent="(e) => finishEditInternalPriority(key, e)"
                        @keydown.esc.prevent="cancelEditInternalPriority"
                      >
                      <button
                        v-else
                        type="button"
                        class="h-4 px-1 rounded text-[10px] tabular-nums text-muted-foreground hover:text-foreground hover:bg-muted/40 transition-colors shrink-0"
                        title="点击编辑优先级"
                        @click="startEditInternalPriority(key)"
                      >
                        P{{ key.internal_priority ?? 50 }}
                      </button>
                      <Button
                        v-if="key.auth_type === 'oauth'"
                        variant="ghost"
                        size="icon"
                        class="h-4 w-4 shrink-0"
                        title="下载 OAuth 授权文件"
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
                      <span class="font-mono">
                        {{ key.auth_type === 'oauth' ? '[OAuth Token]' : (key.auth_type === 'service_account' ? '[Service Account]' : '[Key]') }}
                      </span>
                      <template v-if="key.auth_type === 'oauth'">
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-4 w-4 shrink-0"
                          :disabled="refreshingOAuthKeyId === key.key_id"
                          :title="getKeyOAuthExpires(key)?.isInvalid ? '重新授权' : '刷新 Token'"
                          @click.stop="handleRefreshOAuth(key)"
                        >
                          <RefreshCw
                            class="w-2.5 h-2.5"
                            :class="{ 'animate-spin': refreshingOAuthKeyId === key.key_id }"
                          />
                        </Button>
                        <span
                          v-if="getKeyOAuthExpires(key)"
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
                      </template>
                      <Badge
                        v-if="key.oauth_plan_type"
                        variant="outline"
                        class="text-[9px] px-1 py-0 h-4 shrink-0"
                        :class="getOAuthPlanTypeClass(key.oauth_plan_type)"
                      >
                        {{ formatOAuthPlanType(key.oauth_plan_type) }}
                      </Badge>
                    </div>
                  </div>
                </TableCell>
                <TableCell
                  v-if="showAccountQuotaColumn"
                  class="py-3 align-middle"
                >
                  <div
                    v-if="quotaProgressMap[key.key_id]?.length"
                    class="space-y-1 max-w-[220px]"
                  >
                    <div
                      v-for="(item, idx) in quotaProgressMap[key.key_id].slice(0, 2)"
                      :key="`${key.key_id}-quota-${idx}`"
                      class="w-full"
                    >
                      <div class="h-4 grid grid-cols-[20px_minmax(0,1fr)_42px] items-center gap-1 text-[10px] leading-tight">
                        <span
                          class="text-muted-foreground whitespace-nowrap text-right tabular-nums"
                          :title="getQuotaProgressTooltip(item)"
                        >
                          {{ getQuotaProgressLabel(item.label) }}
                        </span>
                        <div class="relative flex-1 h-1.5 bg-border rounded-full overflow-hidden">
                          <div
                            class="absolute left-0 top-0 h-full transition-all duration-300"
                            :class="getQuotaRemainingBarColorByRemaining(item.remainingPercent)"
                            :style="{ width: `${item.remainingPercent}%` }"
                          />
                        </div>
                        <span
                          class="tabular-nums text-right whitespace-nowrap"
                          :class="getQuotaRemainingClassByRemaining(item.remainingPercent)"
                        >
                          {{ item.remainingPercent.toFixed(1) }}%
                        </span>
                      </div>
                    </div>
                  </div>
                  <span
                    v-else-if="key.account_quota"
                    :class="getQuotaTextClass(key.account_quota)"
                  >
                    {{ key.account_quota }}
                  </span>
                  <span
                    v-else
                    class="text-xs text-muted-foreground"
                  >-</span>
                </TableCell>
                <TableCell class="py-3 px-2 align-middle">
                  <div class="w-[136px] mx-auto text-[10px] leading-4">
                    <div class="flex items-center justify-between gap-2">
                      <span class="text-muted-foreground">请求</span>
                      <span class="tabular-nums text-foreground/90">
                        {{ formatStatInteger(key.request_count) }}
                      </span>
                    </div>
                  </div>
                </TableCell>
                <TableCell class="py-3 text-center">
                  <span class="text-[10px] text-muted-foreground whitespace-nowrap">
                    {{ key.last_used_at ? formatRelativeTime(key.last_used_at) : '-' }}
                  </span>
                </TableCell>
                <TableCell class="py-3 text-center">
                  <Badge
                    :variant="getSchedulingBadgeVariant(key)"
                    class="text-[10px]"
                    :title="getSchedulingTitle(key)"
                  >
                    {{ getSchedulingBadgeLabel(key) }}
                  </Badge>
                </TableCell>
                <TableCell class="py-3 px-2 align-middle">
                  <div class="flex justify-center gap-0.5">
                    <Button
                      v-if="key.cooldown_reason"
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7 text-muted-foreground hover:text-green-600"
                      title="清除冷却"
                      @click="clearCooldown(key.key_id)"
                    >
                      <RefreshCw class="w-3.5 h-3.5" />
                    </Button>
                    <Button
                      v-if="key.circuit_breaker_open || (key.health_score ?? 1) < 0.5"
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7 text-green-600"
                      :disabled="recoveringHealthKeyId === key.key_id"
                      title="刷新健康状态"
                      @click="handleRecoverKey(key)"
                    >
                      <RefreshCw
                        class="w-3.5 h-3.5"
                        :class="{ 'animate-spin': recoveringHealthKeyId === key.key_id }"
                      />
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
                    <Popover
                      :open="proxyDesktopPopoverOpenKeyId === key.key_id"
                      @update:open="(v: boolean) => handleProxyDesktopPopoverToggle(key.key_id, v)"
                    >
                      <PopoverTrigger as-child>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7"
                          :class="key.proxy?.node_id ? 'text-blue-500' : ''"
                          :disabled="savingProxyKeyId === key.key_id"
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
                              :disabled="savingProxyKeyId === key.key_id"
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
                      title="编辑账号"
                      @click="handleEditKey(key)"
                    >
                      <SquarePen class="w-3.5 h-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7 text-foreground hover:text-foreground"
                      :disabled="togglingKeyId === key.key_id"
                      :title="key.is_active ? '禁用' : '启用'"
                      @click="toggleKeyActive(key)"
                    >
                      <Power class="w-3.5 h-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7 text-destructive hover:text-destructive"
                      :disabled="deletingKeyId === key.key_id"
                      title="删除账号"
                      @click="handleDeleteKey(key)"
                    >
                      <Trash2 class="w-3.5 h-3.5" />
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            </TableBody>
          </Table>
        </div>

        <!-- Mobile card list -->
        <div
          v-if="keyPage.keys.length > 0"
          class="xl:hidden divide-y divide-border/40"
        >
          <div
            v-for="key in keyPage.keys"
            :key="key.key_id"
            class="p-4 sm:p-5 hover:bg-muted/30 transition-colors"
            :class="getRowClass(key)"
          >
            <div class="flex items-center gap-3">
              <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2">
                  <span class="text-sm font-medium truncate">
                    {{ key.key_name || '未命名' }}
                  </span>
                  <Badge
                    :variant="getSchedulingBadgeVariant(key)"
                    class="text-[10px] shrink-0"
                    :title="getSchedulingTitle(key)"
                  >
                    {{ getSchedulingBadgeLabel(key) }}
                  </Badge>
                  <span
                    class="text-[10px] font-medium tabular-nums"
                    :class="getHealthScoreColor(key.health_score ?? 1)"
                  >
                    {{ ((key.health_score ?? 1) * 100).toFixed(0) }}%
                  </span>
                </div>
                <div class="flex items-center gap-1 text-[11px] text-muted-foreground mt-0.5 min-w-0">
                  <button
                    type="button"
                    class="h-4 px-1 rounded text-[10px] tabular-nums text-muted-foreground hover:text-foreground hover:bg-muted/40 transition-colors shrink-0"
                    title="点击修改优先级"
                    @click="quickEditInternalPriority(key)"
                  >
                    P{{ key.internal_priority ?? 50 }}
                  </button>
                  <Button
                    v-if="key.auth_type === 'oauth'"
                    variant="ghost"
                    size="icon"
                    class="h-4 w-4 shrink-0"
                    title="下载 OAuth 授权文件"
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
                  <span class="font-mono">
                    {{ key.auth_type === 'oauth' ? '[OAuth Token]' : (key.auth_type === 'service_account' ? '[Service Account]' : '[Key]') }}
                  </span>
                  <template v-if="key.auth_type === 'oauth'">
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-4 w-4 shrink-0"
                      :disabled="refreshingOAuthKeyId === key.key_id"
                      :title="getKeyOAuthExpires(key)?.isInvalid ? '重新授权' : '刷新 Token'"
                      @click.stop="handleRefreshOAuth(key)"
                    >
                      <RefreshCw
                        class="w-2.5 h-2.5"
                        :class="{ 'animate-spin': refreshingOAuthKeyId === key.key_id }"
                      />
                    </Button>
                    <span
                      v-if="getKeyOAuthExpires(key)"
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
                  </template>
                  <Badge
                    v-if="key.oauth_plan_type"
                    variant="outline"
                    class="text-[9px] px-1 py-0 h-4 shrink-0"
                    :class="getOAuthPlanTypeClass(key.oauth_plan_type)"
                  >
                    {{ formatOAuthPlanType(key.oauth_plan_type) }}
                  </Badge>
                </div>
              </div>
              <div class="flex items-center gap-0.5 shrink-0 flex-wrap justify-end max-w-[210px]">
                <Button
                  v-if="key.cooldown_reason"
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-muted-foreground hover:text-green-600"
                  title="清除冷却"
                  @click="clearCooldown(key.key_id)"
                >
                  <RefreshCw class="w-3.5 h-3.5" />
                </Button>
                <Button
                  v-if="key.circuit_breaker_open || (key.health_score ?? 1) < 0.5"
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-green-600"
                  :disabled="recoveringHealthKeyId === key.key_id"
                  title="刷新健康状态"
                  @click="handleRecoverKey(key)"
                >
                  <RefreshCw
                    class="w-3.5 h-3.5"
                    :class="{ 'animate-spin': recoveringHealthKeyId === key.key_id }"
                  />
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
                <Popover
                  :open="proxyMobilePopoverOpenKeyId === key.key_id"
                  @update:open="(v: boolean) => handleProxyMobilePopoverToggle(key.key_id, v)"
                >
                  <PopoverTrigger as-child>
                    <Button
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7"
                      :class="key.proxy?.node_id ? 'text-blue-500' : ''"
                      :disabled="savingProxyKeyId === key.key_id"
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
                          :disabled="savingProxyKeyId === key.key_id"
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
                  title="编辑账号"
                  @click="handleEditKey(key)"
                >
                  <SquarePen class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-foreground hover:text-foreground"
                  :disabled="togglingKeyId === key.key_id"
                  :title="key.is_active ? '禁用' : '启用'"
                  @click="toggleKeyActive(key)"
                >
                  <Power class="w-3.5 h-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 text-destructive hover:text-destructive"
                  :disabled="deletingKeyId === key.key_id"
                  title="删除账号"
                  @click="handleDeleteKey(key)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </Button>
              </div>
            </div>
            <div
              class="mt-2.5 grid gap-2"
              :class="showAccountQuotaColumn ? 'grid-cols-2 sm:grid-cols-3 lg:grid-cols-4' : 'grid-cols-2 sm:grid-cols-3'"
            >
              <div class="p-2 bg-muted/50 rounded-lg text-xs">
                <div class="text-muted-foreground mb-0.5">
                  最后使用
                </div>
                <div class="text-[11px]">
                  {{ key.last_used_at ? formatRelativeTime(key.last_used_at) : '-' }}
                </div>
              </div>
              <div class="p-2 bg-muted/50 rounded-lg text-xs">
                <div class="text-muted-foreground mb-0.5">
                  统计
                </div>
                <div class="text-[10px]">
                  <div class="flex items-center justify-between gap-2">
                    <span class="text-muted-foreground">请求</span>
                    <span class="tabular-nums">{{ formatStatInteger(key.request_count) }}</span>
                  </div>
                </div>
              </div>
              <div
                v-if="showAccountQuotaColumn"
                class="p-2 bg-muted/50 rounded-lg text-xs"
              >
                <div class="text-muted-foreground mb-0.5">
                  配额
                </div>
                <div
                  v-if="quotaProgressMap[key.key_id]?.length"
                  class="space-y-1"
                >
                  <div
                    v-for="(item, idx) in quotaProgressMap[key.key_id]"
                    :key="`${key.key_id}-quota-mobile-${idx}`"
                    class="w-full"
                  >
                    <div class="grid grid-cols-[20px_minmax(0,1fr)_42px] items-center gap-1 text-[10px] leading-tight">
                      <span
                        class="text-muted-foreground whitespace-nowrap text-right tabular-nums"
                        :title="getQuotaProgressTooltip(item)"
                      >
                        {{ getQuotaProgressLabel(item.label) }}
                      </span>
                      <div class="relative flex-1 h-1.5 bg-border rounded-full overflow-hidden">
                        <div
                          class="absolute left-0 top-0 h-full transition-all duration-300"
                          :class="getQuotaRemainingBarColorByRemaining(item.remainingPercent)"
                          :style="{ width: `${item.remainingPercent}%` }"
                        />
                      </div>
                      <span
                        class="tabular-nums text-right whitespace-nowrap"
                        :class="getQuotaRemainingClassByRemaining(item.remainingPercent)"
                      >
                        {{ item.remainingPercent.toFixed(1) }}%
                      </span>
                    </div>
                  </div>
                </div>
                <div
                  v-else-if="key.account_quota"
                  :class="getQuotaTextClass(key.account_quota)"
                >
                  {{ key.account_quota }}
                </div>
                <div
                  v-else
                  class="text-muted-foreground"
                >
                  -
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Empty keys -->
        <div
          v-if="keyPage.keys.length === 0 && !keysLoading"
          class="flex flex-col items-center justify-center py-16 text-center"
        >
          <div class="mx-auto flex h-16 w-16 items-center justify-center rounded-full bg-muted">
            <KeyRound class="h-8 w-8 text-muted-foreground" />
          </div>
          <p class="text-sm text-muted-foreground mt-4">
            暂无账号
          </p>
          <Button
            variant="outline"
            size="sm"
            class="mt-3"
            @click="showImportDialog = true"
          >
            <Upload class="w-3.5 h-3.5 mr-1.5" />
            添加账号
          </Button>
        </div>

        <!-- Pagination -->
        <Pagination
          v-if="keyPage.keys.length > 0"
          :current="currentPage"
          :total="keyPage.total"
          :page-size="pageSize"
          cache-key="pool-keys-page-size"
          @update:current="currentPage = $event"
          @update:page-size="pageSize = $event"
        />
      </template>
    </Card>

    <!-- Dialogs -->
    <OAuthAccountDialog
      v-if="selectedProviderId"
      :open="showImportDialog"
      :provider-id="selectedProviderId"
      :provider-type="selectedProviderType || null"
      @close="showImportDialog = false"
      @saved="handleAccountDialogSaved"
    />
    <PoolSchedulingDialog
      v-if="selectedProviderId"
      v-model="showSchedulingDialog"
      :provider-id="selectedProviderId"
      :provider-type="selectedProviderType"
      :current-config="selectedProviderConfig"
      @saved="handleSchedulingSaved"
    />
    <PoolAdvancedDialog
      v-if="selectedProviderId"
      v-model="showAdvancedDialog"
      :provider-id="selectedProviderId"
      :provider-type="selectedProviderType"
      :current-config="selectedProviderConfig"
      :current-claude-config="selectedProviderClaudeConfig"
      @saved="handleSchedulingSaved"
    />
    <PoolAccountBatchDialog
      v-if="selectedProviderId"
      v-model="showAccountBatchDialog"
      :provider-id="selectedProviderId"
      :provider-name="selectedProviderData?.name || ''"
      :batch-concurrency="selectedProviderConfig?.batch_concurrency"
      @changed="handleAccountBatchChanged"
    />
    <KeyFormDialog
      v-if="selectedProviderId"
      :open="keyFormDialogOpen"
      :endpoint="null"
      :provider-type="selectedProviderData?.provider_type || selectedProviderType"
      :editing-key="editingKey"
      :provider-id="selectedProviderId"
      :available-api-formats="selectedProviderData?.api_formats || []"
      @close="closeKeyFormDialog"
      @saved="handleDialogSaved"
    />
    <OAuthKeyEditDialog
      :open="oauthKeyEditDialogOpen"
      :editing-key="editingKey"
      @close="closeOAuthEditDialog"
      @saved="handleDialogSaved"
    />
    <KeyAllowedModelsEditDialog
      v-if="selectedProviderId"
      :open="keyPermissionsDialogOpen"
      :api-key="editingKey"
      :provider-id="selectedProviderId || ''"
      @close="closeKeyPermissionsDialog"
      @saved="handleDialogSaved"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import {
  Search,
  Upload,
  ChevronDown,
  RefreshCw,
  Power,
  Database,
  KeyRound,
  Download,
  Copy,
  Shield,
  Globe,
  SquarePen,
  Trash2,
  Users,
  Settings2,
} from 'lucide-vue-next'

import {
  Card,
  Badge,
  Button,
  Input,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
  Pagination,
  Popover,
  PopoverTrigger,
  PopoverContent,
} from '@/components/ui'
import RefreshButton from '@/components/ui/refresh-button.vue'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import { useCountdownTimer, getOAuthExpiresCountdown } from '@/composables/useCountdownTimer'
import { useConfirm } from '@/composables/useConfirm'
import { parseApiError } from '@/utils/errorParser'
import {
  getPoolOverview,
  getPoolSchedulingPresets,
  listPoolKeys,
  clearPoolCooldown,
} from '@/api/endpoints/pool'
import {
  revealEndpointKey,
  exportKey,
  deleteEndpointKey,
  updateProviderKey,
  refreshProviderQuota,
} from '@/api/endpoints/keys'
import { refreshProviderOAuth } from '@/api/endpoints/provider_oauth'
import { recoverKeyHealth } from '@/api/endpoints/health'
import type {
  PoolOverviewItem,
  PoolKeyDetail,
  PoolKeysPageResponse,
  PoolPresetMeta,
} from '@/api/endpoints/pool'
import type { ClaudeCodeAdvancedConfig, EndpointAPIKey, PoolAdvancedConfig, ProviderWithEndpointsSummary } from '@/api/endpoints/types/provider'
import { getProvider, updateProvider } from '@/api/endpoints'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import PoolSchedulingDialog from '@/features/pool/components/PoolSchedulingDialog.vue'
import PoolAdvancedDialog from '@/features/pool/components/PoolAdvancedDialog.vue'
import PoolAccountBatchDialog from '@/features/pool/components/PoolAccountBatchDialog.vue'
import ProviderProxyPopover from '@/features/pool/components/ProviderProxyPopover.vue'
import KeyAllowedModelsEditDialog from '@/features/providers/components/KeyAllowedModelsEditDialog.vue'
import KeyFormDialog from '@/features/providers/components/KeyFormDialog.vue'
import OAuthKeyEditDialog from '@/features/providers/components/OAuthKeyEditDialog.vue'
import OAuthAccountDialog from '@/features/providers/components/OAuthAccountDialog.vue'
import ProxyNodeSelect from '@/features/providers/components/ProxyNodeSelect.vue'
import { isAccountLevelBlockReason, classifyAccountBlockLabel, cleanAccountBlockReason } from '@/utils/accountBlock'

const { success, error: showError, warning: showWarning } = useToast()
const { confirm } = useConfirm()
const { copyToClipboard } = useClipboard()
const { tick: countdownTick, start: startCountdownTimer } = useCountdownTimer()
const proxyNodesStore = useProxyNodesStore()

// --- Overview ---
const poolProviders = ref<PoolOverviewItem[]>([])
const overviewLoading = ref(true)
let overviewRequestId = 0
let selectProviderRequestId = 0
let providerDataRequestId = 0
let keysRequestId = 0
let keysSearchDebounceTimer: number | null = null
let suppressFiltersWatch = false

async function loadOverview() {
  const requestId = ++overviewRequestId
  overviewLoading.value = true
  try {
    const res = await getPoolOverview()
    if (requestId !== overviewRequestId) return
    const allProviders = Array.isArray(res.items) ? res.items : []
    const enabledProviders = allProviders.filter(item => item.pool_enabled)
    poolProviders.value = enabledProviders

    // Keep selected provider aligned with dropdown options.
    const selectedId = selectedProviderId.value
    const selectedStillExists = Boolean(
      selectedId && enabledProviders.some(item => item.provider_id === selectedId),
    )

    if (!selectedStillExists) {
      if (enabledProviders.length > 0) {
        // Do not block overview loading on key list fetch; keys area has its own loader.
        void selectProvider(enabledProviders[0].provider_id)
      } else {
        selectedProviderId.value = null
        selectedProviderData.value = null
        showAccountBatchDialog.value = false
        closeProviderProxyPopovers()
      }
    }
  } catch (err) {
    if (requestId !== overviewRequestId) return
    showError(parseApiError(err))
  } finally {
    if (requestId === overviewRequestId) {
      overviewLoading.value = false
    }
  }
}

async function handleSchedulingSaved(updatedProvider: ProviderWithEndpointsSummary) {
  // 优先回写保存接口返回值，避免弹窗立即重开时读到旧配置。
  if (selectedProviderId.value && updatedProvider.id === selectedProviderId.value) {
    selectedProviderData.value = updatedProvider
  }
  showSchedulingDialog.value = false
  showAdvancedDialog.value = false
  await loadOverview()
}

// --- Provider Selection ---
const selectedProviderId = ref<string | null>(null)
const selectedProviderData = ref<ProviderWithEndpointsSummary | null>(null)

// Proxy for Select v-model (string, not string|null)
const selectedProviderIdProxy = computed({
  get: () => selectedProviderId.value ?? '',
  set: (val: string) => {
    if (val && val !== selectedProviderId.value) {
      selectProvider(val)
    }
  },
})

const providerSelectDisabled = computed(() => poolProviders.value.length === 0)

const selectedProviderConfig = computed<PoolAdvancedConfig | null>(() => {
  return (selectedProviderData.value as Record<string, unknown> | null)?.pool_advanced as PoolAdvancedConfig | null ?? null
})

const selectedProviderClaudeConfig = computed(() => {
  return (selectedProviderData.value as Record<string, unknown> | null)?.claude_code_advanced as ClaudeCodeAdvancedConfig | null ?? null
})

const DEFAULT_ENABLED_PRESETS = new Set(['cache_affinity', 'recent_refresh'])

const DEFAULT_PRESET_LABELS: Record<string, string> = {
  lru: 'LRU',
  free_team_first: 'Free/Team',
  recent_refresh: '刷新优先',
  quota_balanced: '额度均衡',
  single_account: '单号优先',
}
const presetLabelsByName = ref<Record<string, string>>({ ...DEFAULT_PRESET_LABELS })

function normalizePresetName(value: unknown): string {
  return String(value ?? '').trim().toLowerCase()
}

async function loadSchedulingPresetMetas(): Promise<void> {
  try {
    const metas = await getPoolSchedulingPresets()
    const next: Record<string, string> = {}
    for (const meta of metas as PoolPresetMeta[]) {
      const name = normalizePresetName(meta.name)
      if (!name) continue
      const label = String(meta.label ?? '').trim()
      next[name] = label || name
    }
    if (Object.keys(next).length > 0) {
      presetLabelsByName.value = next
    }
  } catch {
    presetLabelsByName.value = { ...DEFAULT_PRESET_LABELS }
  }
}

const selectedProviderOverview = computed<PoolOverviewItem | null>(() => {
  const selectedId = selectedProviderId.value
  if (!selectedId) return null
  return poolProviders.value.find(item => item.provider_id === selectedId) || null
})

const poolSchedulingLabel = computed(() => {
  if (!selectedProviderConfig.value && selectedProviderOverview.value?.pool_enabled === false) {
    return '未启用'
  }

  const cfg = selectedProviderConfig.value

  // No pool_advanced config at all: use default enabled presets count
  if (!cfg) return `${DEFAULT_ENABLED_PRESETS.size} 维度`

  const presets = Array.isArray(cfg.scheduling_presets) ? cfg.scheduling_presets : []
  const presetLabels = presetLabelsByName.value

  if (presets.length > 0) {
    // New format: object list with { preset, enabled }
    const first = presets[0]
    if (typeof first === 'object' && first !== null && 'preset' in first) {
      const enabledCount = (presets as Array<{ preset: string; enabled?: boolean }>)
        .filter(p => p.enabled !== false)
        .length
      return enabledCount > 0 ? `${enabledCount} 维度` : '无启用维度'
    }

    // Legacy string list format
    if (typeof first === 'string') {
      const labels = (presets as string[])
        .map(p => presetLabels[normalizePresetName(p)])
        .filter(Boolean)
      if (labels.length > 0) return `${labels.length} 维度`
    }
  }

  // Fallback: legacy scheduling_mode field
  if (cfg.scheduling_mode === 'multi_score') {
    return '多维评分'
  }

  const lruEnabled = cfg.lru_enabled !== false
  const stickyTtl = Number(cfg.sticky_session_ttl_seconds ?? 3600)
  const stickyEnabled = Number.isFinite(stickyTtl) && stickyTtl > 0

  if (lruEnabled && stickyEnabled) return 'LRU + 粘性'
  if (lruEnabled) return 'LRU'
  if (stickyEnabled) return '粘性'
  return '随机'
})

const selectedProviderType = computed(() => {
  const fromDetail = String(selectedProviderData.value?.provider_type || '').trim().toLowerCase()
  if (fromDetail) return fromDetail
  const fromOverview = selectedProviderOverview.value?.provider_type
  return String(fromOverview || '').trim().toLowerCase()
})

const selectedProviderStatusText = computed(() => {
  if (!selectedProviderId.value) return ''
  const providerActive = selectedProviderData.value?.is_active
  if (providerActive === false) return '禁用'
  if (providerActive === true) return '启用'
  if (selectedProviderOverview.value?.pool_enabled === false) return '禁用'
  if (selectedProviderOverview.value?.pool_enabled === true) return '启用'
  return ''
})

const poolHeaderMetaText = computed(() => {
  const providerType = selectedProviderType.value
  const status = selectedProviderStatusText.value
  if (providerType && status) return `${providerType} | ${status}`
  return providerType || status || ''
})

const showAccountQuotaColumn = computed(() => {
  return selectedProviderType.value === 'codex'
    || selectedProviderType.value === 'gemini_cli'
    || selectedProviderType.value === 'kiro'
    || selectedProviderType.value === 'antigravity'
})

const desktopColumnWidths = computed(() => {
  if (showAccountQuotaColumn.value) {
    return {
      name: '28%',
      quota: '23%',
      stats: '15%',
      lastUsed: '10%',
      status: '8%',
      actions: '16%',
    }
  }
  return {
    name: '40%',
    quota: '0%',
    stats: '18%',
    lastUsed: '12%',
    status: '10%',
    actions: '20%',
  }
})

async function selectProvider(id: string) {
  const requestId = ++selectProviderRequestId
  selectedProviderId.value = id
  editingKeyDetail.value = null
  showAccountBatchDialog.value = false
  keyPermissionsDialogOpen.value = false
  keyFormDialogOpen.value = false
  oauthKeyEditDialogOpen.value = false
  closeProviderProxyPopovers()
  proxyDesktopPopoverOpenKeyId.value = null
  proxyMobilePopoverOpenKeyId.value = null
  suppressFiltersWatch = true
  currentPage.value = 1
  searchQuery.value = ''
  statusFilter.value = 'all'
  suppressFiltersWatch = false
  if (keysSearchDebounceTimer !== null) {
    clearTimeout(keysSearchDebounceTimer)
    keysSearchDebounceTimer = null
  }
  const keysTask = loadKeys()
  // Provider summary is non-blocking for key list rendering.
  void loadProviderData(id)
  await keysTask
  if (requestId !== selectProviderRequestId) return
}

async function loadProviderData(id: string) {
  const requestId = ++providerDataRequestId
  try {
    const providerData = await getProvider(id)
    if (requestId !== providerDataRequestId || selectedProviderId.value !== id) return
    selectedProviderData.value = providerData
  } catch {
    if (requestId !== providerDataRequestId || selectedProviderId.value !== id) return
    selectedProviderData.value = null
  }
}

async function refresh() {
  await loadKeys()
}

// --- Keys ---
const keyPage = ref<PoolKeysPageResponse>({ total: 0, page: 1, page_size: 50, keys: [] })
const keysLoading = ref(false)
const refreshingCurrentPageQuota = ref(false)
const searchQuery = ref('')
const statusFilter = ref('all')
const currentPage = ref(1)
const pageSize = ref(50)
const MANUAL_QUOTA_REFRESH_COOLDOWN_SECONDS = 5 * 60
const refreshingOAuthKeyId = ref<string | null>(null)
const revealedKeys = ref<Map<string, string>>(new Map())
const recoveringHealthKeyId = ref<string | null>(null)
const savingProxyKeyId = ref<string | null>(null)
const proxyDesktopPopoverOpenKeyId = ref<string | null>(null)
const proxyMobilePopoverOpenKeyId = ref<string | null>(null)
const deletingKeyId = ref<string | null>(null)
const togglingKeyId = ref<string | null>(null)
const editingPriorityKeyId = ref<string | null>(null)
const editingPriorityValue = ref<number>(0)
const prioritySavingKeyId = ref<string | null>(null)

const keyPermissionsDialogOpen = ref(false)
const keyFormDialogOpen = ref(false)
const oauthKeyEditDialogOpen = ref(false)
const editingKeyDetail = ref<PoolKeyDetail | null>(null)

interface QuotaProgressItem {
  label: string
  remainingPercent: number
  detail?: string
  resetAtSeconds?: number | null
}

const quotaProgressMap = computed<Record<string, QuotaProgressItem[]>>(() => {
  const map: Record<string, QuotaProgressItem[]> = {}
  for (const key of keyPage.value.keys) {
    map[key.key_id] = parseQuotaProgressItems(key.account_quota)
  }
  return map
})

const quotaRefreshSupported = computed(() => {
  return selectedProviderType.value === 'codex'
    || selectedProviderType.value === 'kiro'
    || selectedProviderType.value === 'antigravity'
})

const refreshCurrentPageLoading = computed(() => {
  return keysLoading.value || refreshingCurrentPageQuota.value
})

function normalizeQuotaUpdatedAt(raw: number | null | undefined): number | null {
  const value = Number(raw ?? 0)
  if (!Number.isFinite(value) || value <= 0) return null
  if (value > 1_000_000_000_000) {
    return Math.floor(value / 1000)
  }
  return Math.floor(value)
}

const currentPageQuotaRefreshStats = computed(() => {
  void countdownTick.value
  const seen = new Set<string>()
  const eligibleIds: string[] = []
  let cooledDownCount = 0
  let minRemainingSeconds = 0
  const nowSeconds = Math.floor(Date.now() / 1000)
  for (const key of keyPage.value.keys) {
    const id = String(key.key_id || '').trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    const updatedAt = normalizeQuotaUpdatedAt(key.quota_updated_at ?? null)
    if (updatedAt == null) {
      eligibleIds.push(id)
      continue
    }
    const remaining = MANUAL_QUOTA_REFRESH_COOLDOWN_SECONDS - (nowSeconds - updatedAt)
    if (remaining > 0) {
      cooledDownCount += 1
      if (minRemainingSeconds <= 0 || remaining < minRemainingSeconds) {
        minRemainingSeconds = remaining
      }
      continue
    }
    eligibleIds.push(id)
  }
  return {
    total: seen.size,
    eligibleIds,
    cooledDownCount,
    minRemainingSeconds,
  }
})

async function refreshCurrentPageQuotaInBackground(
  options: { silent?: boolean; reloadAfter?: boolean } = {},
): Promise<boolean> {
  if (!selectedProviderId.value || !quotaRefreshSupported.value) return false

  const providerId = selectedProviderId.value
  const quotaStats = currentPageQuotaRefreshStats.value
  if (quotaStats.eligibleIds.length === 0) {
    if (!options.silent && quotaStats.total > 0 && quotaStats.cooledDownCount > 0) {
      const waitText = quotaStats.minRemainingSeconds > 0
        ? formatTTL(quotaStats.minRemainingSeconds)
        : '稍后'
      showWarning(`当前页额度均在冷却中，请 ${waitText} 后再试`)
    }
    return false
  }

  if (refreshingCurrentPageQuota.value) {
    return false
  }

  refreshingCurrentPageQuota.value = true
  try {
    const result = await refreshProviderQuota(providerId, quotaStats.eligibleIds)
    const successCount = Number(result.success || 0)
    const failedCount = Number(result.failed || 0)
    const skippedCount = Math.max(quotaStats.total - quotaStats.eligibleIds.length, 0)

    // 刷新当前页数据，展示最新额度与状态
    if (selectedProviderId.value === providerId && options.reloadAfter !== false) {
      await loadKeys()
    }

    if (!options.silent) {
      const skippedText = skippedCount > 0 ? `，冷却跳过 ${skippedCount}` : ''
      success(`当前页额度刷新完成：成功 ${successCount}，失败 ${failedCount}${skippedText}`)
    }
    return true
  } catch (err) {
    showError(parseApiError(err, '刷新当前页额度失败'))
    return false
  } finally {
    refreshingCurrentPageQuota.value = false
  }
}

const refreshButtonTitle = computed(() => {
  if (refreshCurrentPageLoading.value) return '刷新中...'
  if (!selectedProviderId.value) return '刷新'
  if (!quotaRefreshSupported.value) return '刷新数据'

  const quotaStats = currentPageQuotaRefreshStats.value
  if (quotaStats.total === 0) return '刷新数据和额度'
  if (quotaStats.eligibleIds.length === 0 && quotaStats.cooledDownCount > 0) {
    const waitText = quotaStats.minRemainingSeconds > 0
      ? formatTTL(quotaStats.minRemainingSeconds)
      : '稍后'
    return `刷新数据（额度冷却 ${waitText}）`
  }
  if (quotaStats.cooledDownCount > 0) {
    return `刷新数据和额度（可刷新 ${quotaStats.eligibleIds.length}/${quotaStats.total}）`
  }
  return '刷新数据和额度'
})

async function refreshCurrentPage() {
  const quotaDidReload = await refreshCurrentPageQuotaInBackground({ reloadAfter: true })
  if (!quotaDidReload) {
    await refresh()
  }
}

async function loadKeys() {
  if (!selectedProviderId.value) return
  const requestId = ++keysRequestId
  const providerId = selectedProviderId.value
  const page = currentPage.value
  const pageSizeValue = pageSize.value
  const search = searchQuery.value || undefined
  const status = statusFilter.value as 'all' | 'active' | 'cooldown' | 'inactive'
  keysLoading.value = true
  try {
    const nextPage = await listPoolKeys(providerId, {
      page,
      page_size: pageSizeValue,
      search,
      status,
    })
    if (requestId !== keysRequestId || selectedProviderId.value !== providerId) return
    keyPage.value = nextPage
  } catch (err) {
    if (requestId !== keysRequestId || selectedProviderId.value !== providerId) return
    showError(parseApiError(err))
  } finally {
    if (requestId === keysRequestId) {
      keysLoading.value = false
    }
  }
}

watch([currentPage, pageSize], () => {
  void loadKeys()
})

watch(statusFilter, () => {
  if (suppressFiltersWatch) return
  currentPage.value = 1
  void loadKeys()
})

watch(searchQuery, () => {
  if (suppressFiltersWatch) return
  currentPage.value = 1
  if (keysSearchDebounceTimer !== null) {
    clearTimeout(keysSearchDebounceTimer)
  }
  keysSearchDebounceTimer = window.setTimeout(() => {
    keysSearchDebounceTimer = null
    void loadKeys()
  }, 300)
})

function normalizeAuthTypeForEdit(authType: string): EndpointAPIKey['auth_type'] {
  if (authType === 'oauth' || authType === 'service_account') return authType
  return 'api_key'
}

function toEndpointApiKey(key: PoolKeyDetail): EndpointAPIKey {
  const nowIso = new Date().toISOString()
  return {
    id: key.key_id,
    provider_id: selectedProviderId.value || '',
    api_formats: key.api_formats || [],
    api_key_masked: key.auth_type === 'oauth'
      ? '[OAuth Token]'
      : key.auth_type === 'service_account'
        ? '[Service Account]'
        : '[Key]',
    auth_type: normalizeAuthTypeForEdit(key.auth_type),
    name: key.key_name || '未命名',
    rate_multipliers: key.rate_multipliers ?? null,
    internal_priority: key.internal_priority ?? 50,
    rpm_limit: key.rpm_limit ?? null,
    allowed_models: key.allowed_models ?? null,
    capabilities: key.capabilities ?? null,
    cache_ttl_minutes: key.cache_ttl_minutes ?? 5,
    max_probe_interval_minutes: key.max_probe_interval_minutes ?? 32,
    health_score: key.health_score ?? 1,
    circuit_breaker_open: key.circuit_breaker_open ?? false,
    consecutive_failures: 0,
    request_count: 0,
    success_count: 0,
    error_count: 0,
    success_rate: 0,
    avg_response_time_ms: 0,
    is_active: key.is_active,
    note: key.note || '',
    last_used_at: key.last_used_at || undefined,
    created_at: key.created_at || nowIso,
    updated_at: nowIso,
    auto_fetch_models: key.auto_fetch_models ?? false,
    locked_models: key.locked_models || [],
    model_include_patterns: key.model_include_patterns || [],
    model_exclude_patterns: key.model_exclude_patterns || [],
    oauth_expires_at: key.oauth_expires_at ?? null,
    oauth_plan_type: key.oauth_plan_type ?? null,
    oauth_invalid_at: key.oauth_invalid_at ?? null,
    oauth_invalid_reason: key.oauth_invalid_reason ?? null,
    proxy: key.proxy ?? null,
  }
}

const editingKey = computed<EndpointAPIKey | null>(() => {
  if (!editingKeyDetail.value) return null
  return toEndpointApiKey(editingKeyDetail.value)
})

function sortCurrentPageKeysByPriority() {
  keyPage.value.keys = [...keyPage.value.keys].sort((a, b) => {
    const pa = Number(a.internal_priority ?? 50)
    const pb = Number(b.internal_priority ?? 50)
    if (pa !== pb) return pa - pb
    return (a.created_at || '').localeCompare(b.created_at || '')
  })
}

function startEditInternalPriority(key: PoolKeyDetail) {
  editingPriorityKeyId.value = key.key_id
  editingPriorityValue.value = Number(key.internal_priority ?? 50)
}

function cancelEditInternalPriority() {
  editingPriorityKeyId.value = null
  editingPriorityValue.value = 0
}

async function applyInternalPriority(key: PoolKeyDetail, nextPriority: number) {
  const normalized = Math.max(1, Math.min(999999, Math.floor(nextPriority)))
  if (Number(key.internal_priority ?? 50) === normalized) return

  prioritySavingKeyId.value = key.key_id
  try {
    await updateProviderKey(key.key_id, { internal_priority: normalized })
    key.internal_priority = normalized
    sortCurrentPageKeysByPriority()
    success('账号优先级已更新')
  } catch (err) {
    showError(parseApiError(err, '更新优先级失败'))
  } finally {
    prioritySavingKeyId.value = null
  }
}

async function quickEditInternalPriority(key: PoolKeyDetail) {
  const raw = window.prompt('设置账号优先级（1-999999，数字越小越优先）', String(key.internal_priority ?? 50))
  if (raw === null) return
  const parsed = Number(raw)
  if (!Number.isFinite(parsed)) {
    showWarning('请输入有效数字')
    return
  }
  await applyInternalPriority(key, parsed)
}

async function finishEditInternalPriority(
  key: PoolKeyDetail,
  event: FocusEvent | KeyboardEvent,
) {
  if (prioritySavingKeyId.value) return
  const target = event.target as HTMLInputElement | null
  const raw = target?.value ?? String(editingPriorityValue.value)
  const parsed = Number(raw)
  const nextPriority = Number.isFinite(parsed) ? parsed : Number(key.internal_priority ?? 50)
  cancelEditInternalPriority()
  await applyInternalPriority(key, nextPriority)
}

function handleEditKey(key: PoolKeyDetail) {
  editingKeyDetail.value = key
  if (key.auth_type === 'oauth') {
    oauthKeyEditDialogOpen.value = true
  } else {
    keyFormDialogOpen.value = true
  }
}

function handleKeyPermissions(key: PoolKeyDetail) {
  editingKeyDetail.value = key
  keyPermissionsDialogOpen.value = true
}

async function handleDialogSaved() {
  editingKeyDetail.value = null
  await loadKeys()
}

function closeKeyFormDialog() {
  keyFormDialogOpen.value = false
  editingKeyDetail.value = null
}

function closeOAuthEditDialog() {
  oauthKeyEditDialogOpen.value = false
  editingKeyDetail.value = null
}

function closeKeyPermissionsDialog() {
  keyPermissionsDialogOpen.value = false
  editingKeyDetail.value = null
}

function getKeyProxyNodeName(key: PoolKeyDetail): string | null {
  if (!key.proxy?.node_id) return null
  const node = proxyNodesStore.nodes.find(n => n.id === key.proxy?.node_id)
  return node ? node.name : `${key.proxy.node_id.slice(0, 8)}...`
}

function handleProxyDesktopPopoverToggle(keyId: string, open: boolean) {
  proxyDesktopPopoverOpenKeyId.value = open ? keyId : null
  if (open) {
    proxyMobilePopoverOpenKeyId.value = null
  }
  if (open) {
    proxyNodesStore.ensureLoaded()
  }
}

function handleProxyMobilePopoverToggle(keyId: string, open: boolean) {
  proxyMobilePopoverOpenKeyId.value = open ? keyId : null
  if (open) {
    proxyDesktopPopoverOpenKeyId.value = null
  }
  if (open) {
    proxyNodesStore.ensureLoaded()
  }
}

async function setKeyProxy(key: PoolKeyDetail, nodeId: string) {
  savingProxyKeyId.value = key.key_id
  try {
    await updateProviderKey(key.key_id, {
      proxy: { node_id: nodeId, enabled: true },
    })
    key.proxy = { node_id: nodeId, enabled: true }
    proxyDesktopPopoverOpenKeyId.value = null
    proxyMobilePopoverOpenKeyId.value = null
    success('代理节点已设置')
  } catch (err) {
    showError(parseApiError(err, '设置代理失败'))
  } finally {
    savingProxyKeyId.value = null
  }
}

async function clearKeyProxy(key: PoolKeyDetail) {
  savingProxyKeyId.value = key.key_id
  try {
    await updateProviderKey(key.key_id, { proxy: null })
    key.proxy = null
    proxyDesktopPopoverOpenKeyId.value = null
    proxyMobilePopoverOpenKeyId.value = null
    success('已清除账号代理，将使用提供商级别代理')
  } catch (err) {
    showError(parseApiError(err, '清除代理失败'))
  } finally {
    savingProxyKeyId.value = null
  }
}

async function handleRecoverKey(key: PoolKeyDetail) {
  if (recoveringHealthKeyId.value) return
  recoveringHealthKeyId.value = key.key_id
  try {
    const result = await recoverKeyHealth(key.key_id)
    success(result.message || 'Key 已恢复')
    await loadKeys()
  } catch (err) {
    showError(parseApiError(err, 'Key恢复失败'))
  } finally {
    recoveringHealthKeyId.value = null
  }
}

async function handleDeleteKey(key: PoolKeyDetail) {
  const confirmed = await confirm({
    title: '删除账号',
    message: `确定要删除账号 "${key.key_name || key.key_id.slice(0, 8)}" 吗？`,
    confirmText: '删除',
    variant: 'destructive',
  })
  if (!confirmed) return

  deletingKeyId.value = key.key_id
  try {
    await deleteEndpointKey(key.key_id)
    success('账号已删除')
    // 乐观更新：直接从本地列表移除，避免等待网络重载
    keyPage.value.keys = keyPage.value.keys.filter(k => k.key_id !== key.key_id)
    keyPage.value.total = Math.max(0, keyPage.value.total - 1)
    // 当前页已空且不是第一页时，自动跳转到前一页
    if (keyPage.value.keys.length === 0 && currentPage.value > 1) {
      currentPage.value--
    }
  } catch (err) {
    showError(parseApiError(err, '删除账号失败'))
  } finally {
    deletingKeyId.value = null
  }
}

async function copyFullKey(key: PoolKeyDetail) {
  const cached = revealedKeys.value.get(key.key_id)
  if (cached) {
    await copyToClipboard(cached)
    return
  }

  try {
    const result = await revealEndpointKey(key.key_id)
    let textToCopy = ''

    if (result.auth_type === 'service_account' && result.auth_config) {
      textToCopy = typeof result.auth_config === 'string'
        ? result.auth_config
        : JSON.stringify(result.auth_config, null, 2)
    } else if (result.auth_type === 'oauth') {
      textToCopy = result.refresh_token || ''
    } else {
      textToCopy = result.api_key || ''
    }

    if (!textToCopy) {
      showError('未获取到可复制内容')
      return
    }

    revealedKeys.value.set(key.key_id, textToCopy)
    await copyToClipboard(textToCopy)
  } catch (err) {
    showError(parseApiError(err, '获取密钥失败'))
  }
}

async function downloadRefreshToken(key: PoolKeyDetail) {
  try {
    const data = await exportKey(key.key_id)
    const providerType = selectedProviderType.value || 'unknown'
    const email = typeof data.email === 'string' ? data.email : ''
    const safeName = (email || key.key_name || key.key_id.slice(0, 8)).replace(/[^a-zA-Z0-9_\-@.]/g, '_')

    const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = `aether_${providerType}_${safeName}.json`
    document.body.appendChild(a)
    a.click()
    document.body.removeChild(a)
    URL.revokeObjectURL(url)
  } catch (err) {
    showError(parseApiError(err, '导出失败'))
  }
}

async function handleRefreshOAuth(key: PoolKeyDetail) {
  if (refreshingOAuthKeyId.value) return

  refreshingOAuthKeyId.value = key.key_id
  try {
    const result = await refreshProviderOAuth(key.key_id)
    const target = keyPage.value.keys.find(k => k.key_id === key.key_id)
    if (target) {
      target.oauth_expires_at = result.expires_at ?? null
      target.oauth_invalid_at = null
      target.oauth_invalid_reason = null
    }
    success('Token 刷新成功')
    await loadKeys()
  } catch (err) {
    showError(parseApiError(err, 'Token 刷新失败'))
    await loadKeys()
  } finally {
    refreshingOAuthKeyId.value = null
  }
}

// --- Actions ---
async function clearCooldown(keyId: string) {
  if (!selectedProviderId.value) return
  try {
    const res = await clearPoolCooldown(selectedProviderId.value, keyId)
    success(res.message)
    await loadKeys()
  } catch (err) {
    showError(parseApiError(err))
  }
}

async function toggleKeyActive(key: PoolKeyDetail) {
  if (togglingKeyId.value) return
  togglingKeyId.value = key.key_id
  try {
    const nextStatus = !key.is_active
    await updateProviderKey(key.key_id, { is_active: nextStatus })
    key.is_active = nextStatus
    if (nextStatus) {
      delete key.scheduling_label
      delete key.scheduling_status
      if (key.scheduling_reason === 'manual_disabled') {
        delete key.scheduling_reason
      }
    } else {
      key.scheduling_label = '禁用'
      key.scheduling_status = 'blocked'
      key.scheduling_reason = 'manual_disabled'
    }
    success(nextStatus ? '账号已启用' : '账号已停用')
    await loadKeys()
  } catch (err) {
    showError(parseApiError(err))
  } finally {
    togglingKeyId.value = null
  }
}

// --- Dialogs ---
const showImportDialog = ref(false)
const showSchedulingDialog = ref(false)
const showAdvancedDialog = ref(false)
const showAccountBatchDialog = ref(false)
const providerProxyMobilePopoverOpen = ref(false)
const providerProxyDesktopPopoverOpen = ref(false)
const savingProviderProxy = ref(false)
const togglingProviderStatus = ref(false)

function openSchedulingDialog() {
  showSchedulingDialog.value = true
}

function getProviderProxyNodeName(): string | null {
  const nodeId = selectedProviderData.value?.proxy?.node_id
  if (!nodeId) return null
  const node = proxyNodesStore.nodes.find(n => n.id === nodeId)
  return node ? node.name : `${nodeId.slice(0, 8)}...`
}

function getProviderProxyButtonTitle(): string {
  const nodeName = getProviderProxyNodeName()
  if (nodeName) return `提供商代理（当前: ${nodeName}）`
  return '提供商代理（未设置）'
}

function closeProviderProxyPopovers(): void {
  providerProxyMobilePopoverOpen.value = false
  providerProxyDesktopPopoverOpen.value = false
}

function handleProviderProxyPopoverToggle(scope: 'mobile' | 'desktop', open: boolean): void {
  if (scope === 'mobile') {
    providerProxyMobilePopoverOpen.value = open
    if (open) {
      providerProxyDesktopPopoverOpen.value = false
    }
  } else {
    providerProxyDesktopPopoverOpen.value = open
    if (open) {
      providerProxyMobilePopoverOpen.value = false
    }
  }
  if (open) {
    proxyNodesStore.ensureLoaded()
    proxyDesktopPopoverOpenKeyId.value = null
    proxyMobilePopoverOpenKeyId.value = null
  }
}

async function setProviderProxy(nodeId: string): Promise<void> {
  const providerId = selectedProviderId.value
  if (!providerId) return
  savingProviderProxy.value = true
  try {
    const updated = await updateProvider(providerId, {
      proxy: { node_id: nodeId, enabled: true },
    })
    if (selectedProviderId.value === providerId) {
      selectedProviderData.value = updated
    }
    closeProviderProxyPopovers()
    success('提供商代理已设置')
  } catch (err) {
    showError(parseApiError(err, '设置提供商代理失败'))
  } finally {
    savingProviderProxy.value = false
  }
}

async function clearProviderProxy(): Promise<void> {
  const providerId = selectedProviderId.value
  if (!providerId) return
  savingProviderProxy.value = true
  try {
    const updated = await updateProvider(providerId, { proxy: null })
    if (selectedProviderId.value === providerId) {
      selectedProviderData.value = updated
    }
    closeProviderProxyPopovers()
    success('提供商代理已清除')
  } catch (err) {
    showError(parseApiError(err, '清除提供商代理失败'))
  } finally {
    savingProviderProxy.value = false
  }
}

function getProviderToggleButtonTitle(): string {
  const active = selectedProviderData.value?.is_active !== false
  return active ? '当前状态：已启用，点击禁用提供商' : '当前状态：已禁用，点击启用提供商'
}

function getProviderToggleButtonClass(): string {
  return ''
}

async function toggleSelectedProviderStatus(): Promise<void> {
  if (togglingProviderStatus.value) return
  const providerId = selectedProviderId.value
  const current = selectedProviderData.value
  if (!providerId || !current) return

  const nextStatus = !current.is_active
  if (!nextStatus) {
    const confirmed = await confirm({
      title: '禁用提供商',
      message: `禁用后该提供商（${current.name}）将不再参与调度，是否继续？`,
      confirmText: '确认禁用',
      variant: 'destructive',
    })
    if (!confirmed) return
  }

  togglingProviderStatus.value = true
  try {
    const updated = await updateProvider(providerId, { is_active: nextStatus })
    if (selectedProviderId.value === providerId) {
      selectedProviderData.value = updated
    }
    success(nextStatus ? '提供商已启用' : '提供商已禁用')
    await loadOverview()
  } catch (err) {
    showError(parseApiError(err, nextStatus ? '启用提供商失败' : '禁用提供商失败'))
  } finally {
    togglingProviderStatus.value = false
  }
}

async function handleAccountBatchChanged(): Promise<void> {
  await Promise.all([loadKeys(), loadOverview()])
}

async function handleAccountDialogSaved() {
  showImportDialog.value = false
  await Promise.all([loadKeys(), loadOverview()])
  // 导入账号后补一次静默额度刷新，避免新账号在列表里暂无额度信息
  await refreshCurrentPageQuotaInBackground({ silent: true })
}

// --- Formatting ---
const COOLDOWN_REASON_MAP: Record<string, string> = {
  rate_limited_429: '429 限流',
  forbidden_403: '403 禁止',
  overloaded_529: '529 过载',
  auth_failed_401: '401 认证失败',
  payment_required_402: '402 欠费',
  server_error_500: '500 错误',
  request_timeout_408: '408 超时',
  conflict_409: '409 冲突',
  locked_423: '423 锁定',
  too_early_425: '425 Too Early',
  bad_gateway_502: '502 网关错误',
  service_unavailable_503: '503 服务不可用',
  gateway_timeout_504: '504 网关超时',
}

function formatCooldownReason(reason: string): string {
  return COOLDOWN_REASON_MAP[reason] || reason
}

type PoolStatusVariant = 'default' | 'secondary' | 'destructive' | 'outline' | 'success' | 'warning' | 'dark'

function getSchedulingStatus(key: PoolKeyDetail): 'available' | 'degraded' | 'blocked' {
  if (getAccountAlertLabel(key)) return 'blocked'

  const status = key.scheduling_status
  if (status === 'available' || status === 'degraded' || status === 'blocked') {
    return status
  }

  if (!key.is_active) return 'blocked'
  if (key.cooldown_reason) return 'blocked'
  if (key.circuit_breaker_open) return 'blocked'
  if (key.cost_limit != null && key.cost_limit > 0 && key.cost_window_usage >= key.cost_limit) return 'blocked'
  if ((key.health_score ?? 1) < 0.8) return 'degraded'
  return 'available'
}

function getSchedulingBadgeLabel(key: PoolKeyDetail): string {
  const accountAlert = getAccountAlertLabel(key)
  if (accountAlert) return accountAlert

  const rawLabel = String(key.scheduling_label || '').trim()
  if (rawLabel) {
    if (rawLabel === '禁用' || rawLabel === '停用') return '禁用'
    return rawLabel
  }

  if (!key.is_active) return '禁用'
  if (key.cooldown_reason) return '冷却'
  if (key.circuit_breaker_open) return '熔断'
  if (key.cost_limit != null && key.cost_limit > 0 && key.cost_window_usage >= key.cost_limit) return '超限'
  if ((key.health_score ?? 1) < 0.5) return '健康低'
  if ((key.health_score ?? 1) < 0.8) return '降级'
  return '可用'
}

function getSchedulingBadgeVariant(key: PoolKeyDetail): PoolStatusVariant {
  if (getAccountAlertLabel(key)) return 'destructive'

  const reason = key.scheduling_reason
  if (reason === 'manual_disabled') return 'secondary'
  if (reason === 'cooldown' || reason === 'circuit_open' || reason === 'cost_exhausted') return 'destructive'
  if (reason === 'cost_soft' || reason === 'cost') return 'warning'
  if (reason === 'health_low' || reason === 'health_degraded' || reason === 'health') return 'warning'
  if (reason === 'available') return 'default'

  const status = getSchedulingStatus(key)
  if (status === 'blocked') return 'destructive'
  if (status === 'degraded') return 'warning'
  return 'default'
}

function getSchedulingTitle(key: PoolKeyDetail): string {
  const accountAlertTitle = getAccountAlertTitle(key)
  if (accountAlertTitle) return accountAlertTitle

  const reasons = key.scheduling_reasons ?? []
  if (reasons.length > 0) {
    return reasons.map((item) => {
      const ttl = item.ttl_seconds && item.ttl_seconds > 0 ? ` (${formatTTL(item.ttl_seconds)})` : ''
      const detail = item.detail ? ` - ${item.detail}` : ''
      return `${item.label}${ttl}${detail}`
    }).join('\n')
  }

  if (key.cooldown_reason) {
    const ttl = key.cooldown_ttl_seconds ? ` (${formatTTL(key.cooldown_ttl_seconds)})` : ''
    return `${formatCooldownReason(key.cooldown_reason)}${ttl}`
  }
  return getSchedulingBadgeLabel(key)
}

function formatTTL(seconds: number): string {
  if (seconds <= 0) return ''
  const m = Math.floor(seconds / 60)
  const s = seconds % 60
  return m > 0 ? `${m}m ${s}s` : `${s}s`
}

function getRowClass(key: PoolKeyDetail): string {
  const status = getSchedulingStatus(key)
  if (!key.is_active || status === 'blocked') return 'bg-muted/50 opacity-60'
  return ''
}

function getHealthScoreColor(score: number): string {
  if (score >= 0.8) return 'text-green-600 dark:text-green-400'
  if (score >= 0.5) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-red-600 dark:text-red-400'
}

function formatOAuthPlanType(planType: string): string {
  const labelMap: Record<string, string> = {
    plus: 'Plus',
    pro: 'Pro',
    free: 'Free',
    paid: 'Paid',
    team: 'Team',
    enterprise: 'Enterprise',
    ultra: 'Ultra',
    'pro+': 'Pro+',
    power: 'Power',
  }
  return labelMap[planType.toLowerCase()] || planType
}

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

function getKeyOAuthExpires(key: PoolKeyDetail) {
  if (key.auth_type !== 'oauth') return null
  if (!key.oauth_expires_at && !key.oauth_invalid_at) return null
  return getOAuthExpiresCountdown(
    key.oauth_expires_at,
    countdownTick.value,
    key.oauth_invalid_at,
    key.oauth_invalid_reason
  )
}

function getOAuthStatusTitle(key: PoolKeyDetail): string {
  const status = getKeyOAuthExpires(key)
  if (!status) return ''
  if (status.isInvalid) {
    const cleaned = status.invalidReason && isAccountLevelBlockReason(status.invalidReason)
      ? cleanAccountBlockReason(status.invalidReason)
      : status.invalidReason
    return cleaned ? `Token 已失效: ${cleaned}` : 'Token 已失效'
  }
  if (status.isExpired) {
    return 'Token 已过期，请重新授权'
  }
  return `Token 剩余有效期: ${status.text}`
}

const _accountAlertCache = new WeakMap<PoolKeyDetail, string | null>()

function getAccountAlertLabel(key: PoolKeyDetail): string | null {
  const cached = _accountAlertCache.get(key)
  if (cached !== undefined) return cached

  let result: string | null = null
  const quotaText = String(key.account_quota || '').trim()
  // 后端 _build_account_quota 返回的确切文本: "账号已封禁" / "访问受限"
  if (quotaText === '账号已封禁' || quotaText === '封禁') result = '账号封禁'
  else if (quotaText === '访问受限') result = '访问受限'
  else if (isAccountLevelBlockReason(key.oauth_invalid_reason)) {
    const reason = String(key.oauth_invalid_reason || '').trim()
    const cleaned = cleanAccountBlockReason(reason)
    result = classifyAccountBlockLabel(cleaned || reason)
  }

  _accountAlertCache.set(key, result)
  return result
}

function getAccountAlertTitle(key: PoolKeyDetail): string {
  const label = getAccountAlertLabel(key)
  if (!label) return ''

  const reason = String(key.oauth_invalid_reason || '').trim()
  if (reason) {
    if (isAccountLevelBlockReason(reason)) {
      const cleaned = cleanAccountBlockReason(reason)
      return cleaned ? `${label}: ${cleaned}` : label
    }
    return `${label}: ${reason}`
  }

  const quotaText = String(key.account_quota || '').trim()
  if (quotaText) return `${label}: ${quotaText}`
  return label
}

function normalizeQuotaLabel(label: string): string {
  const normalized = label.trim()
  if (!normalized) return '额度'
  if (normalized.includes('5H')) return '5H'
  if (normalized.includes('周')) return '周'
  if (normalized.includes('最低剩余')) return '最低'
  if (normalized === '剩余' || normalized.includes('剩余')) return '剩余'
  return normalized
}

function getQuotaProgressLabel(label: string): string {
  if (label === '5H') return '5H'
  if (label === '周') return '周'
  if (label === '最低') return '最低'
  if (label === '剩余') return '剩余'
  return label
}

function getQuotaProgressTooltip(item: QuotaProgressItem): string {
  const detail = item.detail?.trim() || ''
  if ((item.label === '5H' || item.label === '周') && item.resetAtSeconds != null) {
    return `${formatQuotaInlineCountdown(item.resetAtSeconds)} 后重置`
  }
  return detail
}

function getQuotaLabelOrder(label: string): number {
  if (label === '5H') return 0
  if (label === '周') return 1
  if (label === '剩余') return 2
  if (label === '最低') return 3
  return 10
}

function clampPercent(value: number): number {
  if (!Number.isFinite(value)) return 0
  if (value < 0) return 0
  if (value > 100) return 100
  return value
}

function parseQuotaResetRemainingSeconds(detail: string | undefined): number | null {
  if (!detail) return null
  const text = detail.replace(/\s+/g, '')

  if (text.includes('已重置')) return 0
  if (text.includes('即将重置')) return 1
  if (!text.includes('后重置')) return null

  const dayMatch = text.match(/(\d+)天/)
  const hourMatch = text.match(/(\d+)小时/)
  const minuteMatch = text.match(/(\d+)分钟/)
  const secondMatch = text.match(/(\d+)秒/)

  const days = dayMatch ? Number(dayMatch[1]) : 0
  const hours = hourMatch ? Number(hourMatch[1]) : 0
  const minutes = minuteMatch ? Number(minuteMatch[1]) : 0
  const seconds = secondMatch ? Number(secondMatch[1]) : 0
  const total = days * 86400 + hours * 3600 + minutes * 60 + seconds

  if (total <= 0) return 1
  return total
}

function formatQuotaInlineCountdown(resetAtSeconds: number): string {
  // 触发响应式更新，保持倒计时每秒刷新
  void countdownTick.value

  const now = Math.floor(Date.now() / 1000)
  const remain = Math.max(0, Math.floor(resetAtSeconds - now))
  const days = Math.floor(remain / 86400)
  const hours = Math.floor((remain % 86400) / 3600)
  const minutes = Math.floor((remain % 3600) / 60)
  const seconds = remain % 60

  if (days > 0) {
    return `${days}d${String(hours).padStart(2, '0')}:${String(minutes).padStart(2, '0')}`
  }
  if (hours > 0) {
    return `${hours}:${String(minutes).padStart(2, '0')}`
  }
  return `${minutes}:${String(seconds).padStart(2, '0')}`
}

function parseQuotaProgressItems(quotaText: string | null | undefined): QuotaProgressItem[] {
  if (!quotaText) return []

  const segments = quotaText
    .split('|')
    .map(s => s.trim())
    .filter(Boolean)

  const items: QuotaProgressItem[] = []
  for (const segment of segments) {
    const match = segment.match(/^(.*?)(-?\d+(?:\.\d+)?)%\s*(.*)$/)
    if (!match) continue

    const [, rawLabel, rawPercent, rawTail] = match
    const remainingPercent = clampPercent(Number(rawPercent))
    const label = normalizeQuotaLabel(rawLabel)
    const detail = rawTail.trim().replace(/^[()]+|[()]+$/g, '').trim()
    const resetRemainingSeconds = parseQuotaResetRemainingSeconds(detail || undefined)
    const resetAtSeconds = resetRemainingSeconds == null
      ? null
      : Math.floor(Date.now() / 1000) + resetRemainingSeconds

    items.push({
      label,
      remainingPercent,
      detail: detail || undefined,
      resetAtSeconds,
    })
  }

  return items.sort((a, b) => {
    const orderDiff = getQuotaLabelOrder(a.label) - getQuotaLabelOrder(b.label)
    if (orderDiff !== 0) return orderDiff
    return a.label.localeCompare(b.label, 'zh-Hans-CN')
  })
}

function getQuotaRemainingClassByRemaining(remaining: number): string {
  if (remaining <= 10) return 'text-red-600 dark:text-red-400'
  if (remaining <= 30) return 'text-yellow-600 dark:text-yellow-400'
  return 'text-green-600 dark:text-green-400'
}

function getQuotaRemainingBarColorByRemaining(remaining: number): string {
  if (remaining <= 10) return 'bg-red-500 dark:bg-red-400'
  if (remaining <= 30) return 'bg-yellow-500 dark:bg-yellow-400'
  return 'bg-green-500 dark:bg-green-400'
}

function getQuotaTextClass(quotaText: string): string {
  if (quotaText.includes('封禁') || quotaText.includes('受限')) {
    return 'text-[11px] text-destructive leading-4'
  }
  return 'text-[11px] text-foreground/90 leading-4'
}

function formatStatInteger(value: number | null | undefined): string {
  const n = Number(value ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '0'
  return Math.round(n).toLocaleString('en-US')
}

function formatRelativeTime(isoStr: string): string {
  const date = new Date(isoStr)
  const pad = (n: number) => String(n).padStart(2, '0')
  const M = pad(date.getMonth() + 1)
  const D = pad(date.getDate())
  const h = pad(date.getHours())
  const m = pad(date.getMinutes())
  return `${M}-${D} ${h}:${m}`
}

// --- Init ---
onMounted(() => {
  startCountdownTimer()
  void loadSchedulingPresetMetas()
  void loadOverview()
})

onBeforeUnmount(() => {
  if (keysSearchDebounceTimer !== null) {
    clearTimeout(keysSearchDebounceTimer)
    keysSearchDebounceTimer = null
  }
  overviewRequestId += 1
  selectProviderRequestId += 1
  providerDataRequestId += 1
  keysRequestId += 1
})
</script>
