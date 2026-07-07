<template>
  <Dialog
    :model-value="isOpen"
    :title="legacyT('添加账号')"
    :icon="UserPlus"
    size="md"
    @update:model-value="handleDialogUpdate"
  >
    <!-- 右上角代理按钮 -->
    <template #header-actions>
      <Popover
        :open="proxyPopoverOpen"
        @update:open="(v: boolean) => { proxyPopoverOpen = v; if (v) proxyNodesStore.ensureLoaded() }"
      >
        <PopoverTrigger as-child>
          <button
            class="flex items-center justify-center w-8 h-8 rounded-md transition-colors shrink-0"
            :class="selectedProxyNodeId
              ? 'text-blue-500 bg-blue-500/10 hover:bg-blue-500/20'
              : 'text-muted-foreground hover:text-foreground hover:bg-muted'"
            :title="selectedProxyNodeId ? `${legacyT('代理')}: ${getSelectedNodeLabel()}` : legacyT('设置代理节点')"
          >
            <Globe class="w-4 h-4" />
          </button>
        </PopoverTrigger>
        <PopoverContent
          class="w-72 p-3 z-[80]"
          side="bottom"
          align="end"
        >
          <div class="space-y-2">
            <div class="flex items-center justify-between">
              <div class="flex items-center gap-1.5">
                <span class="text-xs font-medium">{{ legacyT('代理节点') }}</span>
                <span
                  v-if="!proxyNodesStore.loading && proxyNodesStore.onlineNodes.length === 0"
                  class="text-[10px] text-muted-foreground"
                >· {{ legacyT('前往「模块管理 · 代理节点」添加') }}</span>
              </div>
              <button
                v-if="selectedProxyNodeId"
                class="text-[10px] text-muted-foreground hover:text-foreground transition-colors"
                @click="selectedProxyNodeId = ''; proxyPopoverOpen = false"
              >
                {{ legacyT('清除') }}
              </button>
            </div>
            <ProxyNodeSelect
              :model-value="selectedProxyNodeId"
              trigger-class="h-8"
              @update:model-value="(v: string) => { selectedProxyNodeId = v; proxyPopoverOpen = false }"
            />
            <p class="text-[10px] text-muted-foreground">
              {{ selectedProxyNodeId ? proxyUsageDescription : legacyT('未设置，依次回退到提供商代理 → 系统代理') }}
            </p>
          </div>
        </PopoverContent>
      </Popover>
    </template>

    <div class="space-y-4">
      <!-- Tab 切换 -->
      <div
        v-if="showAuthorizationMode"
        class="flex rounded-lg border border-border p-0.5 bg-muted/30"
      >
        <button
          class="flex-1 px-3 py-1.5 text-xs font-medium rounded-md transition-all"
          :class="[
            mode === 'oauth'
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground',
          ]"
          @click="switchMode('oauth')"
        >
          {{ authorizationModeLabel }}
        </button>
        <button
          class="flex-1 px-3 py-1.5 text-xs font-medium rounded-md transition-all"
          :class="mode === 'import'
            ? 'bg-background text-foreground shadow-sm'
            : 'text-muted-foreground hover:text-foreground'"
          @click="switchMode('import')"
        >
          {{ importModeLabel }}
        </button>
      </div>

      <!-- Tab 内容：grid 叠放，高度取较高者 -->
      <div class="grid [&>*]:col-start-1 [&>*]:row-start-1">
        <!-- ===== 获取授权 / 设备授权 ===== -->
        <div
          class="space-y-4 transition-opacity duration-150"
          :class="mode === 'oauth' ? 'opacity-100' : 'opacity-0 pointer-events-none'"
        >
          <!-- Windsurf: 浏览器 session/poll 授权 -->
          <template v-if="isWindsurfProvider">
            <div class="space-y-4">
              <div class="grid grid-cols-3 gap-1.5">
                <button
                  v-for="opt in ([
                    { key: 'default', label: '默认' },
                    { key: 'google', label: 'Google' },
                    { key: 'github', label: 'GitHub' },
                  ] as const)"
                  :key="opt.key"
                  class="h-8 text-xs font-medium rounded-md border transition-colors"
                  :class="device.auth_type === opt.key
                    ? 'border-primary bg-primary/5 text-foreground'
                    : 'border-border text-muted-foreground hover:text-foreground hover:border-foreground/20'"
                  @click="selectWindsurfLoginOption(opt.key)"
                >
                  {{ legacyT(opt.label) }}
                </button>
              </div>

              <div
                v-if="device.status === 'error' || device.status === 'expired'"
                class="rounded-xl border border-destructive/20 bg-destructive/5 p-5"
              >
                <div class="flex flex-col items-center text-center space-y-3">
                  <div class="w-10 h-10 rounded-full bg-destructive/10 flex items-center justify-center">
                    <AlertCircle class="w-5 h-5 text-destructive" />
                  </div>
                  <div class="space-y-1">
                    <p class="text-sm font-medium text-destructive">
                      {{ legacyT(device.status === 'expired' ? '授权已过期' : '授权失败') }}
                    </p>
                    <p class="text-xs text-muted-foreground">
                      {{ legacyT(device.error || '请重试') }}
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    @click="resetDevice"
                  >
                    {{ legacyT('重新开始') }}
                  </Button>
                </div>
              </div>

              <div
                v-else-if="device.starting && !device.session_id"
                class="flex items-center justify-center py-12"
              >
                <div class="text-center">
                  <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-primary mx-auto mb-3" />
                  <p class="text-xs text-muted-foreground">
                    {{ legacyT('正在准备登录...') }}
                  </p>
                </div>
              </div>

              <div
                v-else
                class="space-y-4"
              >
                <div class="space-y-2">
                  <div class="flex items-center gap-2">
                    <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">1</span>
                    <span class="text-xs font-medium">{{ legacyT('前往登录') }}</span>
                  </div>
                  <div class="flex gap-2 pl-6">
                    <Button
                      size="sm"
                      :disabled="device.starting || device.completing || !device.verification_uri_complete"
                      @click="openDeviceVerificationUrl"
                    >
                      <ExternalLink class="w-3 h-3 mr-1" />
                      {{ legacyT('打开') }}
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      :disabled="device.starting || device.completing || !device.verification_uri_complete"
                      @click="copyToClipboard(device.verification_uri_complete)"
                    >
                      <Copy class="w-3 h-3 mr-1" />
                      {{ legacyT('复制') }}
                    </Button>
                    <Button
                      v-if="!device.session_id"
                      size="sm"
                      variant="outline"
                      :disabled="device.starting"
                      @click="startDeviceAuth"
                    >
                      {{ legacyT('开始') }}
                    </Button>
                  </div>
                </div>

                <div class="space-y-2">
                  <div class="flex items-center gap-2">
                    <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">2</span>
                    <span class="text-xs font-medium">{{ legacyT('粘贴回调 URL 或 token') }}</span>
                  </div>
                  <div class="pl-6">
                    <Textarea
                      v-model="device.callback_url"
                      :disabled="device.completing"
                      :placeholder="deviceCallbackPlaceholder"
                      class="min-h-[150px] text-xs font-mono break-all !rounded-xl"
                      spellcheck="false"
                    />
                  </div>
                  <div
                    v-if="device.session_id && device.status === 'pending'"
                    class="pl-6 flex items-center gap-1.5 text-[11px] text-muted-foreground"
                  >
                    <div class="animate-spin rounded-full h-3 w-3 border-[1.5px] border-primary/30 border-t-primary" />
                    <span>{{ sessionRemainingText }}</span>
                  </div>
                </div>
              </div>
            </div>
          </template>

          <!-- Kiro: 设备授权模式 -->
          <template v-else-if="isKiroProvider">
            <div class="space-y-3">
              <!-- 授权类型切换 -->
              <div class="grid grid-cols-2 gap-1.5">
                <button
                  v-for="opt in ([
                    { key: 'google', label: 'Google' },
                    { key: 'github', label: 'GitHub' },
                    { key: 'builder_id', label: 'Builder ID' },
                    { key: 'identity_center', label: 'Identity Center' },
                  ] as const)"
                  :key="opt.key"
                  class="h-8 text-xs font-medium rounded-md border transition-colors disabled:opacity-60"
                  :class="device.auth_type === opt.key
                    ? 'border-primary bg-primary/5 text-foreground'
                    : 'border-border text-muted-foreground hover:text-foreground hover:border-foreground/20'"
                  :disabled="isKiroDeviceAuthOptionDisabled(opt.key)"
                  @click="selectDeviceAuthType(opt.key)"
                >
                  {{ opt.label }}
                </button>
              </div>

              <div class="h-[265px]">
                <!-- 错误/过期 -->
                <div
                  v-if="device.status === 'error' || device.status === 'expired'"
                  class="rounded-xl border border-destructive/20 bg-destructive/5 p-5"
                >
                  <div class="flex flex-col items-center text-center space-y-3">
                    <div class="w-10 h-10 rounded-full bg-destructive/10 flex items-center justify-center">
                      <AlertCircle class="w-5 h-5 text-destructive" />
                    </div>
                    <div class="space-y-1">
                      <p class="text-sm font-medium text-destructive">
                        {{ legacyT(device.status === 'expired' ? '授权已过期' : '授权失败') }}
                      </p>
                      <p class="text-xs text-muted-foreground">
                        {{ legacyT(device.error || '请重试') }}
                      </p>
                    </div>
                    <Button
                      size="sm"
                      variant="outline"
                      @click="resetDevice"
                    >
                      {{ legacyT('重新开始') }}
                    </Button>
                  </div>
                </div>

                <!-- Builder ID / Identity Center: 发起中 -->
                <div
                  v-else-if="device.starting && !isSocialDeviceAuth"
                  class="flex items-center justify-center py-12"
                >
                  <div class="text-center">
                    <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-primary mx-auto mb-3" />
                    <p class="text-xs text-muted-foreground">
                      {{ legacyT('正在注册设备...') }}
                    </p>
                  </div>
                </div>

                <!-- Google / GitHub: 粘贴回调 URL -->
                <div
                  v-else-if="isSocialDeviceAuth"
                  class="flex h-full flex-col gap-5 pt-1"
                >
                  <div class="space-y-2 shrink-0">
                    <div class="flex items-center gap-2">
                      <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">1</span>
                      <span class="text-xs font-medium">{{ legacyT('前往授权') }}</span>
                    </div>
                    <div class="flex gap-2 pl-6">
                      <Button
                        size="sm"
                        :disabled="device.starting || device.completing || !device.verification_uri_complete"
                        @click="openDeviceVerificationUrl"
                      >
                        <ExternalLink class="w-3 h-3 mr-1" />
                        {{ legacyT('打开') }}
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        :disabled="device.starting || device.completing || !device.verification_uri_complete"
                        @click="copyToClipboard(device.verification_uri_complete)"
                      >
                        <Copy class="w-3 h-3 mr-1" />
                        {{ legacyT('复制') }}
                      </Button>
                    </div>
                  </div>

                  <div class="flex min-h-0 flex-1 flex-col gap-2">
                    <div class="flex items-center gap-2">
                      <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">2</span>
                      <span class="text-xs font-medium">{{ legacyT('粘贴回调 URL') }}</span>
                    </div>
                    <div class="min-h-0 flex-1 pl-6">
                      <Textarea
                        v-model="device.callback_url"
                        :disabled="device.completing"
                        :placeholder="deviceCallbackPlaceholder"
                        class="h-full min-h-0 overflow-y-auto text-xs font-mono break-all !rounded-xl"
                        spellcheck="false"
                      />
                    </div>
                  </div>
                </div>

                <!-- Builder ID / Identity Center: 等待用户授权 -->
                <div
                  v-else-if="device.session_id && device.status === 'pending'"
                  class="rounded-xl border border-border bg-muted/20 p-5"
                >
                  <div class="flex flex-col items-center text-center space-y-4">
                    <div class="relative">
                      <div class="absolute inset-0 rounded-full bg-primary/20 animate-ping" />
                      <div class="relative w-10 h-10 rounded-full bg-primary/10 flex items-center justify-center">
                        <ExternalLink class="w-5 h-5 text-primary" />
                      </div>
                    </div>

                    <div class="space-y-1">
                      <p class="text-sm font-medium">
                        {{ legacyT('在浏览器中完成授权') }}
                      </p>
                      <p class="text-xs text-muted-foreground">
                        {{ legacyT('授权完成后此页面将自动更新') }}
                      </p>
                    </div>

                    <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                      <div class="animate-spin rounded-full h-3 w-3 border-[1.5px] border-primary/30 border-t-primary" />
                      <span>{{ remainingText }}</span>
                    </div>

                    <div
                      v-if="totp.code.value"
                      class="w-full rounded-lg border border-border bg-background p-3"
                    >
                      <div class="flex items-center justify-between">
                        <div class="flex items-center gap-2">
                          <ShieldCheck class="w-3.5 h-3.5 text-primary" />
                          <span class="text-[10px] text-muted-foreground">{{ legacyT('MFA 验证码') }}</span>
                        </div>
                        <div class="flex items-center gap-1.5">
                          <span
                            class="text-lg font-mono font-bold tracking-[0.25em]"
                          >{{ totp.code.value }}</span>
                          <button
                            class="p-1 rounded hover:bg-muted transition-colors"
                            :title="legacyT('复制验证码')"
                            @click="copyToClipboard(totp.code.value)"
                          >
                            <Copy class="w-3 h-3 text-muted-foreground" />
                          </button>
                        </div>
                      </div>
                      <div class="mt-2 flex items-center gap-2">
                        <div class="flex-1 h-1 rounded-full bg-muted overflow-hidden">
                          <div
                            class="h-full rounded-full transition-all duration-1000 ease-linear"
                            :class="totp.remaining.value <= 5 ? 'bg-red-500' : 'bg-primary'"
                            :style="{ width: `${(totp.remaining.value / 30) * 100}%` }"
                          />
                        </div>
                        <span
                          class="text-[10px] font-mono tabular-nums shrink-0"
                          :class="totp.remaining.value <= 5 ? 'text-red-500' : 'text-muted-foreground'"
                        >{{ totp.remaining.value }}s</span>
                      </div>
                    </div>

                    <div class="flex gap-2 w-full">
                      <Button
                        class="flex-1"
                        size="sm"
                        @click="openDeviceVerificationUrl"
                      >
                        <ExternalLink class="w-3.5 h-3.5 mr-1.5" />
                        {{ legacyT('打开授权页面') }}
                      </Button>
                      <Button
                        size="sm"
                        variant="outline"
                        @click="copyToClipboard(device.verification_uri_complete)"
                      >
                        <Copy class="w-3.5 h-3.5" />
                      </Button>
                    </div>
                  </div>
                </div>

                <!-- 初始状态：当前类型配置 -->
                <div
                  v-else
                  :class="device.auth_type === 'builder_id' ? 'flex h-full flex-col justify-center gap-4' : 'space-y-3'"
                >
                  <p
                    v-if="isSocialDeviceAuth"
                    class="text-xs text-muted-foreground text-center"
                  >
                    {{ legacyT('授权后复制浏览器地址栏的 localhost 回调 URL。') }}
                  </p>

                  <p
                    v-else-if="device.auth_type === 'builder_id'"
                    class="text-xs text-muted-foreground text-center"
                  >
                    {{ legacyT('使用个人 AWS Builder ID 进行设备授权，无需额外配置。') }}
                  </p>

                  <div
                    v-else
                    class="space-y-3"
                  >
                    <div class="space-y-1.5">
                      <label class="text-xs font-medium">Start URL</label>
                      <input
                        v-model="device.start_url"
                        type="text"
                        placeholder="https://your-org.awsapps.com/start"
                        class="w-full h-8 px-2 text-xs rounded-md border border-border bg-background font-mono focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                        spellcheck="false"
                      >
                    </div>
                    <div class="space-y-1.5">
                      <label class="text-xs font-medium">Region</label>
                      <ComboboxRoot
                        :model-value="device.region"
                        :open="regionComboboxOpen"
                        @update:model-value="(v: string) => { if (v) device.region = v }"
                        @update:open="(v: boolean) => { regionComboboxOpen = v; if (v) ensureAwsRegions() }"
                      >
                        <ComboboxAnchor class="relative w-full">
                          <ComboboxInput
                            :display-value="() => device.region"
                            :placeholder="legacyT('输入或选择 Region')"
                            class="w-full h-8 px-2 pr-7 text-xs rounded-md border border-border bg-background font-mono focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                            spellcheck="false"
                            @input="(e: Event) => regionSearch = (e.target as HTMLInputElement).value"
                            @keydown.enter.prevent="onRegionEnter"
                          />
                          <ComboboxTrigger class="absolute right-1.5 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground">
                            <ChevronsUpDown class="w-3.5 h-3.5" />
                          </ComboboxTrigger>
                        </ComboboxAnchor>
                        <ComboboxContent
                          position="popper"
                          class="z-[99] mt-1 max-h-[200px] w-[--radix-combobox-trigger-width] overflow-y-auto rounded-md border border-border bg-popover shadow-md"
                        >
                          <ComboboxViewport>
                            <ComboboxEmpty class="px-2 py-1.5 text-xs text-muted-foreground">
                              {{ awsRegionsLoaded ? legacyT('无匹配结果，回车使用自定义值') : legacyT('加载中...') }}
                            </ComboboxEmpty>
                            <ComboboxItem
                              v-for="r in filteredRegions"
                              :key="r"
                              :value="r"
                              class="flex items-center gap-1.5 px-2 py-1.5 text-xs font-mono cursor-pointer rounded-sm outline-none data-[highlighted]:bg-accent data-[highlighted]:text-accent-foreground"
                            >
                              <Check
                                class="w-3 h-3 shrink-0"
                                :class="device.region === r ? 'opacity-100' : 'opacity-0'"
                              />
                              {{ r }}
                            </ComboboxItem>
                          </ComboboxViewport>
                        </ComboboxContent>
                      </ComboboxRoot>
                    </div>
                    <div class="space-y-1.5">
                      <label class="text-xs font-medium text-muted-foreground">{{ legacyT('TOTP Secret (可选, 2FA认证)') }}</label>
                      <input
                        v-model="device.totp_secret"
                        type="text"
                        placeholder="Base32 secret, 如 JBSWY3DPEHPK3PXP"
                        class="w-full h-8 px-2 text-xs rounded-md border border-border bg-background font-mono focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                        spellcheck="false"
                      >
                    </div>
                  </div>

                  <Button
                    class="w-full"
                    :disabled="device.starting || (device.auth_type === 'identity_center' && !device.start_url.trim())"
                    @click="startDeviceAuth"
                  >
                    {{ device.starting ? legacyT('正在准备授权...') : legacyT('开始授权') }}
                  </Button>
                </div>
              </div>
            </div>
          </template>

          <!-- 非 Kiro: 原有 OAuth 流程 -->
          <template v-else>
            <div
              v-if="oauth.starting && !oauth.authorization_url"
              class="flex items-center justify-center py-12"
            >
              <div class="text-center">
                <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-primary mx-auto mb-3" />
                <p class="text-xs text-muted-foreground">
                  {{ legacyT('正在准备授权...') }}
                </p>
              </div>
            </div>

            <template v-else-if="oauth.authorization_url">
              <div class="space-y-2">
                <div class="flex items-center gap-2">
                  <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">1</span>
                  <span class="text-xs font-medium">{{ legacyT('前往授权') }}</span>
                </div>
                <div class="flex gap-2 pl-6">
                  <Button
                    size="sm"
                    :disabled="oauthBusy"
                    @click="openAuthorizationUrl"
                  >
                    <ExternalLink class="w-3 h-3 mr-1" />
                    {{ legacyT('打开') }}
                  </Button>
                  <Button
                    size="sm"
                    variant="outline"
                    :disabled="oauthBusy"
                    @click="copyToClipboard(oauth.authorization_url)"
                  >
                    <Copy class="w-3 h-3 mr-1" />
                    {{ legacyT('复制') }}
                  </Button>
                </div>
              </div>

              <div class="space-y-2">
                <div class="flex items-center gap-2">
                  <span class="flex items-center justify-center w-4 h-4 rounded-full bg-primary/10 text-primary text-[10px] font-semibold shrink-0">2</span>
                  <span class="text-xs font-medium">{{ legacyT('粘贴回调 URL') }}</span>
                </div>
                <div class="pl-6">
                  <Textarea
                    v-model="oauth.callback_url"
                    :disabled="oauthBusy"
                    placeholder="http://localhost:xxx/callback?code=..."
                    class="min-h-[120px] text-xs font-mono break-all !rounded-xl"
                    spellcheck="false"
                  />
                </div>
              </div>
            </template>
          </template>
        </div>

        <!-- ===== 导入授权 ===== -->
        <div
          class="flex flex-col gap-3 justify-center transition-opacity duration-150"
          :class="mode === 'import' ? 'opacity-100' : 'opacity-0 pointer-events-none'"
        >
          <div
            v-if="isWindsurfProvider"
            class="grid grid-cols-2 gap-1.5 rounded-lg border border-border p-0.5 bg-muted/30"
          >
            <button
              v-for="method in ([
                { key: 'email_password', label: '邮箱密码' },
                { key: 'token_json', label: 'Token / JSON' },
              ] as const)"
              :key="method.key"
              class="h-8 text-xs font-medium rounded-md transition-colors"
              :class="windsurfImportMethod === method.key
                ? 'bg-background text-foreground shadow-sm'
                : 'text-muted-foreground hover:text-foreground'"
              :disabled="importing"
              @click="setWindsurfImportMethod(method.key)"
            >
              {{ legacyT(method.label) }}
            </button>
          </div>

          <div
            v-if="isWindsurfEmailPasswordImport"
            class="space-y-3"
          >
            <div class="space-y-1.5">
              <label class="text-xs font-medium">{{ legacyT('邮箱') }}</label>
              <input
                v-model="windsurfEmail"
                type="email"
                autocomplete="username"
                :disabled="importing"
                placeholder="you@example.com"
                class="w-full h-9 px-2.5 text-xs rounded-md border border-border bg-background focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                spellcheck="false"
              >
            </div>
            <div class="space-y-1.5">
              <label class="text-xs font-medium">{{ legacyT('密码') }}</label>
              <input
                v-model="windsurfPassword"
                type="password"
                autocomplete="current-password"
                :disabled="importing"
                :placeholder="legacyT('Windsurf 密码')"
                class="w-full h-9 px-2.5 text-xs rounded-md border border-border bg-background focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
              >
            </div>
            <div class="space-y-1.5">
              <label class="text-xs font-medium text-muted-foreground">{{ legacyT('名称（可选）') }}</label>
              <input
                v-model="windsurfAccountName"
                type="text"
                autocomplete="off"
                :disabled="importing"
                :placeholder="legacyT('未填写时使用邮箱')"
                class="w-full h-9 px-2.5 text-xs rounded-md border border-border bg-background focus:outline-none focus:ring-1 focus:ring-ring focus:relative focus:z-10"
                spellcheck="false"
              >
            </div>
          </div>

          <JsonImportInput
            v-else
            v-model="importText"
            :disabled="importing"
            :reset-key="importInputResetKey"
            :drop-title="importDropTitle"
            :drop-hint="importDropHint"
            :manual-placeholder="importManualPlaceholder"
            :manual-description="importManualDescription"
            :paste-toggle-text="importPasteToggleText"
            :file-toggle-text="importFileToggleText"
            textarea-class="min-h-[200px] text-xs font-mono break-all !rounded-xl"
            @error="handleImportInputError"
          />

          <div
            v-if="importTask && !isWindsurfEmailPasswordImport"
            class="rounded-xl border border-border bg-muted/20 p-3 space-y-2"
          >
            <div class="flex items-center justify-between text-xs">
              <span class="font-medium">
                {{ getImportTaskStatusText(importTask.status) }}
              </span>
              <span class="font-mono tabular-nums">
                {{ importTask.progress_percent }}%
              </span>
            </div>
            <div class="h-1.5 rounded-full bg-muted overflow-hidden">
              <div
                class="h-full rounded-full bg-primary transition-all duration-300"
                :style="{ width: `${Math.max(0, Math.min(importTask.progress_percent, 100))}%` }"
              />
            </div>
            <div class="flex items-center justify-between text-[11px] text-muted-foreground">
              <span>{{ importProgressText(importTask) }}</span>
              <span>{{ importResultSummaryText(importTask) }}</span>
            </div>
            <p
              v-if="importTaskMessageText"
              class="text-[11px] text-muted-foreground"
            >
              {{ importTaskMessageText }}
            </p>
            <div
              v-if="importTask.error_samples.length > 0"
              class="space-y-1"
            >
              <p class="text-[11px] text-destructive">
                {{ legacyT('最近错误') }}
              </p>
              <p
                v-for="item in importTask.error_samples.slice(0, 3)"
                :key="`${item.index}-${item.error || item.status}`"
                class="text-[11px] text-destructive/90"
              >
                {{ importErrorSampleText(item) }}
              </p>
            </div>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="handleClose"
      >
        {{ legacyT('取消') }}
      </Button>
      <Button
        v-if="mode === 'oauth' && showAuthorizationMode && !isDeviceBrowserProvider"
        :disabled="!canCompleteOAuth"
        @click="handleCompleteOAuth"
      >
        {{ oauth.completing ? legacyT('验证中...') : legacyT('验证') }}
      </Button>
      <Button
        v-if="mode === 'oauth' && isManualDeviceCallbackMode"
        :disabled="!canCompleteDeviceAuth"
        @click="completeDeviceAuth"
      >
        {{ device.completing ? legacyT('验证中...') : legacyT('验证') }}
      </Button>
      <Button
        v-if="mode === 'import'"
        :disabled="!canImport"
        @click="handleImport"
      >
        {{ importButtonText }}
      </Button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref, computed, watch, onBeforeUnmount } from 'vue'
