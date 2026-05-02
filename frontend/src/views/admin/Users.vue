<template>
  <div class="space-y-6 pb-8">
    <!-- 用户表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和筛选器 -->
      <div class="px-4 sm:px-6 py-3.5 border-b border-border/60">
        <!-- 移动端：标题行 + 筛选器行 -->
        <div class="flex flex-col gap-3 sm:hidden">
          <div class="flex items-center justify-between">
            <h3 class="text-base font-semibold">
              用户管理
            </h3>
            <div class="flex items-center gap-2">
              <!-- 新增用户按钮 -->
              <Button
                variant="ghost"
                size="icon"
                class="h-8 w-8"
                title="新增用户"
                @click="openCreateDialog"
              >
                <Plus class="w-3.5 h-3.5" />
              </Button>
              <!-- 刷新按钮 -->
              <RefreshButton
                :loading="usersStore.loading"
                @click="refreshUsers"
              />
            </div>
          </div>
          <!-- 筛选器 -->
          <div class="flex items-center gap-2">
            <div class="relative flex-1">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                id="users-search-mobile"
                v-model="searchQuery"
                type="text"
                placeholder="搜索..."
                class="w-full pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60"
              />
            </div>
            <Select
              v-model="filterRole"
            >
              <SelectTrigger class="w-24 h-8 text-xs border-border/60">
                <SelectValue placeholder="角色" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部
                </SelectItem>
                <SelectItem value="admin">
                  管理员
                </SelectItem>
                <SelectItem value="user">
                  用户
                </SelectItem>
              </SelectContent>
            </Select>
            <Select
              v-model="filterStatus"
            >
              <SelectTrigger class="w-20 h-8 text-xs border-border/60">
                <SelectValue placeholder="状态" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">
                  全部
                </SelectItem>
                <SelectItem value="active">
                  活跃
                </SelectItem>
                <SelectItem value="inactive">
                  禁用
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>

        <!-- 桌面端：单行布局 -->
        <div class="hidden sm:flex items-center justify-between gap-4">
          <h3 class="text-base font-semibold">
            用户管理
          </h3>

          <!-- 筛选器和操作按钮 -->
          <div class="flex items-center gap-2">
            <!-- 搜索框 -->
            <div class="relative">
              <Search class="absolute left-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground z-10 pointer-events-none" />
              <Input
                id="users-search"
                v-model="searchQuery"
                type="text"
                placeholder="搜索用户名或邮箱..."
                class="w-48 pl-8 pr-3 h-8 text-sm bg-background/50 border-border/60 focus:border-primary/40 transition-colors"
              />
            </div>

            <!-- 分隔线 -->
            <div class="h-4 w-px bg-border" />

            <!-- 角色筛选 -->
            <div class="xl:hidden">
              <Select
                v-model="filterRole"
              >
                <SelectTrigger class="w-32 h-8 text-xs border-border/60">
                  <SelectValue placeholder="全部角色" />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="all">
                    全部角色
                  </SelectItem>
                  <SelectItem value="admin">
                    管理员
                  </SelectItem>
                  <SelectItem value="user">
                    普通用户
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <!-- 状态筛选 -->
            <div class="xl:hidden">
              <Select
                v-model="filterStatus"
              >
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
                  <SelectItem value="inactive">
                    禁用
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>

            <!-- 分隔线 -->
            <div class="h-4 w-px bg-border" />

            <!-- 新增用户按钮 -->
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="新增用户"
              @click="openCreateDialog"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>

            <!-- 刷新按钮 -->
            <RefreshButton
              :loading="usersStore.loading"
              @click="refreshUsers"
            />
          </div>
        </div>
      </div>

      <!-- 桌面端表格 -->
      <div class="hidden xl:block overflow-x-auto">
        <Table>
          <TableHeader>
            <TableRow class="border-b border-border/60 hover:bg-transparent">
              <SortableTableHead
                class="w-[260px] h-12 font-semibold"
                column-key="role"
                :sortable="false"
                :filter-active="filterRole !== 'all'"
                filter-title="筛选角色"
                filter-content-class="w-40 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
              >
                用户信息
                <template #filter="{ close }">
                  <TableFilterMenu
                    v-model="filterRole"
                    :options="userRoleFilterOptions"
                    @select="close"
                  />
                </template>
              </SortableTableHead>
              <TableHead class="w-[240px] h-12 font-semibold">
                钱包
              </TableHead>
              <TableHead class="w-[170px] h-12 font-semibold">
                统计/限速
              </TableHead>
              <TableHead class="w-[110px] h-12 font-semibold">
                创建时间
              </TableHead>
              <SortableTableHead
                class="w-[180px] h-12 font-semibold"
                column-key="status"
                :sortable="false"
                :filter-active="filterStatus !== 'all'"
                filter-title="筛选状态"
                filter-content-class="w-40 p-1 rounded-2xl border-border bg-card text-foreground shadow-2xl backdrop-blur-xl"
              >
                状态
                <template #filter="{ close }">
                  <TableFilterMenu
                    v-model="filterStatus"
                    :options="userStatusFilterOptions"
                    @select="close"
                  />
                </template>
              </SortableTableHead>
              <TableHead class="w-[220px] h-12 font-semibold text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="user in paginatedUsers"
              :key="user.id"
              class="border-b border-border/40 hover:bg-muted/30 transition-colors"
            >
              <TableCell class="py-4">
                <div class="flex items-center gap-3">
                  <Avatar class="h-10 w-10 ring-2 ring-background shadow-md">
                    <AvatarFallback class="bg-primary text-sm font-bold text-white">
                      {{ user.username.charAt(0).toUpperCase() }}
                    </AvatarFallback>
                  </Avatar>
                  <div class="flex-1 min-w-0">
                    <div class="mb-1 flex items-center gap-1.5">
                      <div
                        class="truncate text-sm font-semibold"
                        :title="user.username"
                      >
                        {{ user.username }}
                      </div>
                      <Badge
                        :variant="user.role === 'admin' ? 'default' : 'secondary'"
                        class="h-5 px-1.5 py-0 text-[10px] font-medium flex-shrink-0"
                      >
                        {{ user.role === 'admin' ? '管理员' : '普通用户' }}
                      </Badge>
                    </div>
                    <div
                      class="truncate text-xs text-muted-foreground"
                      :title="user.email || '-'"
                    >
                      {{ user.email || '-' }}
                    </div>
                  </div>
                </div>
              </TableCell>
              <TableCell class="py-4">
                <div class="space-y-1.5">
                  <div class="flex items-center gap-1 text-[11px] text-muted-foreground">
                    <span>余额：</span>
                    <Badge
                      v-if="isUserUnlimited(user)"
                      variant="secondary"
                      class="h-5 px-1.5 py-0 text-[10px] font-medium"
                    >
                      无限额度
                    </Badge>
                    <span
                      v-else
                      class="text-sm font-semibold tabular-nums"
                      :class="isNegativeWalletValue(getUserWalletTotalBalance(user)) ? 'text-rose-600' : 'text-foreground'"
                    >
                      {{ formatCurrencyValue(getUserWalletTotalBalance(user), '-') }}
                    </span>
                  </div>
                  <div class="flex items-center gap-2 text-[11px] text-muted-foreground flex-wrap">
                    <span>
                      已消费：
                      <span class="font-medium tabular-nums text-foreground">${{ getUserWalletConsumed(user).toFixed(2) }}</span>
                    </span>
                  </div>
                </div>
              </TableCell>
              <TableCell class="py-4">
                <div class="space-y-1 text-xs">
                  <div class="flex items-center text-muted-foreground">
                    <span class="w-14">请求:</span>
                    <span class="font-medium text-foreground">{{ formatNumber(user.request_count) }}</span>
                  </div>
                  <div class="flex items-center text-muted-foreground">
                    <span class="w-14">Tokens:</span>
                    <span class="font-medium text-foreground">{{ formatTokens(user.total_tokens ?? 0) }}</span>
                  </div>
                  <div class="flex items-center text-muted-foreground">
                    <span class="w-14">限速:</span>
                    <Badge
                      v-if="isRateLimitInherited(user.rate_limit) || isRateLimitUnlimited(user.rate_limit)"
                      variant="secondary"
                      class="h-5 px-1.5 py-0 text-[10px] font-medium"
                    >
                      {{ formatRateLimitInheritable(user.rate_limit) }}
                    </Badge>
                    <span
                      v-else
                      class="font-medium text-foreground"
                    >
                      {{ formatRateLimitInheritable(user.rate_limit) }}
                    </span>
                  </div>
                </div>
              </TableCell>
              <TableCell class="py-4 text-xs text-muted-foreground">
                {{ formatDate(user.created_at) }}
              </TableCell>
              <TableCell class="py-4">
                <div class="flex flex-col items-start gap-1.5">
                  <Badge
                    :variant="user.is_active ? 'success' : 'destructive'"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    {{ user.is_active ? '活跃' : '禁用' }}
                  </Badge>
                  <Badge
                    v-if="getUserWallet(user.id)"
                    :variant="walletStatusBadge(getUserWalletStatus(user.id))"
                    class="h-5 px-1.5 py-0 text-[10px] font-medium"
                  >
                    {{ walletStatusLabel(getUserWalletStatus(user.id)) }}
                  </Badge>
                </div>
              </TableCell>
              <TableCell class="py-4">
                <div class="flex justify-center gap-1">
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="编辑用户"
                    @click="editUser(user)"
                  >
                    <SquarePen class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="资金操作"
                    @click="openWalletActionDialog(user)"
                  >
                    <DollarSign class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="API Keys"
                    @click="manageApiKeys(user)"
                  >
                    <Key class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="登录设备"
                    @click="manageUserSessions(user)"
                  >
                    <MonitorSmartphone class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="user.is_active ? '禁用用户' : '启用用户'"
                    @click="toggleUserStatus(user)"
                  >
                    <PauseCircle
                      v-if="user.is_active"
                      class="h-4 w-4"
                    />
                    <PlayCircle
                      v-else
                      class="h-4 w-4"
                    />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="删除用户"
                    @click="deleteUser(user)"
                  >
                    <Trash2 class="h-4 w-4" />
                  </Button>
                </div>
              </TableCell>
            </TableRow>
          </TableBody>
        </Table>
      </div>

      <!-- 移动端卡片列表 -->
      <div class="xl:hidden bg-muted/[0.14] p-3 sm:p-4">
        <div
          v-if="paginatedUsers.length === 0"
          class="rounded-2xl border border-dashed border-border/60 bg-card/70 px-6 py-10 text-center"
        >
          <Avatar class="mx-auto mb-3 h-12 w-12">
            <AvatarFallback class="bg-muted text-base font-semibold text-muted-foreground">
              U
            </AvatarFallback>
          </Avatar>
          <p class="text-sm font-medium text-foreground">
            {{ searchQuery || filterRole !== 'all' || filterStatus !== 'all' ? '未找到匹配的用户' : '暂无用户' }}
          </p>
          <p
            v-if="searchQuery || filterRole !== 'all' || filterStatus !== 'all'"
            class="mt-1 text-xs text-muted-foreground"
          >
            尝试调整筛选条件
          </p>
        </div>

        <div
          v-else
          class="space-y-3.5"
        >
          <div
            v-for="user in paginatedUsers"
            :key="user.id"
            class="rounded-2xl border border-border/60 bg-card/95 p-4 shadow-[0_10px_26px_-22px_hsl(var(--foreground))]"
          >
            <div class="space-y-4">
              <div class="flex items-start gap-3">
                <Avatar class="h-10 w-10 ring-2 ring-background shadow-md flex-shrink-0">
                  <AvatarFallback class="bg-primary text-sm font-bold text-white">
                    {{ user.username.charAt(0).toUpperCase() }}
                  </AvatarFallback>
                </Avatar>
                <div class="min-w-0 flex-1 space-y-1.5">
                  <div class="flex items-center gap-1.5">
                    <div
                      class="truncate text-sm font-semibold text-foreground"
                      :title="user.username"
                    >
                      {{ user.username }}
                    </div>
                    <Badge
                      :variant="user.role === 'admin' ? 'default' : 'secondary'"
                      class="h-5 px-1.5 py-0 text-[10px] font-medium flex-shrink-0"
                    >
                      {{ user.role === 'admin' ? '管理员' : '普通用户' }}
                    </Badge>
                  </div>
                  <div
                    class="truncate text-[11px] text-muted-foreground"
                    :title="user.email || '-'"
                  >
                    {{ user.email || '-' }}
                  </div>
                </div>
              </div>

              <div class="flex flex-wrap items-center gap-1.5">
                <Badge
                  :variant="user.is_active ? 'success' : 'destructive'"
                  class="h-5 px-1.5 py-0 text-[10px] font-medium"
                >
                  {{ user.is_active ? '活跃' : '禁用' }}
                </Badge>
                <Badge
                  v-if="getUserWallet(user.id)"
                  :variant="walletStatusBadge(getUserWalletStatus(user.id))"
                  class="h-5 px-1.5 py-0 text-[10px] font-medium"
                >
                  {{ walletStatusLabel(getUserWalletStatus(user.id)) }}
                </Badge>
                <Badge
                  variant="secondary"
                  class="h-5 px-1.5 py-0 text-[10px] font-medium"
                >
                  {{ formatRateLimitInheritable(user.rate_limit) }}
                </Badge>
              </div>

              <div class="rounded-xl border border-border/60 bg-muted/40 p-3.5">
                <div class="flex items-start justify-between gap-3">
                  <div class="space-y-1">
                    <p class="text-[11px] text-muted-foreground">
                      余额：
                    </p>
                    <Badge
                      v-if="isUserUnlimited(user)"
                      variant="secondary"
                      class="h-5 px-1.5 py-0 text-[10px] font-medium"
                    >
                      无限额度
                    </Badge>
                    <p
                      v-else
                      class="text-base font-semibold tabular-nums leading-none"
                      :class="isNegativeWalletValue(getUserWalletTotalBalance(user)) ? 'text-rose-600' : 'text-foreground'"
                    >
                      {{ formatCurrencyValue(getUserWalletTotalBalance(user), '-') }}
                    </p>
                  </div>
                  <div class="text-right">
                    <p class="text-[11px] text-muted-foreground">
                      已消费：
                    </p>
                    <p class="text-sm font-medium tabular-nums text-foreground">
                      ${{ getUserWalletConsumed(user).toFixed(2) }}
                    </p>
                  </div>
                </div>
              </div>

              <div class="grid grid-cols-2 gap-2.5 text-xs">
                <div class="rounded-lg border border-border/50 bg-background/70 p-2.5">
                  <div class="mb-1 text-muted-foreground">
                    请求次数
                  </div>
                  <div class="font-semibold text-foreground">
                    {{ formatNumber(user.request_count) }}
                  </div>
                </div>
                <div class="rounded-lg border border-border/50 bg-background/70 p-2.5">
                  <div class="mb-1 text-muted-foreground">
                    Tokens
                  </div>
                  <div class="font-semibold text-foreground">
                    {{ formatTokens(user.total_tokens ?? 0) }}
                  </div>
                </div>
              </div>

              <div class="rounded-lg bg-muted/35 p-2.5 text-[11px] text-muted-foreground">
                <div class="flex items-center justify-between gap-2">
                  <span>创建时间</span>
                  <span class="font-medium text-foreground">{{ formatDate(user.created_at) }}</span>
                </div>
              </div>

              <div class="grid grid-cols-2 gap-2 pt-0.5">
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 text-xs"
                  @click="editUser(user)"
                >
                  <SquarePen class="mr-1.5 h-3.5 w-3.5" />
                  编辑
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 text-xs"
                  @click="openWalletActionDialog(user)"
                >
                  <DollarSign class="mr-1.5 h-3.5 w-3.5" />
                  资金
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 text-xs"
                  @click="manageApiKeys(user)"
                >
                  <Key class="mr-1.5 h-3.5 w-3.5" />
                  API Keys
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 text-xs"
                  @click="manageUserSessions(user)"
                >
                  <MonitorSmartphone class="mr-1.5 h-3.5 w-3.5" />
                  设备
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="h-8 text-xs"
                  @click="toggleUserStatus(user)"
                >
                  <PauseCircle
                    v-if="user.is_active"
                    class="mr-1.5 h-3.5 w-3.5"
                  />
                  <PlayCircle
                    v-else
                    class="mr-1.5 h-3.5 w-3.5"
                  />
                  {{ user.is_active ? '禁用' : '启用' }}
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  class="col-span-2 h-8 border-rose-200 text-xs text-rose-600 hover:bg-rose-50 dark:border-rose-900/60 dark:hover:bg-rose-950/40"
                  @click="deleteUser(user)"
                >
                  <Trash2 class="mr-1.5 h-3.5 w-3.5" />
                  删除
                </Button>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 分页控件 -->
      <Pagination
        :current="currentPage"
        :total="filteredUsers.length"
        :page-size="pageSize"
        cache-key="users-page-size"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>

    <!-- 用户表单对话框（创建/编辑共用） -->
    <UserFormDialog
      ref="userFormDialogRef"
      :open="showUserFormDialog"
      :user="editingUser"
      @close="closeUserFormDialog"
      @submit="handleUserFormSubmit"
    />

    <!-- API Keys 管理对话框 -->
    <Dialog
      v-model="showApiKeysDialog"
      size="xl"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-kraft/10 flex-shrink-0">
              <Key class="h-5 w-5 text-kraft" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                管理 API Keys
              </h3>
              <p class="text-xs text-muted-foreground">
                查看和管理用户的 API 密钥
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="max-h-[60vh] overflow-y-auto space-y-3">
        <template v-if="userApiKeys.length > 0">
          <div
            v-for="apiKey in userApiKeys"
            :key="apiKey.id"
            class="rounded-lg border border-border bg-card p-4 hover:border-primary/30 transition-colors"
          >
            <div class="flex items-center justify-between gap-3">
              <!-- 左侧信息 -->
              <div class="flex items-center gap-3 min-w-0 flex-1">
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2 flex-wrap">
                    <span class="font-semibold text-foreground">
                      {{ apiKey.name || '未命名 API Key' }}
                    </span>
                    <Badge
                      :variant="apiKey.is_active ? 'success' : 'secondary'"
                      class="text-xs"
                    >
                      {{ apiKey.is_active ? '活跃' : '禁用' }}
                    </Badge>
                    <Badge
                      v-if="apiKey.is_locked"
                      variant="secondary"
                      class="text-xs"
                    >
                      已锁定
                    </Badge>
                    <Badge
                      v-if="apiKey.is_standalone"
                      variant="default"
                      class="text-xs bg-purple-500"
                    >
                      独立余额
                    </Badge>
                    <Badge
                      variant="secondary"
                      class="text-xs"
                    >
                      {{ formatRateLimitSimple(apiKey.rate_limit) }}
                    </Badge>
                    <Badge
                      variant="secondary"
                      class="text-xs"
                    >
                      {{ formatConcurrentLimitSimple(apiKey.concurrent_limit) }}
                    </Badge>
                  </div>
                  <div class="flex items-center gap-1 mt-0.5">
                    <code class="text-xs font-mono text-muted-foreground">
                      {{ apiKey.key_display || '****' }}
                    </code>
                    <button
                      class="p-0.5 hover:bg-muted rounded transition-colors"
                      title="复制完整密钥"
                      @click="copyFullKey(apiKey)"
                    >
                      <Copy class="w-3 h-3 text-muted-foreground" />
                    </button>
                  </div>
                </div>
              </div>
              <!-- 右侧统计和操作 -->
              <div class="flex items-center gap-4 flex-shrink-0">
                <div class="text-right text-sm">
                  <div class="text-muted-foreground">
                    {{ (apiKey.total_requests || 0).toLocaleString() }} 次
                  </div>
                  <div class="font-semibold text-rose-600">
                    ${{ (apiKey.total_cost_usd || 0).toFixed(4) }}
                  </div>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="编辑"
                  @click="openEditUserApiKeyDialog(apiKey)"
                >
                  <SquarePen class="h-4 w-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  :title="apiKey.is_locked ? '解锁' : '锁定'"
                  @click="toggleLockApiKey(apiKey)"
                >
                  <Lock
                    v-if="apiKey.is_locked"
                    class="h-4 w-4"
                  />
                  <LockOpen
                    v-else
                    class="h-4 w-4"
                  />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-8 w-8"
                  title="删除"
                  @click="deleteApiKey(apiKey)"
                >
                  <Trash2 class="h-4 w-4" />
                </Button>
              </div>
            </div>
          </div>
        </template>
        <div
          v-else
          class="rounded-lg border-2 border-dashed border-muted-foreground/20 bg-muted/20 px-4 py-12 text-center"
        >
          <div class="flex flex-col items-center gap-3">
            <div class="flex h-14 w-14 items-center justify-center rounded-full bg-muted">
              <Key class="h-6 w-6 text-muted-foreground/50" />
            </div>
            <div>
              <p class="mb-1 text-base font-semibold text-foreground">
                暂无 API Keys
              </p>
              <p class="text-sm text-muted-foreground">
                点击下方按钮创建
              </p>
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-10 px-5"
          @click="showApiKeysDialog = false"
        >
          取消
        </Button>
        <Button
          class="h-10 px-5"
          :disabled="creatingApiKey"
          @click="openCreateUserApiKeyDialog"
        >
          {{ creatingApiKey ? '创建中...' : '创建' }}
        </Button>
      </template>
    </Dialog>

    <Dialog
      v-model="showUserApiKeyFormDialog"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-kraft/10 flex-shrink-0">
              <Key class="h-5 w-5 text-kraft" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                {{ editingUserApiKey ? '编辑 API Key' : '创建 API Key' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                {{ editingUserApiKey ? '更新用户 API Key 的名称、速率限制和并发限制' : '为用户创建新的 API Key' }}
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label
            for="admin-user-key-name"
            class="text-sm font-medium"
          >密钥名称</Label>
          <Input
            id="admin-user-key-name"
            v-model="userApiKeyForm.name"
            class="h-10"
            placeholder="例如：生产环境 Key"
          />
        </div>
        <div class="space-y-2">
          <Label
            for="admin-user-key-rate-limit"
            class="text-sm font-medium"
          >速率限制 (请求/分钟)</Label>
          <Input
            id="admin-user-key-rate-limit"
            :model-value="userApiKeyForm.rate_limit ?? ''"
            type="number"
            min="0"
            max="10000"
            class="h-10"
            placeholder="留空不限"
            @update:model-value="(v) => userApiKeyForm.rate_limit = parseNumberInput(v, { min: 0, max: 10000 })"
          />
          <p class="text-xs text-muted-foreground">
            留空表示不限制
          </p>
        </div>
        <div class="space-y-2">
          <Label
            for="admin-user-key-concurrent-limit"
            class="text-sm font-medium"
          >并发限制</Label>
          <Input
            id="admin-user-key-concurrent-limit"
            :model-value="userApiKeyForm.concurrent_limit ?? ''"
            type="number"
            min="0"
            max="10000"
            class="h-10"
            placeholder="0 = 不限并发"
            @update:model-value="(v) => userApiKeyForm.concurrent_limit = parseNumberInput(v, { min: 0, max: 10000 })"
          />
          <p class="text-xs text-muted-foreground">
            {{ editingUserApiKey ? '留空表示保持当前值，填 0 表示不限并发' : '留空表示不限并发，填 0 也表示不限并发' }}
          </p>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-10 px-5"
          @click="closeUserApiKeyFormDialog"
        >
          取消
        </Button>
        <Button
          class="h-10 px-5"
          :disabled="creatingApiKey"
          @click="submitUserApiKeyForm"
        >
          {{ creatingApiKey ? (editingUserApiKey ? '保存中...' : '创建中...') : (editingUserApiKey ? '保存' : '创建') }}
        </Button>
      </template>
    </Dialog>

    <Dialog
      v-model="showUserSessionsDialog"
      size="xl"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
              <MonitorSmartphone class="h-5 w-5 text-primary" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                登录设备
              </h3>
              <p class="text-xs text-muted-foreground">
                查看并强制下线该用户的设备会话
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="max-h-[60vh] overflow-y-auto space-y-3">
        <div
          v-if="loadingUserSessions"
          class="rounded-lg border border-dashed border-border/60 bg-muted/20 px-4 py-10 text-center text-sm text-muted-foreground"
        >
          正在加载设备会话...
        </div>
        <div
          v-else-if="userSessions.length === 0"
          class="rounded-lg border border-dashed border-border/60 bg-muted/20 px-4 py-10 text-center text-sm text-muted-foreground"
        >
          暂无在线设备
        </div>
        <div
          v-else
          class="space-y-3"
        >
          <div
            v-for="session in userSessions"
            :key="session.id"
            class="rounded-lg border border-border bg-card p-4 hover:border-primary/30 transition-colors"
          >
            <div class="flex items-center justify-between gap-3">
              <div class="min-w-0 flex-1">
                <div class="font-semibold text-foreground">
                  {{ session.device_label }}
                </div>
                <div class="mt-1 text-xs text-muted-foreground">
                  {{ formatSessionMeta(session) }}
                </div>
                <div class="mt-1 text-xs text-muted-foreground">
                  最近活跃 {{ formatDate(session.last_seen_at || session.created_at) }}
                  <span v-if="session.ip_address"> · IP {{ session.ip_address }}</span>
                </div>
              </div>
              <Button
                variant="outline"
                size="sm"
                :disabled="sessionDialogActionLoading === session.id"
                @click="revokeSelectedUserSession(session.id)"
              >
                {{ sessionDialogActionLoading === session.id ? '处理中...' : '强制下线' }}
              </Button>
            </div>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-10 px-5"
          @click="showUserSessionsDialog = false"
        >
          关闭
        </Button>
        <Button
          class="h-10 px-5"
          :disabled="loadingUserSessions || userSessions.length === 0 || sessionDialogActionLoading === 'all'"
          @click="revokeAllSelectedUserSessions"
        >
          {{ sessionDialogActionLoading === 'all' ? '处理中...' : '全部下线' }}
        </Button>
      </template>
    </Dialog>

    <WalletOpsDrawer
      :open="showWalletActionDialogState"
      :wallet="walletActionTarget?.wallet || null"
      :owner-name="walletActionTarget?.user.username || ''"
      :owner-subtitle="walletActionTarget?.user.email || '未设置邮箱'"
      context-label="用户钱包"
      accent="emerald"
      @close="closeWalletActionDrawer"
      @changed="handleWalletDrawerChanged"
    />

    <!-- 新 API Key 显示对话框 -->
    <Dialog
      v-model="showNewApiKeyDialog"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-emerald-100 dark:bg-emerald-900/30 flex-shrink-0">
              <CheckCircle class="h-5 w-5 text-emerald-600 dark:text-emerald-400" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                创建成功
              </h3>
              <p class="text-xs text-muted-foreground">
                请妥善保管, 切勿泄露给他人.
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label class="text-sm font-medium">API Key</Label>
          <div class="flex items-center gap-2">
            <Input
              ref="apiKeyInput"
              type="text"
              :value="newApiKey"
              readonly
              class="flex-1 font-mono text-sm bg-muted/50 h-11"
              @click="selectApiKey"
            />
            <Button
              class="h-11"
              @click="copyApiKey"
            >
              复制
            </Button>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          class="h-10 px-5"
          @click="closeNewApiKeyDialog"
        >
          确定
        </Button>
      </template>
    </Dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import { useUsersStore } from '@/stores/users'
import type { User, ApiKey, UserSession } from '@/api/users'
import { formatSessionMeta } from '@/types/session'
import { adminWalletApi, type AdminWallet } from '@/api/admin-wallets'
import { useToast } from '@/composables/useToast'
import { useConfirm } from '@/composables/useConfirm'
import { useClipboard } from '@/composables/useClipboard'
import { adminApi } from '@/api/admin'
import { walletStatusBadge, walletStatusLabel } from '@/utils/walletDisplay'

// UI 组件
import {
  Dialog,
  Card,
  Button,
  Badge,
  Input,
  Label,
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
  SortableTableHead,
  TableFilterMenu,
  TableCell,
  Avatar,
  AvatarFallback,
  Pagination,
  RefreshButton
} from '@/components/ui'

import {
  Plus,
  SquarePen,
  Key,
  PauseCircle,
  PlayCircle,
  DollarSign,
  Trash2,
  Copy,
  Search,
  CheckCircle,
  Lock,
  LockOpen,
  MonitorSmartphone
} from 'lucide-vue-next'

// 功能组件
import UserFormDialog, { type UserFormData } from '@/features/users/components/UserFormDialog.vue'
import WalletOpsDrawer from '@/features/wallet/components/WalletOpsDrawer.vue'
import { parseApiError } from '@/utils/errorParser'
import { formatTokens, formatRateLimitInheritable, formatRateLimitSimple, isRateLimitInherited, isRateLimitUnlimited } from '@/utils/format'
import { parseNumberInput } from '@/utils/form'
import { log } from '@/utils/logger'

const { success, error } = useToast()
const { confirmDanger } = useConfirm()
const { copyToClipboard } = useClipboard()
const usersStore = useUsersStore()

// 用户表单对话框状态
const showUserFormDialog = ref(false)
const editingUser = ref<UserFormData | null>(null)
const userFormDialogRef = ref<InstanceType<typeof UserFormDialog>>()

// API Keys 对话框状态
const showApiKeysDialog = ref(false)
const showUserSessionsDialog = ref(false)
const showNewApiKeyDialog = ref(false)
const showUserApiKeyFormDialog = ref(false)
const selectedUser = ref<User | null>(null)
const userApiKeys = ref<ApiKey[]>([])
const userSessions = ref<UserSession[]>([])
const newApiKey = ref('')
const creatingApiKey = ref(false)
const loadingUserSessions = ref(false)
const sessionDialogActionLoading = ref<string | null>(null)
const apiKeyInput = ref<HTMLInputElement>()
const editingUserApiKey = ref<ApiKey | null>(null)
const userApiKeyForm = ref({
  name: '',
  rate_limit: undefined as number | undefined,
  concurrent_limit: undefined as number | undefined,
})

// 用户统计
const userWalletMap = ref<Record<string, AdminWallet>>({})

const showWalletActionDialogState = ref(false)
const walletActionTarget = ref<{ user: User; wallet: AdminWallet } | null>(null)

const searchQuery = ref('')
const filterRole = ref('all')
const filterStatus = ref('all')
const userRoleFilterOptions = [
  { value: 'all', label: '全部角色' },
  { value: 'admin', label: '管理员' },
  { value: 'user', label: '普通用户' },
]
const userStatusFilterOptions = [
  { value: 'all', label: '全部状态' },
  { value: 'active', label: '活跃' },
  { value: 'inactive', label: '禁用' },
]

const currentPage = ref(1)
const pageSize = ref(20)
const USERS_PAGE_CACHE_TTL_MS = 10 * 1000
const USER_WALLETS_CACHE_TTL_MS = 10 * 1000
let userWalletsRequestId = 0

const filteredUsers = computed(() => {
  let filtered = [...usersStore.users]

  // 先排序：管理员优先，然后按创建时间倒序
  filtered.sort((a, b) => {
    // 管理员优先
    if (a.role === 'admin' && b.role !== 'admin') return -1
    if (a.role !== 'admin' && b.role === 'admin') return 1
    // 同角色按创建时间倒序（新用户在前）
    return new Date(b.created_at).getTime() - new Date(a.created_at).getTime()
  })

  // 搜索（支持空格分隔的多关键词 AND 搜索）
  if (searchQuery.value) {
    const keywords = searchQuery.value.toLowerCase().split(/\s+/).filter(k => k.length > 0)
    filtered = filtered.filter(u => {
      const searchableText = `${u.username} ${u.email || ''}`.toLowerCase()
      return keywords.every(keyword => searchableText.includes(keyword))
    })
  }

  if (filterRole.value !== 'all') {
    filtered = filtered.filter(u => u.role === filterRole.value)
  }

  if (filterStatus.value !== 'all') {
    filtered = filtered.filter(u =>
      filterStatus.value === 'active' ? u.is_active : !u.is_active
    )
  }

  return filtered
})

const paginatedUsers = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return filteredUsers.value.slice(start, start + pageSize.value)
})

