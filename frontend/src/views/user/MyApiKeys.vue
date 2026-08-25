<template>
  <div class="space-y-6 pb-8">
    <!-- API Keys 表格 -->
    <Card
      variant="default"
      class="overflow-hidden"
    >
      <!-- 标题和操作栏 -->
      <div class="px-4 sm:px-6 py-3 sm:py-3.5 border-b border-border/60">
        <div class="flex flex-col sm:flex-row sm:items-center sm:justify-between gap-3 sm:gap-4">
          <h3 class="text-sm sm:text-base font-semibold shrink-0">
            我的 API Keys
          </h3>

          <!-- 操作按钮 -->
          <div class="flex items-center gap-2">
            <!-- 新增 API Key 按钮 -->
            <Button
              variant="ghost"
              size="icon"
              class="h-8 w-8"
              title="创建新 API Key"
              @click="openCreateApiKeyDialog"
            >
              <Plus class="w-3.5 h-3.5" />
            </Button>

            <!-- 刷新按钮 -->
            <RefreshButton
              :loading="loading"
              @click="loadApiKeys"
            />
          </div>
        </div>
      </div>

      <!-- 加载状态 -->
      <div
        v-if="loading"
        class="flex items-center justify-center py-12"
      >
        <LoadingState message="加载中..." />
      </div>

      <!-- 空状态 -->
      <div
        v-else-if="apiKeys.length === 0"
        class="flex items-center justify-center py-12"
      >
        <EmptyState
          title="暂无 API 密钥"
          description="创建你的第一个 API 密钥开始使用"
          :icon="Key"
        >
          <template #actions>
            <Button
              size="lg"
              class="shadow-lg shadow-primary/20"
              @click="openCreateApiKeyDialog"
            >
              <Plus class="mr-2 h-4 w-4" />
              创建新 API Key
            </Button>
          </template>
        </EmptyState>
      </div>

      <!-- 桌面端表格 -->
      <div
        v-else
        class="hidden md:block overflow-x-auto"
      >
        <Table>
          <TableHeader>
            <TableRow class="border-b border-border/60 hover:bg-transparent">
              <TableHead class="min-w-[200px] h-12 font-semibold">
                密钥名称
              </TableHead>
              <TableHead class="min-w-[160px] h-12 font-semibold">
                密钥
              </TableHead>
              <TableHead class="min-w-[140px] h-12 font-semibold">
                费用(USD)
                <span class="block text-[10px] font-normal text-muted-foreground">
                  扣款倍率 {{ sellRateMultiplier }}x
                </span>
              </TableHead>
              <TableHead class="min-w-[100px] h-12 font-semibold">
                请求次数
              </TableHead>
              <TableHead class="min-w-[70px] h-12 font-semibold text-center">
                状态
              </TableHead>
              <TableHead class="min-w-[100px] h-12 font-semibold">
                最后使用
              </TableHead>
              <TableHead class="min-w-[80px] h-12 font-semibold text-center">
                操作
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            <TableRow
              v-for="apiKey in paginatedApiKeys"
              :key="apiKey.id"
              class="border-b border-border/40 hover:bg-muted/30 transition-colors"
            >
              <!-- 密钥名称 -->
              <TableCell class="py-4">
                <div class="flex-1 min-w-0">
                  <div
                    class="text-sm font-semibold truncate"
                    :title="apiKey.name"
                  >
                    {{ apiKey.name }}
                  </div>
                  <div class="text-xs text-muted-foreground mt-0.5">
                    创建于 {{ formatDate(apiKey.created_at) }}
                  </div>
                  <div class="text-xs text-muted-foreground mt-0.5 truncate">
                    IP 限制：{{ formatIpRules(apiKey.ip_rules) }}
                  </div>
                </div>
              </TableCell>

              <!-- 密钥显示 -->
              <TableCell class="py-4">
                <div class="flex items-center gap-1.5">
                  <code class="text-xs font-mono text-muted-foreground bg-muted/30 px-2 py-1 rounded">
                    {{ apiKey.key_display || 'sk-••••••••' }}
                  </code>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-6 w-6"
                    title="复制完整密钥"
                    @click="copyApiKey(apiKey)"
                  >
                    <Copy class="h-3.5 w-3.5" />
                  </Button>
                </div>
              </TableCell>

              <!-- 费用 -->
              <TableCell class="py-4">
                <div class="flex flex-col">
                  <span class="text-sm font-semibold text-amber-600 dark:text-amber-500">
                    ${{ keyActualCost(apiKey).toFixed(4) }}
                  </span>
                  <span class="text-[10px] text-muted-foreground">
                    标价 ${{ keyCatalogCost(apiKey).toFixed(4) }} · {{ sellRateMultiplier }}x
                  </span>
                </div>
              </TableCell>

              <!-- 请求次数 -->
              <TableCell class="py-4">
                <div class="flex items-center gap-1.5">
                  <Activity class="h-3.5 w-3.5 text-muted-foreground" />
                  <span class="text-sm font-medium text-foreground">
                    {{ formatNumber(apiKey.total_requests || 0) }}
                  </span>
                </div>
              </TableCell>

              <!-- 状态 -->
              <TableCell class="py-4 text-center">
                <div class="flex flex-col items-center gap-1">
                  <Badge
                    :variant="apiKey.is_active ? 'success' : 'secondary'"
                    class="h-5 px-2 py-0 text-[10px] font-medium"
                  >
                    {{ apiKey.is_active ? '活跃' : '禁用' }}
                  </Badge>
                  <Badge
                    v-if="apiKey.is_locked"
                    variant="warning"
                    class="h-5 px-2 py-0 text-[10px] font-medium"
                  >
                    已锁定
                  </Badge>
                  <Badge
                    variant="secondary"
                    class="h-5 px-2 py-0 text-[10px] font-medium"
                  >
                    {{ formatRateLimitSimple(apiKey.rate_limit) }}
                  </Badge>
                  <Badge
                    variant="secondary"
                    class="h-5 px-2 py-0 text-[10px] font-medium"
                  >
                    {{ formatConcurrentLimitSimple(apiKey.concurrent_limit) }}
                  </Badge>
                </div>
              </TableCell>

              <!-- 最后使用时间 -->
              <TableCell class="py-4 text-sm text-muted-foreground">
                {{ apiKey.last_used_at ? formatRelativeTime(apiKey.last_used_at) : '从未使用' }}
              </TableCell>

              <!-- 操作按钮 -->
              <TableCell class="py-4">
                <div class="flex justify-center gap-1">
                  <Button
                    :data-testid="`ccswitch-open-${apiKey.id}`"
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="导入到 CC Switch"
                    @click="openCcSwitchImportDialog(apiKey)"
                  >
                    <Download class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    title="一键安装并配置 CLI"
                    @click="openInstallDialog(apiKey)"
                  >
                    <Terminal class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="apiKey.is_locked ? '已锁定' : '编辑'"
                    :disabled="apiKey.is_locked"
                    @click="openEditApiKeyDialog(apiKey)"
                  >
                    <SquarePen class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="apiKey.is_locked ? '已锁定' : (apiKey.is_active ? '禁用' : '启用')"
                    :disabled="apiKey.is_locked"
                    @click="toggleApiKey(apiKey)"
                  >
                    <Power class="h-4 w-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-8 w-8"
                    :title="apiKey.is_locked ? '已锁定' : '删除'"
                    :disabled="apiKey.is_locked"
                    @click="confirmDelete(apiKey)"
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
      <div
        v-if="!loading && apiKeys.length > 0"
        class="md:hidden space-y-3 p-4"
      >
        <Card
          v-for="apiKey in paginatedApiKeys"
          :key="apiKey.id"
          variant="default"
          class="group hover:shadow-md hover:border-primary/30 transition-all duration-200"
        >
          <div class="p-4">
            <!-- 第一行：名称、状态、操作 -->
            <div class="flex items-center justify-between mb-2">
              <div class="flex items-center gap-2 min-w-0 flex-1">
                <h3 class="text-sm font-semibold text-foreground truncate">
                  {{ apiKey.name }}
                </h3>
                <Badge
                  :variant="apiKey.is_active ? 'success' : 'secondary'"
                  class="text-xs px-1.5 py-0"
                >
                  {{ apiKey.is_active ? '活跃' : '禁用' }}
                </Badge>
                <Badge
                  v-if="apiKey.is_locked"
                  variant="warning"
                  class="text-[10px] px-1.5 py-0"
                >
                  已锁定
                </Badge>
                <Badge
                  variant="secondary"
                  class="text-[10px] px-1.5 py-0"
                >
                  {{ formatRateLimitSimple(apiKey.rate_limit) }}
                </Badge>
                <Badge
                  variant="secondary"
                  class="text-[10px] px-1.5 py-0"
                >
                  {{ formatConcurrentLimitSimple(apiKey.concurrent_limit) }}
                </Badge>
              </div>
              <div class="flex items-center gap-0.5 flex-shrink-0">
                <Button
                  :data-testid="`ccswitch-open-mobile-${apiKey.id}`"
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  title="导入到 CC Switch"
                  @click="openCcSwitchImportDialog(apiKey)"
                >
                  <Download class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  title="一键安装并配置 CLI"
                  @click="openInstallDialog(apiKey)"
                >
                  <Terminal class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="apiKey.is_locked ? '已锁定' : '编辑'"
                  :disabled="apiKey.is_locked"
                  @click="openEditApiKeyDialog(apiKey)"
                >
                  <SquarePen class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  title="复制"
                  @click="copyApiKey(apiKey)"
                >
                  <Copy class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="apiKey.is_locked ? '已锁定' : (apiKey.is_active ? '禁用' : '启用')"
                  :disabled="apiKey.is_locked"
                  @click="toggleApiKey(apiKey)"
                >
                  <Power class="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="apiKey.is_locked ? '已锁定' : '删除'"
                  :disabled="apiKey.is_locked"
                  @click="confirmDelete(apiKey)"
                >
                  <Trash2 class="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>

            <!-- 第二行：密钥、时间、统计 -->
            <div class="space-y-1.5">
              <div class="flex items-center gap-2 text-xs">
                <code class="font-mono text-muted-foreground">{{ apiKey.key_display || 'sk-••••••••' }}</code>
                <span class="text-muted-foreground">•</span>
                <span class="text-muted-foreground">
                  {{ apiKey.last_used_at ? formatRelativeTime(apiKey.last_used_at) : '从未使用' }}
                </span>
              </div>
              <div class="flex items-center gap-3 text-xs">
                <span class="text-amber-600 dark:text-amber-500 font-semibold">
                  ${{ keyActualCost(apiKey).toFixed(4) }}
                </span>
                <span class="text-muted-foreground">
                  标价 ${{ keyCatalogCost(apiKey).toFixed(4) }} · {{ sellRateMultiplier }}x
                </span>
                <span class="text-muted-foreground">•</span>
                <span class="text-foreground font-medium">
                  {{ formatNumber(apiKey.total_requests || 0) }} 次
                </span>
                <span class="text-muted-foreground">•</span>
                <span class="text-muted-foreground">
                  {{ formatRateLimitSimple(apiKey.rate_limit) }}
                </span>
              </div>
              <div class="text-xs text-muted-foreground truncate">
                IP 限制：{{ formatIpRules(apiKey.ip_rules) }}
              </div>
            </div>
          </div>
        </Card>
      </div>

      <!-- 分页 -->
      <Pagination
        v-if="apiKeys.length > 0"
        :current="currentPage"
        :total="apiKeys.length"
        :page-size="pageSize"
        cache-key="my-api-keys-page-size"
        @update:current="currentPage = $event"
        @update:page-size="pageSize = $event"
      />
    </Card>

    <!-- 创建 API 密钥对话框 -->
    <Dialog v-model="showCreateDialog">
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
              <Key class="h-5 w-5 text-primary" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                {{ editingApiKey ? '编辑 API 密钥' : '创建 API 密钥' }}
              </h3>
              <p class="text-xs text-muted-foreground">
                {{ editingApiKey ? '更新密钥名称、速率限制和并发限制' : '创建一个新的密钥用于访问 API 服务' }}
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label
            for="key-name"
            class="text-sm font-semibold"
          >密钥名称</Label>
          <Input
            id="key-name"
            v-model="newKeyName"
            placeholder="例如：生产环境密钥"
            class="h-11 border-border/60"
            autocomplete="off"
            required
          />
          <p class="text-xs text-muted-foreground">
            给密钥起一个有意义的名称方便识别
          </p>
        </div>

        <BillingGroupSelect
          v-model="newKeyBillingGroupId"
          :groups="billingGroups"
        />

        <div class="space-y-2">
          <Label
            for="key-rate-limit"
            class="text-sm font-semibold"
          >速率限制 (请求/分钟)</Label>
          <Input
            id="key-rate-limit"
            :model-value="newKeyRateLimit ?? ''"
            type="number"
            min="0"
            max="10000"
            placeholder="留空不限"
            class="h-11 border-border/60"
            @update:model-value="(v) => newKeyRateLimit = parseNumberInput(v, { min: 0, max: 10000 })"
          />
          <p class="text-xs text-muted-foreground">
            留空不限
          </p>
        </div>

        <div class="space-y-2">
          <Label
            for="key-concurrent-limit"
            class="text-sm font-semibold"
          >并发限制</Label>
          <Input
            id="key-concurrent-limit"
            :model-value="newKeyConcurrentLimit ?? ''"
            type="number"
            min="0"
            max="10000"
            placeholder="0 = 不限并发"
            class="h-11 border-border/60"
            @update:model-value="(v) => newKeyConcurrentLimit = parseNumberInput(v, { min: 0, max: 10000 })"
          />
          <p class="text-xs text-muted-foreground">
            {{ editingApiKey ? '留空表示保持当前值，填 0 表示不限并发' : '留空表示不限并发，填 0 也表示不限并发' }}
          </p>
        </div>

        <div class="space-y-2">
          <Label
            for="key-ip-rules"
            class="text-sm font-semibold"
          >IP 限制</Label>
          <Input
            id="key-ip-rules"
            v-model="newKeyIpRulesText"
            placeholder="例如：203.0.113.10, 10.0.0.0/24, !10.0.0.13"
            class="h-11 border-border/60"
            autocomplete="off"
          />
          <p class="text-xs text-muted-foreground">
            留空表示不限制；支持 IP、CIDR、IPv4 通配符、*，用 ! 前缀拒绝，多个规则用英文逗号分隔
          </p>
        </div>

        <div class="rounded-lg border border-border/60 bg-muted/30 p-4">
          <div class="flex items-center justify-between gap-4">
            <div>
              <Label class="text-sm font-semibold">敏感信息保护</Label>
              <p class="mt-1 text-xs text-muted-foreground">
                {{ keyRedactionMode === 'inherit' ? '默认跟随账户设置' : '管理员开启功能后生效' }}
              </p>
            </div>
            <div class="flex items-center gap-2">
              <Button
                size="sm"
                :variant="keyRedactionMode === 'inherit' ? 'default' : 'outline'"
                @click="keyRedactionMode = 'inherit'"
              >
                跟随账户
              </Button>
              <Button
                size="sm"
                :variant="keyRedactionMode === 'custom' ? 'default' : 'outline'"
                @click="keyRedactionMode = 'custom'"
              >
                单独配置
              </Button>
            </div>
          </div>
          <div
            v-if="keyRedactionMode === 'custom'"
            class="mt-4 flex items-center justify-between gap-4 border-t border-border/50 pt-4"
          >
            <div>
              <Label class="text-sm font-medium">启用保护</Label>
              <p class="mt-1 text-xs text-muted-foreground">
                只影响此 API Key
              </p>
            </div>
            <Switch v-model="newKeyRedactionEnabled" />
          </div>
          <div
            v-if="keyRedactionMode === 'custom' && newKeyRedactionEnabled"
            class="mt-4 flex items-center justify-between gap-4 border-t border-border/50 pt-4"
          >
            <div>
              <Label class="text-sm font-medium">占位符说明</Label>
              <p class="mt-1 text-xs text-muted-foreground">
                向模型说明占位符含义
              </p>
            </div>
            <Switch v-model="newKeyRedactionInjectNotice" />
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-11 px-6"
          @click="closeApiKeyDialog"
        >
          取消
        </Button>
        <Button
          class="h-11 px-6 shadow-lg shadow-primary/20"
          :disabled="creating"
          @click="saveApiKey"
        >
          <Loader2
            v-if="creating"
            class="animate-spin h-4 w-4 mr-2"
          />
          {{ creating ? (editingApiKey ? '保存中...' : '创建中...') : (editingApiKey ? '保存' : '创建') }}
        </Button>
      </template>
    </Dialog>

    <!-- 新密钥创建成功对话框 -->
    <Dialog
      v-model="showKeyDialog"
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
                请妥善保管, 切勿泄露给他人
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-4">
        <div class="space-y-2">
          <Label class="text-sm font-medium">API 密钥</Label>
          <div class="flex items-center gap-2">
            <Input
              type="text"
              :value="newKeyValue"
              readonly
              class="flex-1 font-mono text-sm bg-muted/50 h-11"
              @click="($event.target as HTMLInputElement)?.select()"
            />
            <Button
              class="h-11"
              @click="copyTextToClipboard(newKeyValue)"
            >
              复制
            </Button>
          </div>
        </div>
      </div>

      <template #footer>
        <Button
          data-testid="ccswitch-open-created-key"
          variant="outline"
          class="h-10 px-5 gap-2"
          :disabled="!createdApiKey || !newKeyValue"
          @click="openCcSwitchImportDialogForCreatedKey"
        >
          <Download class="h-4 w-4" />
          导入 CC Switch
        </Button>
        <Button
          class="h-10 px-5"
          @click="closeCreatedKeyDialog"
        >
          确定
        </Button>
      </template>
    </Dialog>

    <!-- 导入 CC Switch 对话框 -->
    <Dialog
      v-model="showCcSwitchDialog"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
              <Download class="h-5 w-5 text-primary" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                导入到 CC Switch
              </h3>
              <p class="text-xs text-muted-foreground truncate">
                当前密钥：{{ selectedCcSwitchApiKey?.name || '未选择' }}
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-5">
        <div class="rounded-lg border border-border/60 bg-muted/30 p-3 text-xs text-muted-foreground">
          选择目标客户端和模型 ID。点击导入后浏览器会请求打开 CC Switch，本页面不会展示或保存包含 API Key 的链接。
        </div>

        <div class="space-y-2">
          <Label class="text-sm font-semibold">目标客户端</Label>
          <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">
            <Button
              v-for="option in ccSwitchTargetOptions"
              :key="option.value"
              :data-testid="`ccswitch-target-${option.value}`"
              :variant="ccSwitchTargetApp === option.value ? 'default' : 'outline'"
              class="justify-start h-auto py-3"
              @click="selectCcSwitchTarget(option.value)"
            >
              {{ option.label }}
            </Button>
          </div>
        </div>

        <div class="space-y-2">
          <Label
            for="ccswitch-provider-name"
            class="text-sm font-semibold"
          >站点名称</Label>
          <Input
            id="ccswitch-provider-name"
            :model-value="ccSwitchProviderName"
            class="h-11 border-border/60"
            autocomplete="off"
            data-testid="ccswitch-provider-name"
            @update:model-value="updateCcSwitchProviderName"
          />
        </div>

        <div class="space-y-3">
          <Label class="text-sm font-semibold">
            {{ ccSwitchTargetApp === 'claude' ? '模型 ID 选择' : '模型 ID' }}
          </Label>

          <div class="space-y-3">
            <div
              v-for="field in ccSwitchModelFields"
              :key="field.key"
              class="space-y-1.5"
            >
              <Label
                :for="`ccswitch-model-${field.key}`"
                class="text-xs font-medium text-muted-foreground"
              >
                {{ field.label }}
              </Label>
              <Select
                v-model="ccSwitchModelIds[field.key]"
                :disabled="!ccSwitchHasModelOptions"
              >
                <SelectTrigger
                  :id="`ccswitch-model-${field.key}`"
                  :data-testid="`ccswitch-model-select-${field.key}`"
                  class="h-11 rounded-2xl border-border/60 bg-card/80 font-mono text-xs"
                >
                  <SelectValue placeholder="选择模型 ID" />
                </SelectTrigger>
                <SelectContent
                  class="max-w-[min(26rem,calc(100vw-4rem))] max-h-[22rem]"
                  search-placeholder="搜索模型 ID..."
                  :search-threshold="4"
                >
                  <SelectItem
                    v-for="model in ccSwitchModelOptions"
                    :key="model"
                    :value="model"
                    :text-value="model"
                    class="font-mono"
                  >
                    {{ model }}
                  </SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>

          <p class="text-xs text-muted-foreground">
            {{ ccSwitchModelHelpText }}
          </p>
          <p
            v-if="!ccSwitchPreparing && !ccSwitchHasModelOptions"
            class="text-xs text-destructive"
          >
            暂无可用模型，请联系管理员配置可用模型后再导入。
          </p>
        </div>

        <div
          v-if="ccSwitchPreparing"
          class="text-xs text-muted-foreground"
        >
          正在加载导入配置...
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-10 px-5"
          @click="showCcSwitchDialog = false"
        >
          关闭
        </Button>
        <Button
          data-testid="ccswitch-confirm"
          class="h-10 px-5 shadow-lg shadow-primary/20"
          :disabled="ccSwitchConfirmDisabled"
          @click="confirmCcSwitchImport"
        >
          <Loader2
            v-if="ccSwitchLoading"
            class="animate-spin h-4 w-4 mr-2"
          />
          {{ ccSwitchLoading ? '准备中...' : '导入到 CC Switch' }}
        </Button>
      </template>
    </Dialog>

    <!-- 一键安装并配置 CLI 对话框 -->
    <Dialog
      v-model="showInstallDialog"
      size="lg"
    >
      <template #header>
        <div class="border-b border-border px-6 py-4">
          <div class="flex items-center gap-3">
            <div class="flex h-9 w-9 items-center justify-center rounded-lg bg-primary/10 flex-shrink-0">
              <Terminal class="h-5 w-5 text-primary" />
            </div>
            <div class="flex-1 min-w-0">
              <h3 class="text-lg font-semibold text-foreground leading-tight">
                一键安装并配置 CLI
              </h3>
              <p class="text-xs text-muted-foreground truncate">
                当前密钥：{{ selectedInstallApiKey?.name || '未选择' }}
              </p>
            </div>
          </div>
        </div>
      </template>

      <div class="space-y-5">
        <div class="rounded-lg border border-border/60 bg-muted/30 p-3 text-xs text-muted-foreground">
          选择要配置的 CLI 和目标系统，Aether 会生成 15 分钟内有效的一次性 install code。页面命令不会包含原始 API Key。
        </div>

        <div class="space-y-2">
          <Label class="text-sm font-semibold">目标 CLI</Label>
          <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
            <Button
              v-for="option in installCliOptions"
              :key="option.value"
              :variant="installCli === option.value ? 'default' : 'outline'"
              class="justify-start h-auto py-3"
              @click="selectInstallCli(option.value)"
            >
              {{ option.label }}
            </Button>
          </div>
        </div>

        <div class="space-y-2">
          <Label class="text-sm font-semibold">目标系统</Label>
          <div class="grid grid-cols-1 sm:grid-cols-3 gap-2">
            <Button
              v-for="option in installSystemOptions"
              :key="option.value"
              :variant="installSystem === option.value ? 'default' : 'outline'"
              class="justify-start h-auto py-3"
              @click="selectInstallSystem(option.value)"
            >
              {{ option.label }}
            </Button>
          </div>
        </div>

        <div class="space-y-2">
          <div class="flex items-center justify-between gap-2">
            <Label class="text-sm font-semibold">复制到目标机器执行</Label>
            <div class="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                class="gap-1.5"
                :disabled="installLoading || !installCommand"
                :title="installCopied ? '已复制' : '一键复制安装命令'"
                @click="copyInstallCommand"
              >
                <CheckCircle
                  v-if="installCopied"
                  class="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400"
                />
                <Copy
                  v-else
                  class="h-3.5 w-3.5"
                />
                {{ installCopied ? '已复制' : '一键复制' }}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                :disabled="installLoading || !selectedInstallApiKey"
                @click="refreshInstallCommand"
              >
                {{ installLoading ? '生成中...' : '重新生成' }}
              </Button>
            </div>
          </div>
          <div class="rounded-lg border border-border/60 bg-background overflow-hidden">
            <pre class="max-h-32 overflow-x-auto whitespace-pre-wrap break-all p-3 text-xs font-mono">{{ installCommand || '正在生成短命令...' }}</pre>
          </div>
          <p class="text-xs text-muted-foreground">
            {{ installCommandHint }}
          </p>
        </div>
      </div>

      <template #footer>
        <Button
          variant="outline"
          class="h-10 px-5"
          @click="showInstallDialog = false"
        >
          关闭
        </Button>
        <Button
          class="h-10 px-5 shadow-lg shadow-primary/20"
          :disabled="!installCommand || installLoading"
          @click="copyInstallCommand"
        >
          {{ installCopied ? '已复制' : '复制命令' }}
        </Button>
      </template>
    </Dialog>

    <!-- 删除确认对话框 -->
    <AlertDialog
      v-model="showDeleteDialog"
      type="danger"
      title="确认删除"
      :description="`确定要删除密钥 &quot;${keyToDelete?.name}&quot; 吗？此操作不可恢复。`"
      confirm-text="删除"
      :loading="deleting"
      @confirm="deleteApiKey"
      @cancel="showDeleteDialog = false"
    />
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onBeforeUnmount, computed, watch, reactive } from 'vue'
import { meApi, type ApiKey, type UserBillingGroup, type InstallSessionTargetSystem, type InstallTargetCli, type ApiKeyInstallSession } from '@/api/me'
import Card from '@/components/ui/card.vue'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Badge from '@/components/ui/badge.vue'
import Switch from '@/components/ui/switch.vue'
import {
  Dialog,
  Pagination,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui'
import { LoadingState, AlertDialog, EmptyState } from '@/components/common'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow
} from '@/components/ui'
import RefreshButton from '@/components/ui/refresh-button.vue'
import { Plus, Key, Copy, Trash2, Loader2, Activity, CheckCircle, Power, SquarePen, Terminal, Download } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'
import { formatRateLimitSimple } from '@/utils/format'
import { parseNumberInput } from '@/utils/form'
import { getErrorStatus } from '@/types/api-error'
import BillingGroupSelect from '@/features/api-keys/components/BillingGroupSelect.vue'
import {
  hasChatPiiRedactionFeatureSettings,
  mergeChatPiiRedactionFeatureSettings,
  readChatPiiRedactionFeatureSettings,
} from '@/utils/featureSettings'
import {
  CC_SWITCH_TARGET_OPTIONS,
  buildCcSwitchProviderImportUrl,
  defaultCcSwitchProviderName,
  type CcSwitchModelIds,
  type CcSwitchTargetApp,
} from '@/features/api-keys/utils/ccswitchImport'