import { Dialog, Button, Textarea, Popover, PopoverTrigger, PopoverContent } from '@/components/ui'
import {
  ComboboxAnchor,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxRoot,
  ComboboxTrigger,
  ComboboxViewport,
} from 'radix-vue'
import { UserPlus, Copy, ExternalLink, Globe, AlertCircle, ShieldCheck, ChevronsUpDown, Check } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { useClipboard } from '@/composables/useClipboard'
import { useTotp } from '@/composables/useTotp'
import { parseApiError } from '@/utils/errorParser'
import { useI18n } from '@/i18n'
import {
  startProviderLevelOAuth,
  completeProviderLevelOAuth,
  importProviderRefreshToken,
  startBatchImportOAuthTask,
  getBatchImportOAuthTaskStatus,
  startDeviceAuthorize,
  pollDeviceAuthorize,
  normalizeBatchImportCredentials,
  getAwsRegions,
} from '@/api/endpoints'
import type {
  OAuthBatchImportTaskStatus,
  OAuthBatchImportTaskStatusResponse,
} from '@/api/endpoints/provider_oauth'
import ProxyNodeSelect from './ProxyNodeSelect.vue'
import { useProxyNodesStore } from '@/stores/proxy-nodes'
import JsonImportInput from '@/components/common/JsonImportInput.vue'