// Watch filter changes and reset to first page
watch([searchQuery, filterRole, filterStatus], () => {
  currentPage.value = 1
})

onMounted(() => {
  void refreshUsers({ preferCache: true })
})

async function refreshUsers(options: { preferCache?: boolean } = {}) {
  const cacheTtlMs = options.preferCache ? USERS_PAGE_CACHE_TTL_MS : 0
  await usersStore.fetchUsers({ cacheTtlMs })
  void loadUserWallets({
    cacheTtlMs: options.preferCache ? USER_WALLETS_CACHE_TTL_MS : 0,
  })
}

function formatDate(dateString: string) {
  return new Date(dateString).toLocaleDateString('zh-CN')
}

async function loadUserWallets(options: { cacheTtlMs?: number } = {}) {
  const requestId = ++userWalletsRequestId
  try {
    const wallets = await adminWalletApi.listAllWallets(
      { owner_type: 'user' },
      { cacheTtlMs: options.cacheTtlMs ?? 0 },
    )
    if (requestId !== userWalletsRequestId) return
    userWalletMap.value = wallets
      .filter((wallet) => !!wallet.user_id)
      .reduce<Record<string, AdminWallet>>((acc, wallet) => {
        acc[wallet.user_id as string] = wallet
        return acc
      }, {})
  } catch (err) {
    if (requestId !== userWalletsRequestId) return
    log.error('加载用户钱包失败:', err)
  }
}