const { success, error: showError } = useToast()

const installCliOptions: Array<{ value: InstallTargetCli; label: string }> = [
  { value: 'claude_code', label: 'Claude Code' },
  { value: 'codex_cli', label: 'Codex CLI' },
  { value: 'gemini_cli', label: 'Gemini CLI' }
]

const installSystemOptions: Array<{ value: InstallSessionTargetSystem; label: string }> = [
  { value: 'macos', label: 'macOS' },
  { value: 'linux', label: 'Linux' },
  { value: 'windows', label: 'Windows' }
]

const ccSwitchTargetOptions = CC_SWITCH_TARGET_OPTIONS
type CcSwitchModelFieldKey = keyof Required<CcSwitchModelIds>

interface CcSwitchModelField {
  key: CcSwitchModelFieldKey
  label: string
}

const apiKeys = ref<ApiKey[]>([])
const loading = ref(false)
const creating = ref(false)
const deleting = ref(false)
const billingGroups = ref<UserBillingGroup[]>([])

function keyCatalogCost(apiKey: ApiKey): number {
  return Number(apiKey.total_cost_usd) || 0
}

function keyActualCost(apiKey: ApiKey): number {
  return Number(apiKey.actual_total_cost_usd) || 0
}

// 分页相关
const currentPage = ref(1)
const pageSize = ref(10)