const props = defineProps<{
  open: boolean
  providerId: string | null
  providerType: string | null
}>()

const emit = defineEmits<{
  close: []
  saved: []
}>()

const { success, error: showError } = useToast()
const { copyToClipboard } = useClipboard()
const { legacyT, locale } = useI18n()
const proxyNodesStore = useProxyNodesStore()
const totp = useTotp()

// 代理节点选择
const proxyPopoverOpen = ref(false)
const selectedProxyNodeId = ref('')

// AWS Regions (动态获取 + 进程内缓存)
const awsRegions = ref<string[]>([])
const awsRegionsLoaded = ref(false)
const regionSearch = ref('')
const regionComboboxOpen = ref(false)

const filteredRegions = computed(() => {
  const q = regionSearch.value.trim().toLowerCase()
  if (!q) return awsRegions.value
  return awsRegions.value.filter(r => r.includes(q))
})

async function ensureAwsRegions() {
  if (awsRegionsLoaded.value) return
  try {
    awsRegions.value = await getAwsRegions()
  } catch {
    awsRegions.value = ['us-east-1', 'us-east-2', 'us-west-1', 'us-west-2', 'eu-north-1']
  }
  awsRegionsLoaded.value = true
}

function onRegionEnter() {
  // If no matching item is highlighted, accept the raw input as a custom region value.
  const raw = regionSearch.value.trim()
  if (raw && !filteredRegions.value.includes(raw)) {
    device.value.region = raw
    regionComboboxOpen.value = false
    regionSearch.value = ''
  }
}