function formatNumber(value?: number | null): string {
  const numericValue = typeof value === 'number' && Number.isFinite(value) ? value : 0
  return numericValue.toLocaleString()
}

function getUserWallet(userId: string): AdminWallet | null {
  return userWalletMap.value[userId] || null
}

function isUserUnlimited(user: User): boolean {
  const wallet = getUserWallet(user.id)
  if (wallet?.limit_mode === 'unlimited' || wallet?.unlimited === true) {
    return true
  }
  return Boolean(user.unlimited)
}

function getUserWalletTotalBalance(user: User): number | null {
  if (isUserUnlimited(user)) {
    return null
  }
  const wallet = getUserWallet(user.id)
  if (!wallet) {
    return null
  }
  return wallet.balance
}

function getUserWalletConsumed(user: User): number {
  return getUserWallet(user.id)?.total_consumed ?? 0
}

function getUserWalletStatus(userId: string): string | null {
  return getUserWallet(userId)?.status ?? null
}

function formatCurrencyValue(value: number | null, nullLabel = '-'): string {
  if (value == null) {
    return nullLabel
  }
  return `$${value.toFixed(2)}`
}

function formatConcurrentLimitSimple(concurrentLimit?: number | null): string {
  if (concurrentLimit == null || concurrentLimit === 0) {
    return '不限并发'
  }
  return `${concurrentLimit} 并发`
}