const paginatedApiKeys = computed(() => {
  const start = (currentPage.value - 1) * pageSize.value
  return apiKeys.value.slice(start, start + pageSize.value)
})

const showCreateDialog = ref(false)
const showKeyDialog = ref(false)
const showDeleteDialog = ref(false)
const showInstallDialog = ref(false)
const showCcSwitchDialog = ref(false)

const newKeyName = ref('')
const newKeyBillingGroupId = ref('')
const newKeyRateLimit = ref<number | undefined>(undefined)
const newKeyConcurrentLimit = ref<number | undefined>(undefined)
const newKeyIpRulesText = ref('')
const keyRedactionMode = ref<'inherit' | 'custom'>('inherit')
const newKeyRedactionEnabled = ref(false)
const newKeyRedactionInjectNotice = ref(true)
const newKeyValue = ref('')
const createdApiKey = ref<ApiKey | null>(null)
const keyToDelete = ref<ApiKey | null>(null)
const editingApiKey = ref<ApiKey | null>(null)
const selectedInstallApiKey = ref<ApiKey | null>(null)
const pendingFirstInstallApiKey = ref<ApiKey | null>(null)
const installCli = ref<InstallTargetCli>('claude_code')
const installSystem = ref<InstallSessionTargetSystem>('linux')
const installSession = ref<ApiKeyInstallSession | null>(null)
const installLoading = ref(false)
const installCopied = ref(false)
let installCopiedResetTimer: ReturnType<typeof setTimeout> | null = null
const selectedCcSwitchApiKey = ref<ApiKey | null>(null)
const ccSwitchPlainApiKey = ref('')
const ccSwitchTargetApp = ref<CcSwitchTargetApp>('claude')
const ccSwitchProviderName = ref('')
const ccSwitchProviderNameDirty = ref(false)
const ccSwitchSiteName = ref('Aether')
const ccSwitchModelIds = reactive<Record<CcSwitchModelFieldKey, string>>({
  default: '',
  haiku: '',
  sonnet: '',
  opus: '',
})
const ccSwitchBaseUrl = ref('')
const ccSwitchAvailableModels = ref<string[]>([])
const ccSwitchPreparing = ref(false)
const ccSwitchLoading = ref(false)