/** 获取已选代理节点的显示名称 */
function getSelectedNodeLabel(): string {
  if (!selectedProxyNodeId.value) return ''
  const node = proxyNodesStore.nodes.find(n => n.id === selectedProxyNodeId.value)
  return node ? node.name : `${selectedProxyNodeId.value.slice(0, 8)  }...`
}

function isEnglishLocale(): boolean {
  return locale.value === 'en-US'
}

function localizedApiError(error: unknown, fallback: string): string {
  return legacyT(parseApiError(error, fallback))
}

// 模式
type DialogMode = 'oauth' | 'import'
const mode = ref<DialogMode>((props.providerType || '').toLowerCase() === 'grok' ? 'import' : 'oauth')
type WindsurfImportMethod = 'email_password' | 'token_json'

// OAuth 状态
interface OAuthState {
  authorization_url: string
  redirect_uri: string
  instructions: string
  provider_type: string
  callback_url: string
  starting: boolean
  completing: boolean
}

function createInitialOAuthState(): OAuthState {
  return {
    authorization_url: '',
    redirect_uri: '',
    instructions: '',
    provider_type: '',
    callback_url: '',
    starting: false,
    completing: false,
  }
}

const oauth = ref<OAuthState>(createInitialOAuthState())
let oauthInitRequestId = 0
let oauthCompleteRequestId = 0

// 设备授权状态
type DeviceAuthType = 'default' | 'google' | 'github' | 'builder_id' | 'identity_center'
type WindsurfLoginOption = 'default' | 'google' | 'github'

interface DeviceAuthState {
  auth_type: DeviceAuthType
  start_url: string
  region: string
  totp_secret: string
  callback_url: string
  callback_required: boolean
  starting: boolean
  completing: boolean
  session_id: string
  user_code: string
  verification_uri: string
  verification_uri_complete: string
  expires_at: number  // unix timestamp (ms)
  interval: number    // 轮询间隔 (秒)
  status: 'idle' | 'pending' | 'authorized' | 'expired' | 'error'
  error: string
}

const BUILDER_ID_START_URL = 'https://view.awsapps.com/start'
const BUILDER_ID_REGION = 'us-east-1'

function createInitialDeviceState(): DeviceAuthState {
  return {
    auth_type: 'google',
    start_url: '',
    region: 'eu-north-1',
    totp_secret: '',
    callback_url: '',
    callback_required: false,
    starting: false,
    completing: false,
    session_id: '',
    user_code: '',
    verification_uri: '',
    verification_uri_complete: '',
    expires_at: 0,
    interval: 5,
    status: 'idle',
    error: '',
  }
}

const device = ref<DeviceAuthState>(createInitialDeviceState())
let deviceAuthRequestId = 0
let devicePollTimer: ReturnType<typeof setTimeout> | null = null
const deviceCountdown = ref(0)
let countdownTimer: ReturnType<typeof setInterval> | null = null

// 导入状态
const importText = ref('')
const importing = ref(false)
const importInputResetKey = ref(0)
const importTask = ref<OAuthBatchImportTaskStatusResponse | null>(null)
let importPollTimer: ReturnType<typeof setTimeout> | null = null
const importPolling = ref(false)
const redundantImportTaskMessagePattern = /^处理中(?:\s+\d+\s*\/\s*\d+)?$/
const windsurfImportMethod = ref<WindsurfImportMethod>('email_password')
const windsurfEmail = ref('')
const windsurfPassword = ref('')
const windsurfAccountName = ref('')

const isOpen = computed(() => props.open)

const isKiroProvider = computed(() => (props.providerType || '').toLowerCase() === 'kiro')
const isGrokProvider = computed(() => (props.providerType || '').toLowerCase() === 'grok')
const isWindsurfProvider = computed(() => (props.providerType || '').toLowerCase() === 'windsurf')
const isDeviceBrowserProvider = computed(() => isKiroProvider.value || isWindsurfProvider.value)
const showAuthorizationMode = computed(() => !isGrokProvider.value)
const defaultMode = computed<DialogMode>(() => (isGrokProvider.value ? 'import' : 'oauth'))

const isSocialDeviceAuth = computed(() =>
  device.value.auth_type === 'google' || device.value.auth_type === 'github'
)

const isKiroSocialManualCallbackMode = computed(() =>
  isKiroProvider.value && isSocialDeviceAuth.value
)

const isManualDeviceCallbackMode = computed(() =>
  isKiroSocialManualCallbackMode.value || isWindsurfProvider.value
)

const isManualDeviceCallbackPending = computed(() =>
  isManualDeviceCallbackMode.value
  && device.value.session_id.length > 0
  && device.value.status === 'pending'
)

const authorizationModeLabel = computed(() => {
  if (isWindsurfProvider.value) return legacyT('浏览器登录')
  if (isDeviceBrowserProvider.value) return legacyT('设备授权')
  return legacyT('获取授权')
})

const deviceCallbackPlaceholder = computed(() =>
  isWindsurfProvider.value
    ? legacyT('粘贴包含 token=...&state=... 的回调 URL；session token/apiKey 也可直接粘贴，普通 token 请用导入授权')
    : `http://localhost:49153/oauth/callback?login_option=${device.value.auth_type}&code=...&state=...`
)

const deviceCountdownFormatted = computed(() => {
  const s = deviceCountdown.value
  const min = Math.floor(s / 60)
  const sec = s % 60
  return `${min}:${String(sec).padStart(2, '0')}`
})

const sessionRemainingText = computed(() => (
  isEnglishLocale()
    ? `Session remaining ${deviceCountdownFormatted.value}`
    : `会话剩余 ${deviceCountdownFormatted.value}`
))

const remainingText = computed(() => (
  isEnglishLocale()
    ? `${deviceCountdownFormatted.value} remaining`
    : `剩余 ${deviceCountdownFormatted.value}`
))

const oauthBusy = computed(() =>
  oauth.value.starting || oauth.value.completing
)

const canCompleteOAuth = computed(() => {
  if (!oauth.value.authorization_url) return false
  if (!oauth.value.callback_url.trim()) return false
  return !oauthBusy.value
})

const canCompleteDeviceAuth = computed(() => {
  if (!isManualDeviceCallbackPending.value) return false
  if (!device.value.callback_url.trim()) return false
  return !device.value.starting && !device.value.completing
})

const canImport = computed(() => {
  if (isWindsurfEmailPasswordImport.value) {
    return windsurfEmail.value.trim().length > 0
      && windsurfPassword.value.trim().length > 0
      && !importing.value
  }
  return importText.value.trim().length > 0 && !importing.value
})