function isNegativeWalletValue(value: number | null): boolean {
  return typeof value === 'number' && value < 0
}

async function toggleUserStatus(user: User) {
  const action = user.is_active ? '禁用' : '启用'
  const confirmed = await confirmDanger(
    `确定要${action}用户 ${user.username} 吗？`,
    `${action}用户`,
    action
  )

  if (!confirmed) return

  try {
    await usersStore.updateUser(user.id, { is_active: !user.is_active })
    success(`用户已${action}`)
  } catch (err: unknown) {
    error(parseApiError(err, '未知错误'), `${action}用户失败`)
  }
}

// ========== 用户表单对话框方法 ==========

function openCreateDialog() {
  editingUser.value = null
  showUserFormDialog.value = true
}

function editUser(user: User) {
  // 创建数组副本，避免与 store 数据共享引用
  editingUser.value = {
    id: user.id,
    username: user.username,
    email: user.email,
    unlimited: user.unlimited,
    role: user.role,
    is_active: user.is_active,
    allowed_providers: user.allowed_providers == null ? null : [...user.allowed_providers],
    allowed_api_formats: user.allowed_api_formats == null ? null : [...user.allowed_api_formats],
    allowed_models: user.allowed_models == null ? null : [...user.allowed_models],
    rate_limit: user.rate_limit ?? null
  }
  showUserFormDialog.value = true
}