const installCommand = computed(() => {
  if (!installSession.value) return ''
  return installSystem.value === 'windows'
    ? installSession.value.powershell_command
    : installSession.value.unix_command
})

const installCommandHint = computed(() => {
  if (installSystem.value === 'windows') {
    return 'Windows 请在 PowerShell 中执行。install code 使用后立即失效，如需再次执行请重新生成。'
  }
  return 'macOS / Linux 请在 sh 兼容终端中执行。install code 使用后立即失效，如需再次执行请重新生成。'
})

const ccSwitchModelOptions = computed(() => ccSwitchAvailableModels.value)
const ccSwitchHasModelOptions = computed(() => ccSwitchModelOptions.value.length > 0)
const ccSwitchConfirmDisabled = computed(() =>
  ccSwitchLoading.value || ccSwitchPreparing.value || !ccSwitchHasModelOptions.value,
)
const ccSwitchModelFields = computed<CcSwitchModelField[]>(() => {
  if (ccSwitchTargetApp.value === 'claude') {
    return [
      {
        key: 'haiku',
        label: 'Haiku 模型 ID',
      },
      {
        key: 'sonnet',
        label: 'Sonnet 模型 ID',
      },
      {
        key: 'opus',
        label: 'Opus 模型 ID',
      },
    ]
  }

  return [
    {
      key: 'default',
      label: '默认模型 ID',
    },
  ]
})
const ccSwitchModelHelpText = computed(() =>
  ccSwitchTargetApp.value === 'claude'
    ? 'Claude Code 会分别写入 Haiku、Sonnet、Opus，Sonnet 同时作为默认模型；模型多时可在下拉中搜索并滚动选择。'
    : '从 Aether 可用模型中选择，模型多时可在下拉中搜索并滚动选择。',
)