const importModeLabel = computed(() => legacyT(isGrokProvider.value ? '导入账号' : '导入授权'))
const importButtonLabel = computed(() => legacyT(isGrokProvider.value ? '导入账号' : '导入'))
const importDropTitle = computed(() => (
  legacyT(isGrokProvider.value ? '拖入 Grok 账号文件或点击选择' : '拖入授权文件或点击选择')
))
const importDropHint = computed(() => (
  legacyT(isGrokProvider.value ? '支持 .json / .txt，可多选、批量导入' : '支持 .json / .txt，可多选')
))
const importManualPlaceholder = computed(() => (
  isGrokProvider.value
    ? legacyT('粘贴 Grok sso/session token，支持每行一个；或粘贴包含 token、sso_token、access_token、plan_type、pool_tier 的 JSON')
    : isWindsurfProvider.value
      ? legacyT('粘贴 show-auth-token Token、API key 或 JSON 内容')
      : legacyT('粘贴 Refresh Token / Access Token 或 JSON 内容')
))
const importManualDescription = computed(() => (
  isGrokProvider.value
    ? legacyT('plan_type / pool_tier 会作为账号套餐与能力特征保存，不是路由池选择。')
    : ''
))
const importPasteToggleText = computed(() => (
  legacyT(isGrokProvider.value ? '或手动粘贴 Grok Token' : '或手动粘贴 Token')
))
const importFileToggleText = computed(() => (
  legacyT(isGrokProvider.value ? '或选择 Grok Token 文件导入' : '或选择 JSON 文件导入')
))
const proxyUsageDescription = computed(() => {
  if (isEnglishLocale()) {
    return isGrokProvider.value
      ? 'Import, refresh, and quota queries use this proxy'
      : 'Authorization, refresh, and quota queries use this proxy'
  }
  return isGrokProvider.value
    ? '导入、刷新、额度查询均走此代理'
    : '授权、刷新、额度查询均走此代理'
})
const isWindsurfEmailPasswordImport = computed(() =>
  isWindsurfProvider.value && windsurfImportMethod.value === 'email_password'
)

const importButtonText = computed(() => {
  if (importing.value) {
    return importTask.value && !isWindsurfEmailPasswordImport.value
      ? (isEnglishLocale() ? `Importing ${importTask.value.progress_percent}%` : `导入中 ${importTask.value.progress_percent}%`)
      : legacyT('导入中...')
  }
  return isWindsurfEmailPasswordImport.value ? legacyT('登录并导入') : importButtonLabel.value
})

const importTaskMessageText = computed(() => {
  const message = importTask.value?.message?.trim()
  if (!message) return ''
  // 后端进度 message 已由“进度 x/y”展示，避免在导入中重复显示“处理中 x/y”。
  return redundantImportTaskMessagePattern.test(message) ? '' : legacyT(message)
})

function importProgressText(task: OAuthBatchImportTaskStatusResponse): string {
  return isEnglishLocale()
    ? `Progress ${task.processed}/${task.total}`
    : `进度 ${task.processed}/${task.total}`
}

function importResultSummaryText(task: OAuthBatchImportTaskStatusResponse): string {
  return isEnglishLocale()
    ? `Success ${task.success} · Failed ${task.failed}`
    : `成功 ${task.success} · 失败 ${task.failed}`
}

function importErrorSampleText(item: OAuthBatchImportTaskStatusResponse['error_samples'][number]): string {
  return `#${item.index + 1} ${legacyT(item.error || '导入失败')}`
}

function stopImportPolling() {
  if (importPollTimer) {
    clearTimeout(importPollTimer)
    importPollTimer = null
  }
  importPolling.value = false
}

function getImportTaskStatusText(status: OAuthBatchImportTaskStatus): string {
  switch (status) {
    case 'submitted':
      return legacyT('任务已提交')
    case 'processing':
      return legacyT('正在导入')
    case 'completed':
      return legacyT('导入完成')
    case 'failed':
      return legacyT('导入失败')
    default:
      return legacyT('处理中')
  }
}

function getOAuthSuccessMessage(
  action: '授权' | '导入',
  options?: { email?: string | null; replaced?: boolean }
): string {
  const email = typeof options?.email === 'string' ? options.email.trim() : ''
  const replaced = options?.replaced === true
  const actionText = legacyT(action)

  if (isEnglishLocale()) {
    if (email) {
      return replaced
        ? `${actionText} succeeded: ${email} (replaced existing account)`
        : `${actionText} succeeded: ${email}`
    }
    return replaced
      ? `${actionText} succeeded; replaced existing account`
      : `${actionText} succeeded; account added`
  }

  if (email) {
    return replaced
      ? `${action}成功: ${email}（已替换旧账号）`
      : `${action}成功: ${email}`
  }
  return replaced
    ? `${action}成功，已替换旧账号`
    : `${action}成功，账号已添加`
}

function getBatchImportSuccessMessage(task: OAuthBatchImportTaskStatusResponse): string {
  const replacedCount = Math.max(task.replaced_count ?? 0, 0)
  const createdCount = Math.max(task.created_count ?? task.success - replacedCount, 0)
  const parts: string[] = []

  if (createdCount > 0) {
    parts.push(isEnglishLocale() ? `${createdCount} added` : `新增 ${createdCount} 个`)
  }
  if (replacedCount > 0) {
    parts.push(isEnglishLocale() ? `${replacedCount} replaced` : `替换 ${replacedCount} 个`)
  }
  if (task.failed > 0) {
    parts.push(isEnglishLocale() ? `${task.failed} failed` : `失败 ${task.failed} 个`)
  }

  if (parts.length === 0) {
    if (isEnglishLocale()) {
      return task.failed > 0 ? `Batch import complete: ${task.failed} failed` : 'Batch import complete'
    }
    return task.failed > 0 ? `批量导入完成：失败 ${task.failed} 个` : '批量导入完成'
  }
  if (task.failed === 0 && createdCount > 0 && replacedCount === 0) {
    if (isEnglishLocale()) return `Batch import succeeded: ${createdCount} accounts added`
    return `批量导入成功：${createdCount} 个账号已添加`
  }
  if (task.failed === 0 && createdCount === 0 && replacedCount > 0) {
    if (isEnglishLocale()) return `Batch import succeeded: ${replacedCount} existing accounts replaced`
    return `批量导入成功：已替换 ${replacedCount} 个旧账号`
  }

  const prefix = task.failed > 0
    ? legacyT('批量导入完成')
    : legacyT('批量导入成功')
  return isEnglishLocale()
    ? `${prefix}: ${parts.join(', ')}`
    : `${prefix}：${parts.join('，')}`
}

function scheduleImportPoll(taskId: string, delayMs = 1200) {
  stopImportPolling()
  importPollTimer = setTimeout(() => {
    void pollImportTaskStatus(taskId)
  }, delayMs)
}

async function pollImportTaskStatus(taskId: string) {
  if (!props.providerId || importPolling.value) return

  importPolling.value = true
  try {
    const task = await getBatchImportOAuthTaskStatus(props.providerId, taskId)
    importTask.value = task

    if (task.status === 'completed') {
      stopImportPolling()
      importing.value = false
      if (task.success > 0) {
        success(getBatchImportSuccessMessage(task))
        emit('saved')
        handleClose()
      } else {
        showError(legacyT(task.error || '批量导入失败'), legacyT('导入失败'))
      }
      return
    }

    if (task.status === 'failed') {
      stopImportPolling()
      importing.value = false
      showError(legacyT(task.error || task.message || '批量导入失败'), legacyT('导入失败'))
      return
    }

    scheduleImportPoll(taskId)
  } catch {
    if (importing.value) {
      scheduleImportPoll(taskId, 2000)
    }
  } finally {
    importPolling.value = false
  }
}

function stopDevicePolling() {
  if (devicePollTimer) {
    clearTimeout(devicePollTimer)
    devicePollTimer = null
  }
  if (countdownTimer) {
    clearInterval(countdownTimer)
    countdownTimer = null
  }
}

function resetDeviceRuntimeState() {
  stopDevicePolling()
  totp.stop()
  device.value.callback_url = ''
  device.value.callback_required = false
  device.value.starting = false
  device.value.completing = false
  device.value.session_id = ''
  device.value.user_code = ''
  device.value.verification_uri = ''
  device.value.verification_uri_complete = ''
  device.value.expires_at = 0
  device.value.interval = 5
  device.value.status = 'idle'
  device.value.error = ''
}

function isKiroDeviceAuthOptionDisabled(_authType: DeviceAuthType): boolean {
  if (!isKiroProvider.value) return false
  if (device.value.starting) {
    return !isSocialDeviceAuth.value
  }
  if (!device.value.session_id) return false
  if (isSocialDeviceAuth.value && device.value.status === 'pending') {
    return false
  }
  return true
}

function selectWindsurfLoginOption(loginOption: WindsurfLoginOption) {
  if (!isWindsurfProvider.value) return
  if (device.value.auth_type === loginOption && device.value.session_id && device.value.status === 'pending') return
  deviceAuthRequestId += 1
  resetDeviceRuntimeState()
  device.value.auth_type = loginOption
}

function selectDeviceAuthType(authType: DeviceAuthType) {
  if (device.value.auth_type === authType) return
  if (isKiroDeviceAuthOptionDisabled(authType)) return

  deviceAuthRequestId += 1
  resetDeviceRuntimeState()
  device.value.auth_type = authType
  if (authType === 'google' || authType === 'github') {
    void ensureKiroSocialDeviceAuth()
  }
}