function closeUserFormDialog() {
  showUserFormDialog.value = false
  editingUser.value = null
}

async function handleUserFormSubmit(data: UserFormData & { password?: string; unlimited?: boolean }) {
  userFormDialogRef.value?.setSaving(true)
  try {
    if (data.id) {
      // 更新用户
      const updateData: Record<string, unknown> = {
        username: data.username,
        email: data.email || undefined,
        unlimited: data.unlimited,
        role: data.role,
        allowed_providers: data.allowed_providers,
        allowed_api_formats: data.allowed_api_formats,
        allowed_models: data.allowed_models,
        rate_limit: data.rate_limit ?? null
      }
      if (data.password) {
        updateData.password = data.password
      }
      await usersStore.updateUser(data.id, updateData)
      await loadUserWallets()
      success('用户信息已更新')
    } else {
      // 创建用户
      const newUser = await usersStore.createUser({
        username: data.username,
        password: data.password ?? '',
        email: data.email || undefined,
        initial_gift_usd: data.initial_gift_usd,
        unlimited: data.unlimited,
        role: data.role,
        allowed_providers: data.allowed_providers,
        allowed_api_formats: data.allowed_api_formats,
        allowed_models: data.allowed_models,
        rate_limit: data.rate_limit ?? null
      })
      // 如果创建时指定为禁用，则更新状态
      if (data.is_active === false && newUser) {
        await usersStore.updateUser(newUser.id, { is_active: false })
      }
      await loadUserWallets()
      success('用户创建成功')
    }
    closeUserFormDialog()
  } catch (err: unknown) {
    const title = data.id ? '更新用户失败' : '创建用户失败'
    error(parseApiError(err, '未知错误'), title)
  } finally {
    userFormDialogRef.value?.setSaving(false)
  }
}