onMounted(() => {
  installSystem.value = detectCurrentSystem()
  loadApiKeys()
})

onBeforeUnmount(() => {
  resetInstallCopiedState()
})

watch(showInstallDialog, (isOpen) => {
  if (!isOpen) {
    resetInstallCopiedState()
  }
})

watch(showKeyDialog, (isOpen) => {
  if (!isOpen && pendingFirstInstallApiKey.value) {
    closeCreatedKeyDialog()
  }
})

async function loadApiKeys() {
  loading.value = true
  try {
    const [keys, groups] = await Promise.all([meApi.getApiKeys(), meApi.getBillingGroups()])
    apiKeys.value = keys
    billingGroups.value = groups
  } catch (error: unknown) {
    log.error('加载 API 密钥失败:', error)
    const status = getErrorStatus(error)
    if (status === undefined) {
      showError('无法连接到服务器，请检查后端服务是否运行')
    } else if (status === 401) {
      showError('认证失败，请重新登录')
    } else {
      showError(parseApiError(error, '加载 API 密钥失败'))
    }
  } finally {
    loading.value = false
  }
}

function clearInstallCopiedResetTimer() {
  if (installCopiedResetTimer) {
    clearTimeout(installCopiedResetTimer)
    installCopiedResetTimer = null
  }
}