function resetDevice() {
  deviceAuthRequestId += 1
  stopDevicePolling()
  totp.stop()
  const { auth_type, start_url, region, totp_secret } = device.value
  device.value = createInitialDeviceState()
  device.value.auth_type = isWindsurfProvider.value ? (auth_type === 'google' || auth_type === 'github' ? auth_type : 'default') : auth_type
  device.value.start_url = start_url
  device.value.region = region
  device.value.totp_secret = totp_secret
  if (!isWindsurfProvider.value && (device.value.auth_type === 'google' || device.value.auth_type === 'github')) {
    void ensureKiroSocialDeviceAuth()
  }
}

function resetForm() {
  oauthInitRequestId += 1
  oauthCompleteRequestId += 1
  deviceAuthRequestId += 1
  oauth.value = createInitialOAuthState()
  stopImportPolling()
  stopDevicePolling()
  totp.stop()
  device.value = createInitialDeviceState()
  if (isWindsurfProvider.value) {
    device.value.auth_type = 'default'
  }
  importText.value = ''
  importing.value = false
  importTask.value = null
  importInputResetKey.value += 1
  windsurfImportMethod.value = 'email_password'
  windsurfEmail.value = ''
  windsurfPassword.value = ''
  windsurfAccountName.value = ''
  proxyPopoverOpen.value = false
  selectedProxyNodeId.value = ''
  mode.value = defaultMode.value
}

function switchMode(newMode: DialogMode) {
  if (mode.value === newMode) return
  if (newMode === 'oauth' && !showAuthorizationMode.value) return

  mode.value = newMode
  if (newMode === 'oauth') {
    if (isKiroProvider.value) {
      void ensureKiroSocialDeviceAuth()
    } else if (!oauth.value.authorization_url && !oauth.value.starting) {
      initOAuth()
    }
  }
}

function handleDialogUpdate(value: boolean) {
  if (!value) {
    handleClose()
  }
}

function handleClose() {
  resetForm()
  emit('close')
}

function openAuthorizationUrl() {
  const url = oauth.value.authorization_url
  if (!url) return
  window.open(url, '_blank', 'noopener,noreferrer')
}

async function initOAuth() {
  if (!props.providerId) return
  if (!showAuthorizationMode.value) return
  if (isDeviceBrowserProvider.value) return
  if (oauth.value.starting) return

  const requestId = ++oauthInitRequestId
  oauth.value.starting = true
  try {
    const resp = await startProviderLevelOAuth(props.providerId)
    if (requestId !== oauthInitRequestId) return
    oauth.value.authorization_url = resp.authorization_url
    oauth.value.redirect_uri = resp.redirect_uri
    oauth.value.instructions = resp.instructions
    oauth.value.provider_type = resp.provider_type
  } catch (err: unknown) {
    if (requestId !== oauthInitRequestId) return
    const errorMessage = localizedApiError(err, '初始化授权失败')
    showError(errorMessage, legacyT('错误'))
    mode.value = 'import'
  } finally {
    if (requestId === oauthInitRequestId) {
      oauth.value.starting = false
    }
  }
}

async function handleCompleteOAuth() {
  if (oauth.value.completing) return
  if (!canCompleteOAuth.value || !props.providerId) return
  const requestId = ++oauthCompleteRequestId
  oauth.value.completing = true
  try {
    const result = await completeProviderLevelOAuth(props.providerId, {
      callback_url: oauth.value.callback_url.trim(),
      proxy_node_id: selectedProxyNodeId.value || undefined,
    })
    if (requestId !== oauthCompleteRequestId) return
    success(getOAuthSuccessMessage('授权', result))
    emit('saved')
    handleClose()
  } catch (err: unknown) {
    if (requestId !== oauthCompleteRequestId) return
    const errorMessage = localizedApiError(err, '完成授权失败')
    showError(errorMessage, legacyT('错误'))
  } finally {
    if (requestId === oauthCompleteRequestId) {
      oauth.value.completing = false
    }
  }
}

function parseImportText(text: string): {
  api_key?: string
  token?: string
  refresh_token?: string
  access_token?: string
  password?: string
  expires_at?: number
  name?: string
  email?: string
  account_id?: string
  account_user_id?: string
  plan_type?: string
  pool_tier?: string
  sso_rw_token?: string
  cf_cookies?: string
  cf_clearance?: string
  user_agent?: string
  browser_profile?: string
  user_id?: string
  account_name?: string
  headers?: Record<string, string>
} | null {
  const trimmed = text.trim()
  if (!trimmed) return null

  // Kiro: keep full JSON so backend can extract auth_method/region/client_id, etc.
  if (isKiroProvider.value) {
    return { refresh_token: trimmed }
  }

  if (isGrokProvider.value) {
    const cookieImport = parseGrokCookieImport(trimmed)
    if (cookieImport) {
      return cookieImport
    }
  }

  if (isWindsurfProvider.value) {
    try {
      const parsed: unknown = JSON.parse(trimmed)
      if (typeof parsed === 'object' && parsed !== null) {
        const obj = parsed as Record<string, unknown>
        const apiKey = normalizeStringField(obj.api_key) ?? normalizeStringField(obj.apiKey)
        const token = normalizeStringField(obj.token) ?? normalizeStringField(obj.auth_token) ?? normalizeStringField(obj.authToken)
        const refreshToken = normalizeStringField(obj.refresh_token) ?? normalizeStringField(obj.refreshToken)
        const accessToken = normalizeStringField(obj.access_token) ?? normalizeStringField(obj.accessToken)
        const email = normalizeStringField(obj.email)
        const password = normalizeStringField(obj.password)
        if (apiKey || token || refreshToken || accessToken || (email && password)) {
          return {
            api_key: apiKey,
            token,
            refresh_token: refreshToken,
            access_token: accessToken,
            email,
            password,
            name: normalizeStringField(obj.name) ?? email,
          }
        }
      }
    } catch {
      // Not JSON: treat as token copied from show-auth-token.
    }
    return { token: trimmed }
  }

  try {
    const parsed: unknown = JSON.parse(trimmed)
    if (typeof parsed === 'object' && parsed !== null) {
      const obj = parsed as Record<string, unknown>
      const grokCookieImport = isGrokProvider.value
        ? parseGrokCookieImport(normalizeStringField(obj.cookie) ?? normalizeStringField(obj.cookieHeader) ?? '')
        : null
      const refreshToken = obj.refresh_token
      const refreshTokenCamel = obj.refreshToken
      const accessToken = obj.access_token
      const accessTokenCamel = obj.accessToken
      const sessionToken = obj.session_token
      const sessionTokenCamel = obj.sessionToken
      const grokSsoToken = isGrokProvider.value
        ? normalizeStringField(obj.sso_token) ?? normalizeStringField(obj.ssoToken) ?? normalizeStringField(obj.token) ?? grokCookieImport?.access_token
        : undefined
      const normalizedRefreshToken = typeof refreshToken === 'string' && refreshToken.trim()
        ? refreshToken.trim()
        : (typeof refreshTokenCamel === 'string' && refreshTokenCamel.trim() ? refreshTokenCamel.trim() : undefined)
      const normalizedAccessToken = typeof accessToken === 'string' && accessToken.trim()
        ? accessToken.trim()
        : (typeof accessTokenCamel === 'string' && accessTokenCamel.trim() ? accessTokenCamel.trim() : undefined)
      const normalizedSessionToken = typeof sessionToken === 'string' && sessionToken.trim()
        ? sessionToken.trim()
        : (typeof sessionTokenCamel === 'string' && sessionTokenCamel.trim() ? sessionTokenCamel.trim() : undefined)
      const normalizedHeaders = normalizeHeadersField(obj.headers)
        ?? normalizeHeadersField(obj.request_headers)
        ?? normalizeHeadersField(obj.requestHeaders)
        ?? normalizeHeadersField(obj.header_overrides)
        ?? normalizeHeadersField(obj.headerOverrides)
        ?? normalizeHeadersField(obj.extra_headers)
        ?? normalizeHeadersField(obj.extraHeaders)
      const importedAccessToken = normalizedAccessToken ?? grokSsoToken ?? normalizedSessionToken ?? bearerTokenFromHeaders(normalizedHeaders)
      if (normalizedRefreshToken || importedAccessToken) {
        return {
          refresh_token: normalizedRefreshToken,
          access_token: importedAccessToken,
          expires_at: normalizeExpiryField(obj.expires_at) ?? normalizeExpiryField(obj.expiresAt) ?? normalizeExpiryField(obj.expired),
          name: (typeof obj.name === 'string' ? obj.name : undefined) || (typeof obj.oauth_email === 'string' ? obj.oauth_email : undefined),
          email: normalizeStringField(obj.email) ?? normalizeStringField(obj.oauth_email),
          account_id: normalizeStringField(obj.account_id) ?? normalizeStringField(obj.accountId) ?? normalizeStringField(obj.chatgpt_account_id) ?? normalizeStringField(obj.chatgptAccountId),
          account_user_id: normalizeStringField(obj.account_user_id) ?? normalizeStringField(obj.accountUserId) ?? normalizeStringField(obj.chatgpt_account_user_id) ?? normalizeStringField(obj.chatgptAccountUserId),
          plan_type: normalizeStringField(obj.plan_type) ?? normalizeStringField(obj.planType) ?? normalizeStringField(obj.chatgpt_plan_type) ?? normalizeStringField(obj.chatgptPlanType),
          pool_tier: isGrokProvider.value ? normalizeStringField(obj.pool_tier) ?? normalizeStringField(obj.poolTier) ?? normalizeStringField(obj.tier) : undefined,
          sso_rw_token: isGrokProvider.value ? normalizeStringField(obj.sso_rw_token) ?? normalizeStringField(obj.ssoRwToken) ?? grokCookieImport?.sso_rw_token : undefined,
          cf_cookies: isGrokProvider.value ? normalizeStringField(obj.cf_cookies) ?? normalizeStringField(obj.cfCookies) ?? grokCookieImport?.cf_cookies : undefined,
          cf_clearance: isGrokProvider.value ? normalizeStringField(obj.cf_clearance) ?? normalizeStringField(obj.cfClearance) ?? grokCookieImport?.cf_clearance : undefined,
          user_agent: isGrokProvider.value ? normalizeStringField(obj.user_agent) ?? normalizeStringField(obj.userAgent) ?? grokCookieImport?.user_agent : undefined,
          browser_profile: isGrokProvider.value ? normalizeStringField(obj.browser_profile) ?? normalizeStringField(obj.browserProfile) ?? normalizeStringField(obj.browser) ?? normalizeStringField(obj.impersonate) ?? grokCookieImport?.browser_profile : undefined,
          user_id: normalizeStringField(obj.user_id) ?? normalizeStringField(obj.userId) ?? normalizeStringField(obj.chatgpt_user_id) ?? normalizeStringField(obj.chatgptUserId),
          account_name: normalizeStringField(obj.account_name) ?? normalizeStringField(obj.accountName),
          headers: normalizedHeaders,
        }
      }
      return null
    }
  } catch {
    // Not JSON: treat as raw token.
  }

  if (isLikelyJwtToken(trimmed)) {
    return { access_token: trimmed }
  }

  return { refresh_token: trimmed }
}