async function manageApiKeys(user: User) {
  selectedUser.value = user
  showApiKeysDialog.value = true
  await loadUserApiKeys(user.id)
}

async function manageUserSessions(user: User) {
  selectedUser.value = user
  showUserSessionsDialog.value = true
  loadingUserSessions.value = true
  try {
    userSessions.value = await usersStore.getUserSessions(user.id)
  } catch (err) {
    error(parseApiError(err, '加载用户设备会话失败'))
  } finally {
    loadingUserSessions.value = false
  }
}

async function loadUserApiKeys(userId: string) {
  try {
    userApiKeys.value = await usersStore.getUserApiKeys(userId)
  } catch (err) {
    log.error('加载API Keys失败:', err)
    userApiKeys.value = []
  }
}

function openCreateUserApiKeyDialog() {
  userApiKeyForm.value = {
    name: `Key-${new Date().toISOString().split('T')[0]}`,
    rate_limit: undefined,
    concurrent_limit: undefined,
  }
  editingUserApiKey.value = null
  showUserApiKeyFormDialog.value = true
}

function openEditUserApiKeyDialog(apiKey: ApiKey) {
  editingUserApiKey.value = apiKey
  userApiKeyForm.value = {
    name: apiKey.name || '',
    rate_limit: apiKey.rate_limit ?? undefined,
    concurrent_limit: apiKey.concurrent_limit ?? undefined,
  }
  showUserApiKeyFormDialog.value = true
}