function resetInstallCopiedState() {
  clearInstallCopiedResetTimer()
  installCopied.value = false
}

function openEditApiKeyDialog(apiKey: ApiKey) {
  const hasRedactionFeature = hasChatPiiRedactionFeatureSettings(apiKey.feature_settings)
  const redactionFeature = readChatPiiRedactionFeatureSettings(apiKey.feature_settings)
  editingApiKey.value = apiKey
  newKeyName.value = apiKey.name || ''
  newKeyBillingGroupId.value = apiKey.group_id
  newKeyRateLimit.value = apiKey.rate_limit ?? undefined
  newKeyConcurrentLimit.value = apiKey.concurrent_limit ?? undefined
  newKeyIpRulesText.value = apiKey.ip_rules?.join(', ') ?? ''
  keyRedactionMode.value = hasRedactionFeature ? 'custom' : 'inherit'
  newKeyRedactionEnabled.value = redactionFeature.enabled
  newKeyRedactionInjectNotice.value = redactionFeature.inject_model_instruction
  showCreateDialog.value = true
}

function openCreateApiKeyDialog() {
  editingApiKey.value = null
  createdApiKey.value = null
  newKeyName.value = ''
  newKeyBillingGroupId.value = ''
  newKeyRateLimit.value = undefined
  newKeyConcurrentLimit.value = undefined
  newKeyIpRulesText.value = ''
  keyRedactionMode.value = 'inherit'
  newKeyRedactionEnabled.value = false
  newKeyRedactionInjectNotice.value = true
  showCreateDialog.value = true
}

function detectCurrentSystem(): InstallSessionTargetSystem {
  const platform = window.navigator.platform.toLowerCase()
  const userAgent = window.navigator.userAgent.toLowerCase()
  if (platform.includes('mac')) return 'macos'
  if (platform.includes('win') || userAgent.includes('windows')) return 'windows'
  return 'linux'
}

async function openInstallDialog(apiKey: ApiKey) {
  selectedInstallApiKey.value = apiKey
  installSession.value = null
  resetInstallCopiedState()
  showInstallDialog.value = true
  await refreshInstallCommand()
}

async function selectInstallCli(value: InstallTargetCli) {
  installCli.value = value
  await refreshInstallCommand()
}

async function selectInstallSystem(value: InstallSessionTargetSystem) {
  installSystem.value = value
  await refreshInstallCommand()
}

async function refreshInstallCommand() {
  if (!selectedInstallApiKey.value) return
  installLoading.value = true
  installSession.value = null
  resetInstallCopiedState()
  try {
    installSession.value = await meApi.createApiKeyInstallSession(selectedInstallApiKey.value.id, {
      target_cli: installCli.value,
      target_system: installSystem.value,
    })
  } catch (error) {
    log.error('生成 CLI 安装命令失败:', error)
    showError(parseApiError(error, '生成 CLI 安装命令失败'))
  } finally {
    installLoading.value = false
  }
}

async function copyInstallCommand() {
  if (!installCommand.value) return
  const copied = await copyTextToClipboard(installCommand.value, false)
  if (!copied) return

  installCopied.value = true
  success('安装命令已复制到剪贴板')
  clearInstallCopiedResetTimer()
  installCopiedResetTimer = setTimeout(() => {
    installCopied.value = false
    installCopiedResetTimer = null
  }, 2000)
}

function uniqueModelNames(models: Array<{ name?: string | null; display_name?: string | null }>): string[] {
  const names = new Set<string>()
  for (const model of models) {
    const name = String(model.name || '').trim()
    if (name) names.add(name)
  }
  return Array.from(names)
}

function findRecommendedModel(
  models: string[],
  predicate: (model: string) => boolean,
): string | undefined {
  return models.find(model => predicate(model.toLowerCase()))
}

function recommendedDefaultCcSwitchModel(targetApp: CcSwitchTargetApp, models: string[]): string {
  const findModel = (predicate: (model: string) => boolean) =>
    findRecommendedModel(models, predicate)

  if (targetApp === 'claude') {
    return findModel(model => model.includes('claude') && model.includes('sonnet'))
      || findModel(model => model.includes('claude'))
      || findModel(model => model.startsWith('gpt') || model.includes('gpt-'))
      || models[0]
      || ''
  }

  if (targetApp === 'gemini') {
    return findModel(model => model.includes('gemini')) || models[0] || ''
  }

  return findModel(model => model.startsWith('gpt') || model.includes('gpt-'))
    || models[0]
    || ''
}