function parseGrokCookieImport(text: string): {
  access_token: string
  sso_rw_token?: string
  cf_cookies?: string
  cf_clearance?: string
  user_agent?: string
  browser_profile?: string
  user_id?: string
} | null {
  const cookies = parseCookieHeader(text)
  const sso = cookies.get('sso')
  if (!sso) return null
  const userAgent = currentBrowserUserAgent()

  return {
    access_token: sso,
    sso_rw_token: cookies.get('sso-rw'),
    cf_cookies: buildGrokCookieProfile(cookies),
    cf_clearance: cookies.get('cf_clearance'),
    user_agent: userAgent,
    browser_profile: inferGrokBrowserProfile(userAgent),
    user_id: cookies.get('x-userid'),
  }
}

function currentBrowserUserAgent(): string | undefined {
  const value = typeof navigator !== 'undefined' ? navigator.userAgent?.trim() : ''
  return value || undefined
}

function inferGrokBrowserProfile(userAgent: string | undefined): string | undefined {
  const value = (userAgent || '').toLowerCase()
  if (!value) return 'chrome136'
  if (value.includes('firefox/')) return 'firefox'
  if (value.includes('safari/') && !value.includes('chrome/') && !value.includes('chromium/')) {
    return value.includes('iphone') || value.includes('ipad') ? 'safari_ios' : 'safari'
  }
  return 'chrome136'
}

function buildGrokCookieProfile(cookies: Map<string, string>): string | undefined {
  const parts: string[] = []
  for (const [name, value] of cookies) {
    if (name === 'sso' || name === 'sso-rw') continue
    parts.push(`${name}=${value}`)
  }
  return parts.length > 0 ? parts.join('; ') : undefined
}

function parseCookieHeader(text: string): Map<string, string> {
  const normalized = text.trim().replace(/^cookie:\s*/i, '')
  const cookies = new Map<string, string>()
  for (const segment of normalized.split(';')) {
    const part = segment.trim()
    if (!part) continue
    const separator = part.indexOf('=')
    if (separator <= 0) continue
    const name = part.slice(0, separator).trim().toLowerCase()
    const value = part.slice(separator + 1).trim()
    if (name && value) {
      cookies.set(name, value)
    }
  }
  return cookies
}

function normalizeStringField(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() ? value.trim() : undefined
}

function normalizeHeadersField(value: unknown): Record<string, string> | undefined {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return undefined
  const headers: Record<string, string> = {}
  for (const [rawKey, rawValue] of Object.entries(value as Record<string, unknown>)) {
    const key = rawKey.trim().toLowerCase()
    if (!key || ['host', 'content-length', 'connection', 'transfer-encoding', 'proxy-authorization'].includes(key)) {
      continue
    }
    let headerValue: string | undefined
    if (typeof rawValue === 'string') {
      headerValue = rawValue.trim()
    } else if (typeof rawValue === 'number' || typeof rawValue === 'boolean') {
      headerValue = String(rawValue)
    }
    if (headerValue) {
      headers[key] = headerValue
    }
  }
  return Object.keys(headers).length > 0 ? headers : undefined
}

function bearerTokenFromHeaders(headers: Record<string, string> | undefined): string | undefined {
  const authorization = headers?.authorization?.trim()
  if (!authorization) return undefined
  const match = authorization.match(/^bearer\s+(.+)$/i)
  return match?.[1]?.trim() || undefined
}

function normalizeNumberField(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value) && value > 0) {
    return Math.floor(value)
  }
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value.trim())
    if (Number.isFinite(parsed) && parsed > 0) {
      return Math.floor(parsed)
    }
  }
  return undefined
}

function normalizeExpiryField(value: unknown): number | undefined {
  const numeric = normalizeNumberField(value)
  if (numeric) return numeric
  if (typeof value === 'string' && value.trim()) {
    const parsed = Date.parse(value.trim())
    if (Number.isFinite(parsed) && parsed > 0) {
      return Math.floor(parsed / 1000)
    }
  }
  return undefined
}

function isLikelyJwtToken(token: string): boolean {
  const parts = token.trim().split('.')
  if (parts.length !== 3 || parts.some(part => !part)) return false

  try {
    const header = JSON.parse(decodeBase64Url(parts[0])) as Record<string, unknown>
    const payload = JSON.parse(decodeBase64Url(parts[1])) as Record<string, unknown>
    const tokenType = typeof header.typ === 'string' ? header.typ.toLowerCase() : ''
    if (tokenType && tokenType !== 'jwt' && tokenType !== 'at+jwt') return false
    return ['exp', 'aud', 'iss', 'scope', 'scp'].some(key => key in payload)
  } catch {
    return false
  }
}

function decodeBase64Url(value: string): string {
  const normalized = value.replace(/-/g, '+').replace(/_/g, '/')
  const padded = normalized.padEnd(normalized.length + ((4 - (normalized.length % 4)) % 4), '=')
  return atob(padded)
}

function handleImportInputError(payload: { message: string; title?: string }) {
  showError(legacyT(payload.message), payload.title ? legacyT(payload.title) : undefined)
}

function setWindsurfImportMethod(method: WindsurfImportMethod) {
  if (!isWindsurfProvider.value || importing.value) return
  windsurfImportMethod.value = method
  importTask.value = null
}

async function handleWindsurfEmailPasswordImport() {
  if (!props.providerId) return

  const email = windsurfEmail.value.trim()
  const password = windsurfPassword.value.trim()
  if (!email || !password) {
    showError(legacyT('请输入邮箱和密码'), legacyT('格式错误'))
    return
  }

  importing.value = true
  try {
    const result = await importProviderRefreshToken(props.providerId, {
      email,
      password,
      name: windsurfAccountName.value.trim() || email,
      proxy_node_id: selectedProxyNodeId.value || undefined,
    })
    success(getOAuthSuccessMessage('导入', result))
    emit('saved')
    handleClose()
  } catch (err: unknown) {
    const errorMessage = localizedApiError(err, '导入失败')
    showError(errorMessage, legacyT('错误'))
  } finally {
    importing.value = false
  }
}