function closeUserApiKeyFormDialog() {
  showUserApiKeyFormDialog.value = false
  editingUserApiKey.value = null
  userApiKeyForm.value = {
    name: '',
    rate_limit: undefined,
    concurrent_limit: undefined,
  }
}

async function submitUserApiKeyForm() {
  if (!selectedUser.value) return
  if (!userApiKeyForm.value.name.trim()) {
    error('请输入密钥名称', editingUserApiKey.value ? '更新 API Key 失败' : '创建 API Key 失败')
    return
  }

  creatingApiKey.value = true
  try {
    if (editingUserApiKey.value) {
      await usersStore.updateApiKey(selectedUser.value.id, editingUserApiKey.value.id, {
        name: userApiKeyForm.value.name,
        rate_limit: userApiKeyForm.value.rate_limit ?? 0,
        concurrent_limit: userApiKeyForm.value.concurrent_limit,
      })
      success('API Key已更新')
    } else {
      const response = await usersStore.createApiKey(selectedUser.value.id, {
        name: userApiKeyForm.value.name,
        rate_limit: userApiKeyForm.value.rate_limit ?? 0,
        concurrent_limit: userApiKeyForm.value.concurrent_limit,
      })
      newApiKey.value = response.key || ''
      showNewApiKeyDialog.value = true
      success('API Key创建成功')
    }
    await loadUserApiKeys(selectedUser.value.id)
    closeUserApiKeyFormDialog()
  } catch (err: unknown) {
    error(parseApiError(err, '未知错误'), editingUserApiKey.value ? '更新 API Key 失败' : '创建 API Key 失败')
  } finally {
    creatingApiKey.value = false
  }
}