function recommendedCcSwitchModelIds(targetApp: CcSwitchTargetApp, models: string[]): Record<CcSwitchModelFieldKey, string> {
  const defaultModel = recommendedDefaultCcSwitchModel(targetApp, models)

  if (targetApp !== 'claude') {
    return {
      default: defaultModel,
      haiku: '',
      sonnet: '',
      opus: '',
    }
  }

  const gptFallback = findRecommendedModel(
    models,
    model => model.startsWith('gpt') || model.includes('gpt-'),
  ) || defaultModel
  const sonnet = findRecommendedModel(
    models,
    model => model.includes('sonnet'),
  ) || findRecommendedModel(
    models,
    model => model.includes('claude'),
  ) || gptFallback

  return {
    default: sonnet,
    haiku: findRecommendedModel(models, model => model.includes('haiku')) || gptFallback,
    sonnet,
    opus: findRecommendedModel(models, model => model.includes('opus')) || sonnet,
  }
}

function applyRecommendedCcSwitchModelIds(targetApp: CcSwitchTargetApp) {
  const recommended = recommendedCcSwitchModelIds(targetApp, ccSwitchAvailableModels.value)
  for (const field of ccSwitchModelFields.value) {
    ccSwitchModelIds[field.key] = recommended[field.key]
  }
}

function updateCcSwitchProviderName(value: string) {
  ccSwitchProviderName.value = value
  ccSwitchProviderNameDirty.value = true
}

function selectedCcSwitchModelIds(): CcSwitchModelIds {
  if (ccSwitchTargetApp.value === 'claude') {
    return {
      default: ccSwitchModelIds.sonnet.trim(),
      haiku: ccSwitchModelIds.haiku.trim(),
      sonnet: ccSwitchModelIds.sonnet.trim(),
      opus: ccSwitchModelIds.opus.trim(),
    }
  }

  return {
    default: ccSwitchModelIds.default.trim(),
  }
}

async function prepareCcSwitchDialog() {
  ccSwitchPreparing.value = true
  try {
    const [clientConfig, modelsResponse] = await Promise.all([
      meApi.getClientConfig(),
      meApi.getAvailableModels({ limit: 1000 }),
    ])
    ccSwitchBaseUrl.value = clientConfig.base_url
    ccSwitchSiteName.value = clientConfig.site_name?.trim() || 'Aether'
    if (!ccSwitchProviderNameDirty.value) {
      ccSwitchProviderName.value = defaultCcSwitchProviderName(ccSwitchSiteName.value)
    }
    ccSwitchAvailableModels.value = uniqueModelNames(modelsResponse.models || [])
    applyRecommendedCcSwitchModelIds(ccSwitchTargetApp.value)
  } catch (error) {
    log.error('加载 CC Switch 导入配置失败:', error)
    showError(parseApiError(error, '加载 CC Switch 导入配置失败'))
  } finally {
    ccSwitchPreparing.value = false
  }
}

async function openCcSwitchImportDialog(apiKey: ApiKey, plainApiKey = '') {
  selectedCcSwitchApiKey.value = apiKey
  ccSwitchPlainApiKey.value = plainApiKey
  ccSwitchTargetApp.value = 'claude'
  ccSwitchProviderNameDirty.value = false
  ccSwitchProviderName.value = defaultCcSwitchProviderName(ccSwitchSiteName.value)
  ccSwitchBaseUrl.value = ''
  ccSwitchAvailableModels.value = []
  for (const key of Object.keys(ccSwitchModelIds) as CcSwitchModelFieldKey[]) {
    ccSwitchModelIds[key] = ''
  }
  showCcSwitchDialog.value = true
  await prepareCcSwitchDialog()
}

async function openCcSwitchImportDialogForCreatedKey() {
  if (!createdApiKey.value || !newKeyValue.value) return
  pendingFirstInstallApiKey.value = null
  showKeyDialog.value = false
  await openCcSwitchImportDialog(createdApiKey.value, newKeyValue.value)
}

function selectCcSwitchTarget(value: CcSwitchTargetApp) {
  ccSwitchTargetApp.value = value
  applyRecommendedCcSwitchModelIds(value)
}

function isMissingFullApiKeyError(message: string): boolean {
  return message.includes('没有存储完整密钥信息') || message.includes('缺少完整密钥')
}

async function confirmCcSwitchImport() {
  if (!selectedCcSwitchApiKey.value) return

  if (!ccSwitchHasModelOptions.value) {
    showError('暂无可用模型，请联系管理员配置可用模型后再导入')
    return
  }

  const missingModelField = ccSwitchModelFields.value.find(field => !ccSwitchModelIds[field.key].trim())
  if (missingModelField) {
    showError(`请填写${missingModelField.label}`)
    return
  }

  ccSwitchLoading.value = true
  try {
    const apiKey = ccSwitchPlainApiKey.value
      || (await meApi.getFullApiKey(selectedCcSwitchApiKey.value.id)).key
    const baseUrl = ccSwitchBaseUrl.value || (await meApi.getClientConfig()).base_url
    const importUrl = buildCcSwitchProviderImportUrl({
      targetApp: ccSwitchTargetApp.value,
      baseUrl,
      apiKey,
      apiKeyName: selectedCcSwitchApiKey.value.name,
      siteName: ccSwitchSiteName.value,
      modelIds: selectedCcSwitchModelIds(),
      providerName: ccSwitchProviderName.value,
    })

    if (importUrl.length > 8000) {
      showError('CC Switch 导入链接过长，请减少模型配置后重试')
      return
    }

    window.location.href = importUrl
    success('如果浏览器询问是否打开 CC Switch，请选择允许')
    showCcSwitchDialog.value = false
  } catch (error) {
    log.error('生成 CC Switch 导入链接失败:', error)
    const message = parseApiError(error, '生成 CC Switch 导入链接失败')
    showError(
      isMissingFullApiKeyError(message)
        ? '该密钥缺少完整密钥信息，请重新创建 API Key'
        : message,
    )
  } finally {
    ccSwitchLoading.value = false
  }
}

function closeCreatedKeyDialog() {
  showKeyDialog.value = false
  const pending = pendingFirstInstallApiKey.value
  pendingFirstInstallApiKey.value = null
  createdApiKey.value = null
  if (pending) {
    void openInstallDialog(pending)
  }
}

function closeApiKeyDialog() {
  showCreateDialog.value = false
  editingApiKey.value = null
  if (!showKeyDialog.value) {
    createdApiKey.value = null
  }
  newKeyName.value = ''
  newKeyBillingGroupId.value = ''
  newKeyRateLimit.value = undefined
  newKeyConcurrentLimit.value = undefined
  newKeyIpRulesText.value = ''
  keyRedactionMode.value = 'inherit'
  newKeyRedactionEnabled.value = false
  newKeyRedactionInjectNotice.value = true
}