async function handleImport() {
  if (!canImport.value || !props.providerId) return
  if (isWindsurfEmailPasswordImport.value) {
    await handleWindsurfEmailPasswordImport()
    return
  }

  const inputText = importText.value.trim()
  if (!inputText) {
    showError(legacyT('请输入凭据数据'), legacyT('格式错误'))
    return
  }

  const normalizedCredentials = normalizeBatchImportCredentials(inputText)
  if (!normalizedCredentials.ok) {
    showError(legacyT(normalizedCredentials.message), legacyT('格式错误'))
    return
  }

  importing.value = true
  let keepImporting = false
  try {
    const proxyNodeId = selectedProxyNodeId.value || undefined
    // Kiro 的单条 JSON 凭据也必须走 batch-import 路径，后端需要完整 auth_config。
    if (isKiroProvider.value || normalizedCredentials.isBatch) {
      const task = await startBatchImportOAuthTask(props.providerId, normalizedCredentials.credentials, proxyNodeId)
      importTask.value = {
        task_id: task.task_id,
        provider_id: props.providerId,
        provider_type: props.providerType || '',
        status: task.status,
        total: task.total,
        processed: task.processed,
        success: task.success,
        failed: task.failed,
        created_count: task.created_count ?? 0,
        replaced_count: task.replaced_count ?? 0,
        progress_percent: task.progress_percent,
        message: task.message || null,
        error: null,
        error_samples: [],
        created_at: Math.floor(Date.now() / 1000),
        started_at: null,
        finished_at: null,
        updated_at: Math.floor(Date.now() / 1000),
      }
      keepImporting = true
      scheduleImportPoll(task.task_id, 400)
    } else {
      // 单条导入
      const parsed = parseImportText(normalizedCredentials.credentials)
      if (!parsed) {
        showError(legacyT('无法解析输入内容，请检查格式'), legacyT('格式错误'))
        return
      }
      const result = await importProviderRefreshToken(props.providerId, {
        ...parsed,
        proxy_node_id: proxyNodeId,
      })
      success(getOAuthSuccessMessage('导入', result))
      emit('saved')
      handleClose()
    }
  } catch (err: unknown) {
    const errorMessage = localizedApiError(err, '导入失败')
    showError(errorMessage, legacyT('错误'))
  } finally {
    if (!keepImporting) {
      importing.value = false
    }
  }
}

// ==== 设备授权 ====

function openDeviceVerificationUrl() {
  const url = device.value.verification_uri_complete || device.value.verification_uri
  if (url) window.open(url, '_blank', 'noopener,noreferrer')
}

function startCountdown() {
  if (countdownTimer) clearInterval(countdownTimer)
  deviceCountdown.value = Math.max(0, Math.round((device.value.expires_at - Date.now()) / 1000))
  countdownTimer = setInterval(() => {
    deviceCountdown.value = Math.max(0, Math.round((device.value.expires_at - Date.now()) / 1000))
    if (deviceCountdown.value <= 0 && countdownTimer) {
      clearInterval(countdownTimer)
      countdownTimer = null
    }
  }, 1000)
}

async function startDeviceAuth() {
  if (!props.providerId) return
  if (device.value.starting) return
  const requestId = ++deviceAuthRequestId
  const requestedAuthType = device.value.auth_type
  device.value.callback_url = ''
  device.value.callback_required = false
  device.value.session_id = ''
  device.value.user_code = ''
  device.value.verification_uri = ''
  device.value.verification_uri_complete = ''
  device.value.status = 'idle'
  device.value.starting = true
  device.value.error = ''
  try {
    const isWindsurf = isWindsurfProvider.value
    const isBuilderID = requestedAuthType === 'builder_id'
    const isSocial = requestedAuthType === 'google' || requestedAuthType === 'github'
    const windsurfLoginOption: WindsurfLoginOption = isSocial ? requestedAuthType : 'default'
    const authTypeForRequest = isWindsurf
      ? 'browser'
      : (requestedAuthType === 'default' ? 'google' : requestedAuthType)
    const resp = await startDeviceAuthorize(props.providerId, {
      auth_type: authTypeForRequest,
      login_option: isWindsurf ? windsurfLoginOption : undefined,
      start_url: isWindsurf ? undefined : (isBuilderID ? BUILDER_ID_START_URL : (isSocial ? undefined : (device.value.start_url.trim() || undefined))),
      region: isWindsurf ? undefined : (isBuilderID || isSocial ? BUILDER_ID_REGION : (device.value.region.trim() || undefined)),
      proxy_node_id: selectedProxyNodeId.value || undefined,
    })
    if (requestId !== deviceAuthRequestId || device.value.auth_type !== requestedAuthType) return
    device.value.session_id = resp.session_id
    device.value.user_code = resp.user_code
    device.value.verification_uri = resp.verification_uri
    device.value.verification_uri_complete = resp.verification_uri_complete
    device.value.expires_at = Date.now() + resp.expires_in * 1000
    device.value.interval = resp.interval || 5
    device.value.callback_required = resp.callback_required === true || isSocial || isWindsurf
    device.value.status = 'pending'
    startCountdown()
    if (!device.value.callback_required) {
      scheduleDevicePoll()
    }
    // 如果配置了 TOTP secret，启动验证码生成
    if (!device.value.callback_required && device.value.totp_secret.trim()) {
      totp.start(device.value.totp_secret.trim())
    }
  } catch (err: unknown) {
    if (requestId !== deviceAuthRequestId || device.value.auth_type !== requestedAuthType) return
    const errorMessage = localizedApiError(err, '发起设备授权失败')
    showError(errorMessage, legacyT('错误'))
    device.value.status = 'error'
    device.value.error = errorMessage
  } finally {
    if (requestId === deviceAuthRequestId && device.value.auth_type === requestedAuthType) {
      device.value.starting = false
    }
  }
}

async function ensureKiroSocialDeviceAuth() {
  if (!props.open || !props.providerId || !isKiroProvider.value || !isSocialDeviceAuth.value) return
  if (device.value.starting) return
  if (device.value.session_id && device.value.status === 'pending') return
  await startDeviceAuth()
}

function scheduleDevicePoll() {
  if (devicePollTimer) clearTimeout(devicePollTimer)
  devicePollTimer = setTimeout(() => pollDevice(), device.value.interval * 1000)
}

async function completeDeviceAuth() {
  if (device.value.completing || !canCompleteDeviceAuth.value) return
  device.value.completing = true
  try {
    await pollDevice(true)
  } finally {
    device.value.completing = false
  }
}

function normalizeWindsurfSubmittedCredential(value: string): { callback_url?: string, token?: string } {
  const trimmed = value.trim()
  if (!trimmed) return {}
  if (/^https?:\/\//i.test(trimmed)) {
    return { callback_url: trimmed }
  }

  const query = trimmed.replace(/^[?#&]+/, '')
  const params = new URLSearchParams(query)
  const hasTokenParam = ['token', 'auth_token', 'access_token'].some(key => params.has(key))
  const hasStateParam = params.has('state')
  if (hasTokenParam && hasStateParam) {
    return { callback_url: `https://windsurf.com/show-auth-token?${query}` }
  }
  if (hasTokenParam) {
    return { token: params.get('token') || params.get('auth_token') || params.get('access_token') || trimmed }
  }

  return { token: trimmed }
}

async function pollDevice(withCallback = false) {
  if (!props.providerId || !device.value.session_id || device.value.status !== 'pending') return

  try {
    const submittedCredential = withCallback ? device.value.callback_url.trim() : ''
    const windsurfSubmitted = isWindsurfProvider.value
      ? normalizeWindsurfSubmittedCredential(submittedCredential)
      : {}
    const result = await pollDeviceAuthorize(props.providerId, {
      session_id: device.value.session_id,
      callback_url: withCallback ? (windsurfSubmitted.callback_url || (!isWindsurfProvider.value ? submittedCredential : undefined)) : undefined,
      token: withCallback ? windsurfSubmitted.token : undefined,
    })

    switch (result.status) {
      case 'authorized':
        stopDevicePolling()
        totp.stop()
        device.value.status = 'authorized'
        success(getOAuthSuccessMessage('授权', result))
        emit('saved')
        handleClose()
        return
      case 'pending':
        if (!device.value.callback_required) {
          scheduleDevicePoll()
        }
        return
      case 'slow_down':
        device.value.interval = Math.min(device.value.interval + 5, 30)
        scheduleDevicePoll()
        return
      case 'expired':
        stopDevicePolling()
        device.value.status = 'expired'
        device.value.error = result.error || '设备码已过期'
        return
      case 'error':
        stopDevicePolling()
        device.value.status = 'error'
        device.value.error = result.error || '授权失败'
        return
    }
  } catch (err: unknown) {
    if (withCallback) {
      const errorMessage = localizedApiError(err, '完成授权失败')
      showError(errorMessage, legacyT('错误'))
    }
    // 网络错误等，继续轮询
    if (!withCallback && !device.value.callback_required) {
      scheduleDevicePoll()
    }
  }
}

onBeforeUnmount(() => {
  stopImportPolling()
  stopDevicePolling()
})

watch(() => props.open, (newOpen) => {
  if (newOpen) {
    proxyNodesStore.ensureLoaded()
    mode.value = defaultMode.value
    if (!showAuthorizationMode.value) {
      return
    }
    if (isWindsurfProvider.value) {
      device.value.auth_type = 'default'
    } else if (isKiroProvider.value) {
      void ensureKiroSocialDeviceAuth()
    } else {
      initOAuth()
    }
  } else {
    resetForm()
  }
})

watch(
  () => [props.open, props.providerId, props.providerType] as const,
  () => {
    if (props.open && !showAuthorizationMode.value) {
      mode.value = 'import'
      return
    }
    if (props.open && isWindsurfProvider.value && mode.value === 'oauth') {
      device.value.auth_type = ['default', 'google', 'github'].includes(device.value.auth_type)
        ? device.value.auth_type
        : 'default'
    } else if (props.open && isKiroProvider.value && mode.value === 'oauth') {
      void ensureKiroSocialDeviceAuth()
    }
  },
)
</script>