async function revokeSelectedUserSession(sessionId: string) {
  if (!selectedUser.value) return
  sessionDialogActionLoading.value = sessionId
  try {
    await usersStore.revokeUserSession(selectedUser.value.id, sessionId)
    userSessions.value = userSessions.value.filter((session) => session.id !== sessionId)
    success('设备已强制下线')
  } catch (err) {
    error(parseApiError(err, '强制下线失败'))
  } finally {
    sessionDialogActionLoading.value = null
  }
}

async function revokeAllSelectedUserSessions() {
  if (!selectedUser.value) return
  sessionDialogActionLoading.value = 'all'
  try {
    const result = await usersStore.revokeAllUserSessions(selectedUser.value.id)
    userSessions.value = []
    success(result.revoked_count > 0 ? `已强制下线 ${result.revoked_count} 个设备` : '没有可下线的设备')
  } catch (err) {
    error(parseApiError(err, '强制下线全部设备失败'))
  } finally {
    sessionDialogActionLoading.value = null
  }
}

function selectApiKey() {
  apiKeyInput.value?.select()
}

async function copyApiKey() {
  await copyToClipboard(newApiKey.value)
}

async function closeNewApiKeyDialog() {
  showNewApiKeyDialog.value = false
  newApiKey.value = ''
}

async function deleteApiKey(apiKey: ApiKey) {
  const confirmed = await confirmDanger(
    `确定要删除这个API Key吗？\n\n${apiKey.key_display || '****'}\n\n此操作无法撤销。`,
    '删除 API Key'
  )

  if (!confirmed) return

  try {
    await usersStore.deleteApiKey(selectedUser.value.id, apiKey.id)
    await loadUserApiKeys(selectedUser.value.id)
    success('API Key已删除')
  } catch (err: unknown) {
    error(parseApiError(err, '未知错误'), '删除 API Key 失败')
  }
}

async function toggleLockApiKey(apiKey: ApiKey) {
  if (!selectedUser.value) return
  try {
    const response = await adminApi.toggleUserApiKeyLock(selectedUser.value.id, apiKey.id)
    // 更新本地状态
    const index = userApiKeys.value.findIndex(k => k.id === apiKey.id)
    if (index !== -1) {
      userApiKeys.value[index].is_locked = response.is_locked
    }
    success(response.message)
  } catch (err: unknown) {
    log.error('切换密钥锁定状态失败:', err)
    error(parseApiError(err, '操作失败'), '锁定/解锁失败')
  }
}

async function copyFullKey(apiKey: ApiKey) {
  if (!selectedUser.value) return
  try {
    const response = await usersStore.getFullApiKey(selectedUser.value.id, apiKey.id)
    await copyToClipboard(response.key)
  } catch (err: unknown) {
    log.error('复制密钥失败:', err)
    error(parseApiError(err, '未知错误'), '复制密钥失败')
  }
}

function openWalletActionDialog(user: User) {
  const wallet = getUserWallet(user.id)
  if (!wallet) {
    error('该用户的钱包尚未初始化，暂时无法进行资金操作')
    return
  }

  walletActionTarget.value = {
    user,
    wallet,
  }
  showWalletActionDialogState.value = true
}

function closeWalletActionDrawer() {
  showWalletActionDialogState.value = false
}

async function handleWalletDrawerChanged() {
  await loadUserWallets()
  if (!walletActionTarget.value) return
  const latestWallet = getUserWallet(walletActionTarget.value.user.id)
  if (latestWallet) {
    walletActionTarget.value.wallet = latestWallet
  }
}

async function deleteUser(user: User) {
  const confirmed = await confirmDanger(
    `确定要删除用户 ${user.username} 吗？\n\n此操作将删除：\n• 用户账户\n• 所有API密钥\n• 所有使用记录\n\n此操作无法撤销！`,
    '删除用户'
  )

  if (!confirmed) return

  try {
    await usersStore.deleteUser(user.id)
    success('用户已删除')
  } catch (err: unknown) {
    error(parseApiError(err, '未知错误'), '删除用户失败')
  }
}
</script>