async function saveApiKey() {
  if (!newKeyName.value.trim()) {
    showError('请输入密钥名称')
    return
  }
  if (!newKeyBillingGroupId.value) {
    showError('请选择计费用户组')
    return
  }

  creating.value = true
  try {
    const ipRules = parseIpRulesInput(newKeyIpRulesText.value)
    const isCreatingFirstApiKey = !editingApiKey.value && apiKeys.value.length === 0
    if (editingApiKey.value) {
      await meApi.updateApiKey(editingApiKey.value.id, {
        name: newKeyName.value,
        group_id: newKeyBillingGroupId.value,
        rate_limit: newKeyRateLimit.value ?? 0,
        concurrent_limit: newKeyConcurrentLimit.value,
        ip_rules: ipRules,
        feature_settings: keyRedactionMode.value === 'custom'
          ? mergeChatPiiRedactionFeatureSettings(editingApiKey.value.feature_settings, {
                enabled: newKeyRedactionEnabled.value,
                inject_model_instruction: newKeyRedactionInjectNotice.value,
            })
          : null,
      })
      success('API 密钥更新成功')
    } else {
      const newKey = await meApi.createApiKey({
        name: newKeyName.value,
        group_id: newKeyBillingGroupId.value,
        rate_limit: newKeyRateLimit.value ?? 0,
        concurrent_limit: newKeyConcurrentLimit.value,
        ip_rules: ipRules,
        ...(keyRedactionMode.value === 'custom'
          ? {
              feature_settings: mergeChatPiiRedactionFeatureSettings(null, {
                enabled: newKeyRedactionEnabled.value,
                inject_model_instruction: newKeyRedactionInjectNotice.value,
              }),
            }
          : {}),
      })
      newKeyValue.value = newKey.key || ''
      createdApiKey.value = newKey
      if (isCreatingFirstApiKey) {
        pendingFirstInstallApiKey.value = newKey
      }
      showKeyDialog.value = true
      success('API 密钥创建成功')
    }
    closeApiKeyDialog()
    await loadApiKeys()
  } catch (error) {
    log.error(editingApiKey.value ? '更新 API 密钥失败:' : '创建 API 密钥失败:', error)
    showError(editingApiKey.value ? '更新 API 密钥失败' : '创建 API 密钥失败')
  } finally {
    creating.value = false
  }
}

function confirmDelete(apiKey: ApiKey) {
  keyToDelete.value = apiKey
  showDeleteDialog.value = true
}

async function deleteApiKey() {
  if (!keyToDelete.value) return

  deleting.value = true
  try {
    await meApi.deleteApiKey(keyToDelete.value.id)
    apiKeys.value = apiKeys.value.filter(k => k.id !== keyToDelete.value?.id)
    showDeleteDialog.value = false
    success('API 密钥已删除')
  } catch (error) {
    log.error('删除 API 密钥失败:', error)
    showError('删除 API 密钥失败')
  } finally {
    deleting.value = false
    keyToDelete.value = null
  }
}

async function toggleApiKey(apiKey: ApiKey) {
  try {
    const updated = await meApi.toggleApiKey(apiKey.id, !apiKey.is_active)
    const index = apiKeys.value.findIndex(k => k.id === apiKey.id)
    if (index !== -1) {
      apiKeys.value[index].is_active = updated.is_active
    }
    success(updated.is_active ? '密钥已启用' : '密钥已禁用')
  } catch (error) {
    log.error('切换密钥状态失败:', error)
    showError('操作失败')
  }
}

async function copyApiKey(apiKey: ApiKey) {
  try {
    // 调用后端 API 获取完整密钥
    const response = await meApi.getFullApiKey(apiKey.id)
    const copied = await copyTextToClipboard(response.key, false) // 不显示内部提示
    if (copied) {
      success('完整密钥已复制到剪贴板')
    }
  } catch (error) {
    log.error('复制密钥失败:', error)
    showError('复制失败，请重试')
  }
}

async function copyTextToClipboard(text: string, showToast: boolean = true): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      if (showToast) success('已复制到剪贴板')
      return true
    } else {
      const textArea = document.createElement('textarea')
      textArea.value = text
      textArea.style.position = 'fixed'
      textArea.style.left = '-999999px'
      textArea.style.top = '-999999px'
      document.body.appendChild(textArea)
      textArea.focus()
      textArea.select()

      try {
        const successful = document.execCommand('copy')
        if (successful && showToast) {
          success('已复制到剪贴板')
        }
        if (successful) {
          return true
        } else {
          showError('复制失败，请手动复制')
          return false
        }
      } finally {
        document.body.removeChild(textArea)
      }
    }
  } catch (error) {
    log.error('复制失败:', error)
    showError('复制失败，请手动选择文本进行复制')
    return false
  }
}

function formatNumber(num: number | undefined | null): string {
  if (num === undefined || num === null) {
    return '0'
  }
  return num.toLocaleString('zh-CN')
}

function formatConcurrentLimitSimple(concurrentLimit?: number | null): string {
  if (concurrentLimit == null || concurrentLimit === 0) {
    return '不限并发'
  }
  return `${concurrentLimit} 并发`
}

function formatIpRules(ipRules?: string[] | null): string {
  return ipRules && ipRules.length > 0 ? ipRules.join(', ') : '不限制'
}

function parseIpRulesInput(value: string): string[] | null {
  const items = value
    .split(',')
    .map((item) => item.trim())
    .filter(Boolean)
  return items.length > 0 ? items : null
}

function formatDate(dateString?: string | null): string {
  if (!dateString) return '未知'
  const date = new Date(dateString)
  if (Number.isNaN(date.getTime())) return '未知'
  return date.toLocaleDateString('zh-CN', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit'
  })
}

function formatRelativeTime(dateString: string): string {
  const date = new Date(dateString)
  if (Number.isNaN(date.getTime())) return '未知'
  const now = new Date()
  const diffMs = now.getTime() - date.getTime()
  const diffMins = Math.floor(diffMs / (1000 * 60))
  const diffHours = Math.floor(diffMs / (1000 * 60 * 60))
  const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24))

  if (diffMins < 1) return '刚刚'
  if (diffMins < 60) return `${diffMins}分钟前`
  if (diffHours < 24) return `${diffHours}小时前`
  if (diffDays < 7) return `${diffDays}天前`

  return formatDate(dateString)
}

</script>
