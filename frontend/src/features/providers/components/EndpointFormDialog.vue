<template>
  <Dialog
    :model-value="internalOpen"
    title="端点管理"
    :description="`管理 ${provider?.name} 的 API 端点`"
    :icon="Settings"
    size="2xl"
    @update:model-value="handleDialogUpdate"
  >
    <div class="flex flex-col gap-4">
      <!-- 已有端点列表（可滚动） -->
      <div
        v-if="localEndpoints.length > 0"
        class="space-y-3 max-h-[50vh] overflow-y-auto scrollbar-hide"
      >
        <Label class="text-muted-foreground">已配置的端点</Label>

        <!-- 端点卡片列表 -->
        <div class="space-y-3">
          <div
            v-for="endpoint in localEndpoints"
            :key="endpoint.id"
            class="rounded-lg border bg-card"
            :class="{ 'opacity-60': !endpoint.is_active }"
          >
            <!-- 卡片头部：格式名称 + 状态 + 操作 -->
            <div class="flex items-center justify-between px-4 py-2.5 bg-muted/30 border-b">
              <div class="flex items-center gap-3">
                <span class="font-medium">{{ formatApiFormat(endpoint.api_format) }}</span>
                <Badge
                  v-if="!endpoint.is_active"
                  variant="secondary"
                  class="text-xs"
                >
                  停用
                </Badge>
              </div>
              <div class="flex items-center gap-1.5">
                <!-- 格式转换按钮 -->
                <span
                  class="mr-1"
                  :title="isEndpointFormatConversionDisabled ? formatConversionDisabledTooltip : (endpoint.format_acceptance_config?.enabled ? '已启用格式转换（点击关闭）' : '启用格式转换')"
                >
                  <Button
                    variant="ghost"
                    size="icon"
                    :class="`h-7 w-7 ${endpoint.format_acceptance_config?.enabled ? 'text-primary' : ''} ${isEndpointFormatConversionDisabled ? 'opacity-50' : ''}`"
                    :disabled="togglingFormatEndpointId === endpoint.id || isEndpointFormatConversionDisabled"
                    @click="handleToggleFormatConversion(endpoint)"
                  >
                    <Shuffle class="w-3.5 h-3.5" />
                  </Button>
                </span>
                <!-- 上游流式三态按钮 -->
                <Button
                  variant="ghost"
                  size="icon"
                  :class="getUpstreamStreamButtonClass(endpoint)"
                  :title="getUpstreamStreamTooltip(endpoint)"
                  :disabled="savingEndpointId === endpoint.id || isUpstreamStreamPolicyLocked(endpoint)"
                  @click="handleCycleUpstreamStream(endpoint)"
                >
                  <Radio class="w-3.5 h-3.5" />
                </Button>
                <!-- 启用/停用 -->
                <Button
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7"
                  :title="endpoint.is_active ? '停用' : '启用'"
                  :disabled="togglingEndpointId === endpoint.id"
                  @click="handleToggleEndpoint(endpoint)"
                >
                  <Power class="w-3.5 h-3.5" />
                </Button>
                <!-- 删除 -->
                <Button
                  v-if="!isFixedProvider"
                  variant="ghost"
                  size="icon"
                  class="h-7 w-7 hover:text-destructive"
                  title="删除"
                  :disabled="deletingEndpointId === endpoint.id"
                  @click="handleDeleteEndpoint(endpoint)"
                >
                  <Trash2 class="w-3.5 h-3.5" />
                </Button>
              </div>
            </div>

            <!-- 卡片内容 -->
            <div class="p-4 space-y-4">
              <!-- URL 配置区 -->
              <div class="flex items-end gap-3">
                <div class="flex-1 min-w-0 grid grid-cols-3 gap-3">
                  <div class="col-span-2 space-y-1.5">
                    <Label class="text-xs text-muted-foreground">Base URL</Label>
                    <Input
                      :model-value="getEndpointEditState(endpoint.id)?.url ?? endpoint.base_url"
                      :placeholder="provider?.website || 'https://api.example.com'"
                      :disabled="isFixedProvider"
                      @update:model-value="(v) => updateEndpointField(endpoint.id, 'url', v)"
                    />
                  </div>
                  <div class="space-y-1.5">
                    <Label class="text-xs text-muted-foreground">自定义路径</Label>
                    <Input
                      :model-value="getDisplayedPath(endpoint)"
                      :placeholder="getDefaultPath(endpoint.api_format, endpoint.base_url) || '留空使用默认'"
                      :disabled="isFixedProvider"
                      @update:model-value="(v) => updateEndpointField(endpoint.id, 'path', v)"
                    />
                  </div>
                </div>
                <!-- 保存/撤销按钮（URL/路径有修改时显示） -->
                <div
                  v-if="hasUrlChanges(endpoint)"
                  class="flex items-center gap-1 shrink-0"
                >
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-9 w-9"
                    title="保存"
                    :disabled="savingEndpointId === endpoint.id"
                    @click="saveEndpoint(endpoint)"
                  >
                    <Check class="w-4 h-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    class="h-9 w-9"
                    title="撤销"
                    @click="resetEndpointChanges(endpoint)"
                  >
                    <RotateCcw class="w-4 h-4" />
                  </Button>
                </div>
              </div>

              <!-- 请求规则（合并请求头和请求体规则） -->
              <Collapsible v-model:open="endpointRulesExpanded[endpoint.id]">
                <div class="flex items-center gap-2">
                  <!-- 有规则时显示可折叠的触发器 -->
                  <CollapsibleTrigger
                    v-if="getTotalRulesCount(endpoint) > 0"
                    as-child
                  >
                    <button
                      type="button"
                      class="flex items-center gap-2 py-1.5 px-2 -mx-2 rounded-md hover:bg-muted/50 transition-colors"
                    >
                      <ChevronRight
                        class="w-4 h-4 transition-transform text-muted-foreground"
                        :class="{ 'rotate-90': endpointRulesExpanded[endpoint.id] }"
                      />
                      <span class="text-sm font-medium">请求规则</span>
                      <Badge
                        variant="secondary"
                        class="text-xs"
                      >
                        {{ getTotalRulesCount(endpoint) }} 条
                      </Badge>
                    </button>
                  </CollapsibleTrigger>
                  <!-- 没有规则时只显示标题 -->
                  <span
                    v-else
                    class="text-sm text-muted-foreground py-1.5"
                  >
                    请求规则
                  </span>
                  <div class="flex-1" />
                  <div class="flex items-center gap-1 shrink-0">
                    <Button
                      v-if="hasRulesChanges(endpoint) || hasBodyRulesChanges(endpoint)"
                      variant="ghost"
                      size="icon"
                      class="h-7 w-7"
                      title="保存规则"
                      :disabled="savingEndpointId === endpoint.id"
                      @click="saveEndpoint(endpoint)"
                    >
                      <Save class="w-3.5 h-3.5" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      class="h-7 text-xs px-2"
                      title="添加请求头规则"
                      @click="handleAddEndpointRule(endpoint.id)"
                    >
                      <Plus class="w-3 h-3 mr-1" />
                      请求头
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      class="h-7 text-xs px-2"
                      title="添加请求体规则"
                      @click="handleAddEndpointBodyRule(endpoint.id)"
                    >
                      <Plus class="w-3 h-3 mr-1" />
                      请求体
                    </Button>
                    <Button
                      v-if="isFixedProvider && hasDefaultBodyRules(endpoint.api_format)"
                      variant="ghost"
                      size="sm"
                      class="h-7 text-xs px-2"
                      title="重置请求体"
                      :disabled="resettingDefaultRulesEndpointId === endpoint.id"
                      @click="handleResetBodyRulesToDefault(endpoint)"
                    >
                      <RotateCcw class="w-3 h-3 mr-1" />
                      重置请求体
                    </Button>
                  </div>
                </div>
                <CollapsibleContent class="pt-3">
                  <div class="space-y-2">
                    <div
                      v-if="getEndpointRulesCount(endpoint) > 1 || getEndpointBodyRulesCount(endpoint) > 1"
                      class="flex items-center gap-1.5 text-xs text-muted-foreground px-2"
                    >
                      <GripVertical class="w-3.5 h-3.5" />
                      <span>拖拽左侧手柄可调整规则执行顺序</span>
                    </div>
                    <!-- 请求头规则列表 - 主题色边框 -->
                    <template
                      v-for="(rule, index) in getEndpointEditRules(endpoint.id)"
                      :key="`header-${index}`"
                    >
                      <div
                        class="flex items-center gap-1.5 px-2 py-1.5 rounded-md border-l-4 border-primary/60 bg-muted/30"
                        :class="[
                          isHeaderRuleDragging(endpoint.id, index) ? 'opacity-60 border-primary bg-primary/5' : '',
                          isHeaderRuleDragOver(endpoint.id, index) ? 'ring-1 ring-primary/40 bg-primary/10' : ''
                        ]"
                        @dragover.prevent="handleHeaderRuleDragOver(endpoint.id, index)"
                        @dragleave="handleHeaderRuleDragLeave(endpoint.id, index)"
                        @drop.prevent="handleHeaderRuleDrop(endpoint.id, index)"
                      >
                        <button
                          type="button"
                          class="h-7 w-6 shrink-0 inline-flex items-center justify-center rounded-sm text-muted-foreground/60 hover:text-muted-foreground hover:bg-muted cursor-grab active:cursor-grabbing"
                          title="拖拽排序"
                          draggable="true"
                          @dragstart="(e) => handleHeaderRuleDragStart(endpoint.id, index, e)"
                          @dragend="() => handleHeaderRuleDragEnd(endpoint.id)"
                        >
                          <GripVertical class="w-3.5 h-3.5" />
                        </button>
                        <span
                          class="text-[10px] font-semibold text-primary shrink-0"
                          title="请求头"
                        >H</span>
                        <Select
                          :model-value="rule.action"
                          :open="ruleSelectOpen[`${endpoint.id}-${index}`]"
                          @update:model-value="(v) => updateEndpointRuleAction(endpoint.id, index, v as 'set' | 'drop' | 'rename')"
                          @update:open="(v) => handleRuleSelectOpen(endpoint.id, index, v)"
                        >
                          <SelectTrigger class="w-[88px] h-7 text-xs shrink-0">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="set">
                              覆写
                            </SelectItem>
                            <SelectItem value="drop">
                              删除
                            </SelectItem>
                            <SelectItem value="rename">
                              重命名
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 shrink-0"
                          :class="rule.condition ? 'text-primary' : ''"
                          title="条件触发"
                          @click="toggleEndpointRuleCondition(endpoint.id, index)"
                        >
                          <Filter class="w-3 h-3" />
                        </Button>
                        <template v-if="rule.action === 'set'">
                          <Input
                            :model-value="rule.key"
                            placeholder="名称"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointRuleField(endpoint.id, index, 'key', v)"
                          />
                          <span class="text-muted-foreground text-xs">=</span>
                          <Input
                            :model-value="rule.value"
                            placeholder="值"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointRuleField(endpoint.id, index, 'value', v)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'drop'">
                          <Input
                            :model-value="rule.key"
                            placeholder="要删除的名称"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointRuleField(endpoint.id, index, 'key', v)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'rename'">
                          <Input
                            :model-value="rule.from"
                            placeholder="原名"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointRuleField(endpoint.id, index, 'from', v)"
                          />
                          <span class="text-muted-foreground text-xs">→</span>
                          <Input
                            :model-value="rule.to"
                            placeholder="新名"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointRuleField(endpoint.id, index, 'to', v)"
                          />
                        </template>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 shrink-0"
                          @click="removeEndpointRule(endpoint.id, index)"
                        >
                          <X class="w-3 h-3" />
                        </Button>
                      </div>
                      <EndpointConditionEditor
                        v-if="rule.condition"
                        :model-value="rule.condition"
                        path-hint="请求体字段路径"
                        removable
                        @update:model-value="(condition) => updateEndpointRuleCondition(endpoint.id, index, condition)"
                        @remove="clearEndpointRuleCondition(endpoint.id, index)"
                      />
                    </template>

                    <div
                      v-if="getEndpointEditBodyRules(endpoint.id).length > 0"
                      class="flex items-center gap-1 text-xs text-muted-foreground px-2"
                    >
                      <span><code class="bg-muted px-1 rounded">.</code> 嵌套字段 / <code class="bg-muted px-1 rounded">[N]</code> 数组索引 / <code class="bg-muted px-1 rounded">[*]</code> 通配符；值为 JSON 格式</span>
                      <div class="flex-1" />
                      <Popover
                        :open="bodyRuleHelpOpenEndpointId === endpoint.id"
                        @update:open="(v: boolean) => setBodyRuleHelpOpen(endpoint.id, v)"
                      >
                        <PopoverTrigger as-child>
                          <button
                            type="button"
                            class="shrink-0 h-6 w-6 inline-flex items-center justify-center rounded-md hover:bg-muted/60"
                            title="规则说明"
                            aria-label="规则说明"
                          >
                            <HelpCircle class="w-3.5 h-3.5 text-muted-foreground/60" />
                          </button>
                        </PopoverTrigger>
                        <PopoverContent
                          side="bottom"
                          align="end"
                          :side-offset="6"
                          class="w-80 p-3 !z-[90]"
                        >
                          <div class="text-xs space-y-2">
                            <div>
                              <div class="font-medium mb-0.5">
                                路径语法
                              </div>
                              <div class="text-muted-foreground">
                                <code>metadata.user_id</code> 嵌套字段<br>
                                <code>messages[0].content</code> 数组索引<br>
                                <code>tools[*].name</code> 通配符（遍历所有元素）<br>
                                <code>tools[0-4].name</code> 范围（遍历索引 0~4）<br>
                                <code>config\.v1.key</code> 转义点号
                              </div>
                            </div>
                            <div>
                              <div class="font-medium mb-0.5">
                                值格式 (JSON)
                              </div>
                              <div class="text-muted-foreground">
                                <code>123</code> 数字 / <code>"text"</code> 字符串 / <code>true</code> 布尔<br>
                                <code>{"k":"v"}</code> 对象 / <code>[1,2]</code> 数组 / <code>null</code><br>
                                <code v-pre>{{$original}}</code> 引用原值
                              </div>
                            </div>
                            <div>
                              <div class="font-medium mb-0.5">
                                命名风格
                              </div>
                              <div class="text-muted-foreground">
                                批量转换字段命名：capitalize / snake_case / camelCase / PascalCase / kebab-case
                              </div>
                            </div>
                            <div>
                              <div class="font-medium mb-0.5">
                                条件运算符
                              </div>
                              <div class="text-muted-foreground">
                                <code>eq</code> <code>neq</code> 等于/不等于<br>
                                <code>gt</code> <code>lt</code> <code>gte</code> <code>lte</code> 大小比较<br>
                                <code>starts_with</code> <code>ends_with</code> <code>contains</code> 字符串匹配<br>
                                <code>matches</code> 正则匹配<br>
                                <code>exists</code> <code>not_exists</code> 字段存在性<br>
                                <code>in</code> 在列表中（值填 <code>["a","b"]</code>）<br>
                                <code>type_is</code> 类型判断（string/number/boolean/array/object/null）<br>
                                条件路径支持 <code>$item.xxx</code> 引用通配符当前元素<br>
                                可切换 <code>Current</code>/<code>Original</code> 数据源，并支持 <code>ALL</code>/<code>ANY</code> 组合条件
                              </div>
                            </div>
                            <div class="text-muted-foreground">
                              规则按顺序执行，前面的修改对后续规则可见。
                            </div>
                            <div class="text-muted-foreground">
                              规则默认在格式转换后按目标提供商结构匹配；条件切到 <code>Original</code> 时则按客户端原始请求体匹配。
                            </div>
                          </div>
                        </PopoverContent>
                      </Popover>
                    </div>

                    <!-- 请求体规则列表 - 次要色边框 -->
                    <template
                      v-for="(rule, index) in getEndpointEditBodyRules(endpoint.id)"
                      :key="`body-${index}`"
                    >
                      <div
                        class="flex items-center gap-1.5 px-2 py-1.5 rounded-md border-l-4 border-muted-foreground/40 bg-muted/30"
                        :class="[
                          isBodyRuleDragging(endpoint.id, index) ? 'opacity-60 border-muted-foreground/70 bg-muted/50' : '',
                          isBodyRuleDragOver(endpoint.id, index) ? 'ring-1 ring-muted-foreground/40 bg-muted/40' : ''
                        ]"
                        @dragover.prevent="handleBodyRuleDragOver(endpoint.id, index)"
                        @dragleave="handleBodyRuleDragLeave(endpoint.id, index)"
                        @drop.prevent="handleBodyRuleDrop(endpoint.id, index)"
                      >
                        <button
                          type="button"
                          class="h-7 w-6 shrink-0 inline-flex items-center justify-center rounded-sm text-muted-foreground/60 hover:text-muted-foreground hover:bg-muted cursor-grab active:cursor-grabbing"
                          title="拖拽排序"
                          draggable="true"
                          @dragstart="(e) => handleBodyRuleDragStart(endpoint.id, index, e)"
                          @dragend="() => handleBodyRuleDragEnd(endpoint.id)"
                        >
                          <GripVertical class="w-3.5 h-3.5" />
                        </button>
                        <span
                          class="text-[10px] font-semibold text-muted-foreground shrink-0"
                          title="请求体"
                        >B</span>
                        <Select
                          :model-value="rule.action"
                          :open="bodyRuleSelectOpen[`${endpoint.id}-${index}`]"
                          @update:model-value="(v: string) => updateEndpointBodyRuleAction(endpoint.id, index, v as BodyRuleAction)"
                          @update:open="(v) => handleBodyRuleSelectOpen(endpoint.id, index, v)"
                        >
                          <SelectTrigger class="w-[96px] h-7 text-xs shrink-0">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            <SelectItem value="set">
                              覆写
                            </SelectItem>
                            <SelectItem value="drop">
                              删除
                            </SelectItem>
                            <SelectItem value="rename">
                              重命名
                            </SelectItem>
                            <SelectItem value="append">
                              追加
                            </SelectItem>
                            <SelectItem value="insert">
                              插入
                            </SelectItem>
                            <SelectItem value="regex_replace">
                              正则替换
                            </SelectItem>
                            <SelectItem value="name_style">
                              命名风格
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 shrink-0"
                          :class="rule.condition ? 'text-primary' : ''"
                          title="条件触发"
                          @click="toggleBodyRuleCondition(endpoint.id, index)"
                        >
                          <Filter class="w-3 h-3" />
                        </Button>
                        <template v-if="rule.action === 'set'">
                          <Input
                            :model-value="rule.path"
                            placeholder="字段路径（如 metadata.user_id）"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'path', v)"
                          />
                          <span class="text-muted-foreground text-xs">=</span>
                          <Input
                            :model-value="rule.value"
                            placeholder="123 / &quot;text&quot; / {{$original}}"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'value', v)"
                          />
                          <CheckCircle
                            class="w-4 h-4 shrink-0"
                            :class="getBodySetValueValidation(rule) === true ? 'text-green-600' : getBodySetValueValidation(rule) === false ? 'text-destructive' : 'text-muted-foreground/40'"
                            :title="getBodySetValueValidationTip(rule)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'drop'">
                          <Input
                            :model-value="rule.path"
                            placeholder="要删除的字段路径"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'path', v)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'rename'">
                          <Input
                            :model-value="rule.from"
                            placeholder="原路径"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'from', v)"
                          />
                          <span class="text-muted-foreground text-xs">→</span>
                          <Input
                            :model-value="rule.to"
                            placeholder="新路径"
                            size="sm"
                            class="flex-1 min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'to', v)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'append'">
                          <Input
                            :model-value="rule.path"
                            placeholder="数组路径（如 messages）"
                            size="sm"
                            class="flex-[2] min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'path', v)"
                          />
                          <span class="text-muted-foreground text-xs">+=</span>
                          <Input
                            :model-value="rule.value"
                            placeholder="值 (JSON)"
                            size="sm"
                            class="flex-[3] min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'value', v)"
                          />
                          <CheckCircle
                            class="w-4 h-4 shrink-0"
                            :class="getBodySetValueValidation(rule) === true ? 'text-green-600' : getBodySetValueValidation(rule) === false ? 'text-destructive' : 'text-muted-foreground/40'"
                            :title="getBodySetValueValidationTip(rule)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'insert'">
                          <Input
                            :model-value="rule.path"
                            placeholder="数组路径（如 messages）"
                            size="sm"
                            class="flex-[2] min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'path', v)"
                          />
                          <Input
                            :model-value="rule.index"
                            placeholder="位置"
                            size="sm"
                            class="w-14 h-7 text-xs shrink-0"
                            title="插入位置（支持负数）"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'index', v)"
                          />
                          <Input
                            :model-value="rule.value"
                            placeholder="值 (JSON)"
                            size="sm"
                            class="flex-[3] min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'value', v)"
                          />
                          <CheckCircle
                            class="w-4 h-4 shrink-0"
                            :class="getBodySetValueValidation(rule) === true ? 'text-green-600' : getBodySetValueValidation(rule) === false ? 'text-destructive' : 'text-muted-foreground/40'"
                            :title="getBodySetValueValidationTip(rule)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'regex_replace'">
                          <Input
                            :model-value="rule.path"
                            placeholder="字段路径"
                            size="sm"
                            class="flex-[2] min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'path', v)"
                          />
                          <Input
                            :model-value="rule.pattern"
                            placeholder="正则"
                            size="sm"
                            class="flex-[2] min-w-0 h-7 text-xs font-mono"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'pattern', v)"
                          />
                          <span class="text-muted-foreground text-xs">→</span>
                          <Input
                            :model-value="rule.replacement"
                            placeholder="替换为"
                            size="sm"
                            class="flex-[2] min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'replacement', v)"
                          />
                          <Input
                            :model-value="rule.flags"
                            placeholder="ims"
                            size="sm"
                            class="w-12 h-7 text-xs shrink-0 font-mono"
                            title="正则标志：i=忽略大小写 m=多行 s=dotall"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'flags', v)"
                          />
                          <Input
                            :model-value="rule.count"
                            placeholder="全部"
                            size="sm"
                            class="w-14 h-7 text-xs shrink-0"
                            title="替换次数；留空=默认全部，0=全部"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'count', v)"
                          />
                          <CheckCircle
                            class="w-4 h-4 shrink-0"
                            :class="getRegexPatternValidation(rule) === true ? 'text-green-600' : getRegexPatternValidation(rule) === false ? 'text-destructive' : 'text-muted-foreground/40'"
                            :title="getRegexPatternValidationTip(rule)"
                          />
                        </template>
                        <template v-else-if="rule.action === 'name_style'">
                          <Input
                            :model-value="rule.path"
                            placeholder="字段路径（如 tools[*].name）"
                            size="sm"
                            class="flex-[2] min-w-0 h-7 text-xs"
                            @update:model-value="(v) => updateEndpointBodyRuleField(endpoint.id, index, 'path', v)"
                          />
                          <span class="text-muted-foreground text-xs">→</span>
                          <Select
                            :model-value="rule.style || 'capitalize'"
                            @update:model-value="(v: string) => updateEndpointBodyRuleField(endpoint.id, index, 'style', v)"
                          >
                            <SelectTrigger class="w-[120px] h-7 text-xs shrink-0">
                              <SelectValue />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="capitalize">
                                Capitalize
                              </SelectItem>
                              <SelectItem value="snake_case">
                                snake_case
                              </SelectItem>
                              <SelectItem value="camelCase">
                                camelCase
                              </SelectItem>
                              <SelectItem value="PascalCase">
                                PascalCase
                              </SelectItem>
                              <SelectItem value="kebab-case">
                                kebab-case
                              </SelectItem>
                            </SelectContent>
                          </Select>
                        </template>
                        <Button
                          variant="ghost"
                          size="icon"
                          class="h-7 w-7 shrink-0"
                          @click="removeEndpointBodyRule(endpoint.id, index)"
                        >
                          <X class="w-3 h-3" />
                        </Button>
                      </div>
                      <EndpointConditionEditor
                        v-if="rule.condition"
                        :model-value="rule.condition"
                        :path-hint="getBodyRuleConditionPathPlaceholder(rule.path)"
                        removable
                        @update:model-value="(condition) => updateEndpointBodyRuleCondition(endpoint.id, index, condition)"
                        @remove="clearEndpointBodyRuleCondition(endpoint.id, index)"
                      />
                    </template>
                  </div>
                </CollapsibleContent>
              </Collapsible>
            </div>
          </div>
        </div>
      </div>

      <!-- 添加新端点 -->
      <div
        v-if="!isFixedProvider && availableFormats.length > 0"
        class="rounded-lg border border-dashed p-3"
      >
        <!-- 卡片头部：API 格式选择 + 添加按钮 -->
        <div class="flex items-center justify-between px-4 py-2.5 bg-muted/30 border-b border-dashed">
          <Select
            v-model="newEndpoint.api_format"
            :open="formatSelectOpen"
            @update:open="handleFormatSelectOpen"
          >
            <SelectTrigger class="h-auto w-auto gap-1.5 !border-0 bg-transparent !shadow-none p-0 font-medium rounded-none flex-row-reverse !ring-0 !ring-offset-0 !outline-none [&>svg]:h-4 [&>svg]:w-4 [&>svg]:opacity-70">
              <SelectValue placeholder="选择格式..." />
            </SelectTrigger>
            <SelectContent>
              <SelectItem
                v-for="format in availableFormats"
                :key="format.value"
                :value="format.value"
              >
                {{ format.label }}
              </SelectItem>
            </SelectContent>
          </Select>
          <Button
            variant="outline"
            size="sm"
            class="h-7 px-3"
            :disabled="!newEndpoint.api_format || (!newEndpoint.base_url?.trim() && !provider?.website?.trim()) || addingEndpoint"
            @click="handleAddEndpoint"
          >
            添加
          </Button>
        </div>
        <!-- 卡片内容：URL 配置 -->
        <div class="p-4">
          <div class="flex items-end gap-3">
            <div class="flex-1 min-w-0 grid grid-cols-3 gap-3">
              <div class="col-span-2 space-y-1.5">
                <Label class="text-xs text-muted-foreground">Base URL</Label>
                <Input
                  v-model="newEndpoint.base_url"
                  size="sm"
                  :placeholder="provider?.website || 'https://api.example.com'"
                />
              </div>
              <div class="space-y-1.5">
                <Label class="text-xs text-muted-foreground">自定义路径</Label>
                <Input
                  v-model="newEndpoint.custom_path"
                  size="sm"
                  :placeholder="newEndpointDefaultPath || '留空使用默认'"
                />
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 空状态 -->
      <div
        v-if="localEndpoints.length === 0 && availableFormats.length === 0"
        class="text-center py-8 text-muted-foreground"
      >
        <p>所有 API 格式都已配置</p>
      </div>
    </div>

    <template #footer>
      <Button
        variant="outline"
        @click="handleClose"
      >
        关闭
      </Button>
    </template>
  </Dialog>

  <!-- 删除端点确认弹窗 -->
  <AlertDialog
    :model-value="deleteConfirmOpen"
    title="删除端点"
    :description="deleteConfirmDescription"
    confirm-text="删除"
    cancel-text="取消"
    type="danger"
    @update:model-value="deleteConfirmOpen = $event"
    @confirm="confirmDeleteEndpoint"
    @cancel="deleteConfirmOpen = false"
  />
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch } from 'vue'
import {
  Dialog,
  Button,
  Input,
  Label,
  Badge,
  Select,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  Collapsible,
  CollapsibleTrigger,
  CollapsibleContent,
  Popover,
  PopoverTrigger,
  PopoverContent,
} from '@/components/ui'
import { Settings, Trash2, Check, X, Power, ChevronRight, Plus, Shuffle, RotateCcw, Radio, CheckCircle, Save, Filter, HelpCircle, GripVertical } from 'lucide-vue-next'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'
import { log } from '@/utils/logger'
import AlertDialog from '@/components/common/AlertDialog.vue'
import EndpointConditionEditor from './EndpointConditionEditor.vue'
import {
  createEndpoint,
  getDefaultBodyRules,
  updateEndpoint,
  deleteEndpoint,
  type ProviderEndpoint,
  type ProviderWithEndpointsSummary,
  type HeaderRule,
  type BodyRule,
  type BodyRuleRegexReplace,
  type BodyRuleNameStyle,
} from '@/api/endpoints'
import { adminApi } from '@/api/admin'
import { formatApiFormat } from '@/api/endpoints/types/api-format'
import {
  conditionEquals,
  conditionToEditable,
  createEmptyConditionLeaf,
  editableConditionToApi,
  getBodyRuleConditionPathPlaceholder,
  type EditableConditionNode,
  validateEditableCondition,
} from './endpoint-rule-condition'

// 编辑用的规则类型（统一的可编辑结构）
interface EditableRule {
  action: 'set' | 'drop' | 'rename'
  key: string      // set/drop 用
  value: string    // set 用
  from: string     // rename 用
  to: string       // rename 用
  condition: EditableConditionNode | null
}

// 编辑用的请求体规则类型
type BodyRuleAction = 'set' | 'drop' | 'rename' | 'append' | 'insert' | 'regex_replace' | 'name_style'

interface EditableBodyRule {
  action: BodyRuleAction
  path: string     // set/drop/append/insert/regex_replace 用
  value: string    // set/append/insert 用（JSON 格式）
  from: string     // rename 用
  to: string       // rename 用
  index: string    // insert 用（字符串输入，保存时解析为 int）
  pattern: string  // regex_replace 用
  replacement: string // regex_replace 用
  flags: string    // regex_replace 用（i/m/s）
  count: string    // regex_replace 用（空=默认全部；0=全部）
  style: string    // name_style 用（snake_case/camelCase/PascalCase/kebab-case/capitalize）
  condition: EditableConditionNode | null
}

// 端点编辑状态（仅 URL、路径、规则，格式转换是直接保存的）
interface EndpointEditState {
  url: string
  path: string
  upstreamStreamPolicy: string
  rules: EditableRule[]
  bodyRules: EditableBodyRule[]
}

const props = defineProps<{
  modelValue: boolean
  provider: ProviderWithEndpointsSummary | null
  endpoints?: ProviderEndpoint[]
  systemFormatConversionEnabled?: boolean
  providerFormatConversionEnabled?: boolean
}>()

const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  'endpointCreated': []
  'endpointUpdated': []
}>()

// 计算端点级格式转换是否应该被禁用
const isEndpointFormatConversionDisabled = computed(() => {
  return props.systemFormatConversionEnabled || props.providerFormatConversionEnabled
})

// 获取禁用提示
const formatConversionDisabledTooltip = computed(() => {
  if (props.systemFormatConversionEnabled) {
    return '请先关闭系统级开关'
  }
  if (props.providerFormatConversionEnabled) {
    return '请先关闭提供商级开关'
  }
  return ''
})

const { success, error: showError } = useToast()

// 规则 Select 的展开状态（与 Collapsible 分开管理）
const ruleSelectOpen = ref<Record<string, boolean>>({})

// 打开规则选择器时关闭其他所有下拉
function handleRuleSelectOpen(endpointId: string, index: number, open: boolean) {
  if (open) {
    formatSelectOpen.value = false
    // 关闭其他 Select
    Object.keys(ruleSelectOpen.value).forEach(key => {
      ruleSelectOpen.value[key] = false
    })
  }
  ruleSelectOpen.value[`${endpointId}-${index}`] = open
}

// 打开格式选择器时关闭其他所有下拉
function handleFormatSelectOpen(open: boolean) {
  if (open) {
    // 关闭所有规则 Select
    Object.keys(ruleSelectOpen.value).forEach(key => {
      ruleSelectOpen.value[key] = false
    })
    Object.keys(bodyRuleSelectOpen.value).forEach(key => {
      bodyRuleSelectOpen.value[key] = false
    })
  }
  formatSelectOpen.value = open
}

// 打开请求体规则选择器时关闭其他所有下拉
function handleBodyRuleSelectOpen(endpointId: string, index: number, open: boolean) {
  if (open) {
    formatSelectOpen.value = false
    // 关闭所有 Select
    Object.keys(ruleSelectOpen.value).forEach(key => {
      ruleSelectOpen.value[key] = false
    })
    Object.keys(bodyRuleSelectOpen.value).forEach(key => {
      bodyRuleSelectOpen.value[key] = false
    })
  }
  bodyRuleSelectOpen.value[`${endpointId}-${index}`] = open
}

function clearHeaderRuleSelectOpen(endpointId: string) {
  Object.keys(ruleSelectOpen.value).forEach((key) => {
    if (key.startsWith(`${endpointId}-`)) {
      delete ruleSelectOpen.value[key]
    }
  })
}

function clearBodyRuleSelectOpen(endpointId: string) {
  Object.keys(bodyRuleSelectOpen.value).forEach((key) => {
    if (key.startsWith(`${endpointId}-`)) {
      delete bodyRuleSelectOpen.value[key]
    }
  })
}

function isHeaderRuleDragging(endpointId: string, index: number): boolean {
  return headerRuleDraggedIndex.value[endpointId] === index
}

function isHeaderRuleDragOver(endpointId: string, index: number): boolean {
  return headerRuleDragOverIndex.value[endpointId] === index
}

function isBodyRuleDragging(endpointId: string, index: number): boolean {
  return bodyRuleDraggedIndex.value[endpointId] === index
}

function isBodyRuleDragOver(endpointId: string, index: number): boolean {
  return bodyRuleDragOverIndex.value[endpointId] === index
}

function clearHeaderRuleDragState(endpointId: string) {
  headerRuleDraggedIndex.value[endpointId] = null
  headerRuleDragOverIndex.value[endpointId] = null
}

function clearBodyRuleDragState(endpointId: string) {
  bodyRuleDraggedIndex.value[endpointId] = null
  bodyRuleDragOverIndex.value[endpointId] = null
}

function handleHeaderRuleDragStart(endpointId: string, index: number, event: DragEvent) {
  const rules = getEndpointEditRules(endpointId)
  if (!rules[index]) return

  headerRuleDraggedIndex.value[endpointId] = index
  headerRuleDragOverIndex.value[endpointId] = null
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', `header:${endpointId}:${index}`)
  }
}

function handleHeaderRuleDragOver(endpointId: string, index: number) {
  const dragged = headerRuleDraggedIndex.value[endpointId]
  if (dragged === null || dragged === undefined || dragged === index) return
  headerRuleDragOverIndex.value[endpointId] = index
}

function handleHeaderRuleDragLeave(endpointId: string, index: number) {
  if (headerRuleDragOverIndex.value[endpointId] === index) {
    headerRuleDragOverIndex.value[endpointId] = null
  }
}

function handleHeaderRuleDrop(endpointId: string, targetIndex: number) {
  const dragIndex = headerRuleDraggedIndex.value[endpointId]
  clearHeaderRuleDragState(endpointId)
  if (dragIndex === null || dragIndex === undefined || dragIndex === targetIndex) return

  const rules = getEndpointEditRules(endpointId)
  if (dragIndex < 0 || dragIndex >= rules.length || targetIndex < 0 || targetIndex >= rules.length) return

  const [draggedRule] = rules.splice(dragIndex, 1)
  rules.splice(targetIndex, 0, draggedRule)
  clearHeaderRuleSelectOpen(endpointId)
}

function handleHeaderRuleDragEnd(endpointId: string) {
  clearHeaderRuleDragState(endpointId)
}

function handleBodyRuleDragStart(endpointId: string, index: number, event: DragEvent) {
  const rules = getEndpointEditBodyRules(endpointId)
  if (!rules[index]) return

  bodyRuleDraggedIndex.value[endpointId] = index
  bodyRuleDragOverIndex.value[endpointId] = null
  if (event.dataTransfer) {
    event.dataTransfer.effectAllowed = 'move'
    event.dataTransfer.setData('text/plain', `body:${endpointId}:${index}`)
  }
}

function handleBodyRuleDragOver(endpointId: string, index: number) {
  const dragged = bodyRuleDraggedIndex.value[endpointId]
  if (dragged === null || dragged === undefined || dragged === index) return
  bodyRuleDragOverIndex.value[endpointId] = index
}

function handleBodyRuleDragLeave(endpointId: string, index: number) {
  if (bodyRuleDragOverIndex.value[endpointId] === index) {
    bodyRuleDragOverIndex.value[endpointId] = null
  }
}

function handleBodyRuleDrop(endpointId: string, targetIndex: number) {
  const dragIndex = bodyRuleDraggedIndex.value[endpointId]
  clearBodyRuleDragState(endpointId)
  if (dragIndex === null || dragIndex === undefined || dragIndex === targetIndex) return

  const rules = getEndpointEditBodyRules(endpointId)
  if (dragIndex < 0 || dragIndex >= rules.length || targetIndex < 0 || targetIndex >= rules.length) return

  const [draggedRule] = rules.splice(dragIndex, 1)
  rules.splice(targetIndex, 0, draggedRule)
  clearBodyRuleSelectOpen(endpointId)
}

function handleBodyRuleDragEnd(endpointId: string) {
  clearBodyRuleDragState(endpointId)
}

// 状态
const addingEndpoint = ref(false)
const savingEndpointId = ref<string | null>(null)
const resettingDefaultRulesEndpointId = ref<string | null>(null)
const deletingEndpointId = ref<string | null>(null)
const togglingEndpointId = ref<string | null>(null)
const togglingFormatEndpointId = ref<string | null>(null)
const formatSelectOpen = ref(false)

// 删除确认弹窗状态
const deleteConfirmOpen = ref(false)
const endpointToDelete = ref<ProviderEndpoint | null>(null)

// 请求规则折叠状态
const endpointRulesExpanded = ref<Record<string, boolean>>({})

// 请求体规则 Select 的展开状态
const bodyRuleSelectOpen = ref<Record<string, boolean>>({})

// 请求体规则说明 Popover 的展开状态
const bodyRuleHelpOpenEndpointId = ref<string | null>(null)

// 规则拖拽状态（按 endpoint 维度）
const headerRuleDraggedIndex = ref<Record<string, number | null>>({})
const headerRuleDragOverIndex = ref<Record<string, number | null>>({})
const bodyRuleDraggedIndex = ref<Record<string, number | null>>({})
const bodyRuleDragOverIndex = ref<Record<string, number | null>>({})

function setBodyRuleHelpOpen(endpointId: string, open: boolean) {
  bodyRuleHelpOpenEndpointId.value = open ? endpointId : null
}

// 每个端点的编辑状态（内联编辑）
const endpointEditStates = ref<Record<string, EndpointEditState>>({})
const defaultBodyRulesByFormat = ref<Record<string, BodyRule[]>>({})
const defaultBodyRulesLoaded = ref<Record<string, boolean>>({})
const loadingDefaultBodyRulesByFormat = ref<Record<string, boolean>>({})

// 系统保留的 header 名称（不允许用户设置）
const RESERVED_HEADERS = new Set([
  'authorization',
  'x-api-key',
  'x-goog-api-key',
  'content-type',
  'content-length',
  'host',
])

// 系统保留的 body 字段名（不允许用户设置）
const RESERVED_BODY_FIELDS = new Set([
  'model',
  'stream',
])

// {{$original}} 占位符处理
const ORIGINAL_PLACEHOLDER = '{{$original}}'
const ORIGINAL_SENTINEL = '__AETHER_ORIGINAL__'

// 将 {{$original}} 替换为合法 JSON 以便 JSON.parse 校验
// 处理三种写法：裸占位符 {{$original}}、带引号 "{{$original}}"、引号内拼接 "prefix_{{$original}}_suffix"
function prepareValueForJsonParse(raw: string): string {
  // Step 1: 纯文本替换占位符为 sentinel
  const result = raw.replaceAll(ORIGINAL_PLACEHOLDER, ORIGINAL_SENTINEL)

  // Step 2: 尝试直接 parse（占位符在引号内时已经是合法 JSON）
  try { JSON.parse(result); return result } catch { /* sentinel not in valid JSON position */ }

  // Step 3: 有裸 sentinel 不在引号内，需要扫描并补引号
  let out = ''
  let inStr = false
  let i = 0
  while (i < result.length) {
    if (result[i] === '\\' && inStr) {
      out += result[i] + (result[i + 1] || '')
      i += 2
      continue
    }
    if (result[i] === '"') {
      inStr = !inStr
      out += result[i]
      i++
      continue
    }
    if (!inStr && result.startsWith(ORIGINAL_SENTINEL, i)) {
      out += `"${  ORIGINAL_SENTINEL  }"`
      i += ORIGINAL_SENTINEL.length
      continue
    }
    out += result[i]
    i++
  }
  return out
}

// 递归还原: 将 sentinel 字符串还原为 {{$original}}
function restoreOriginalPlaceholder(value: unknown): unknown {
  if (typeof value === 'string') {
    if (value === ORIGINAL_SENTINEL) return ORIGINAL_PLACEHOLDER
    if (value.includes(ORIGINAL_SENTINEL)) {
      return value.replaceAll(ORIGINAL_SENTINEL, ORIGINAL_PLACEHOLDER)
    }
    return value
  }
  if (Array.isArray(value)) return value.map(item => restoreOriginalPlaceholder(item))
  if (value !== null && typeof value === 'object') {
    const result: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) result[k] = restoreOriginalPlaceholder(v)
    return result
  }
  return value
}

function parseBodyRulePathParts(path: string): string[] | null {
  const raw = path.trim()
  if (!raw) return null

  const parts: string[] = []
  let current = ''
  for (let i = 0; i < raw.length; i++) {
    const ch = raw[i]
    // 支持 \. 转义字面量点号；其他反斜杠组合按字面量保留
    if (ch === '\\' && i + 1 < raw.length && raw[i + 1] === '.') {
      current += '.'
      i++
      continue
    }
    if (ch === '.') {
      if (!current) return null // 禁止空段：.a / a. / a..b
      parts.push(current)
      current = ''
      continue
    }
    current += ch
  }
  if (!current) return null
  parts.push(current)
  return parts
}

function initBodyRuleSetValueForEditor(value: unknown): { value: string } {
  if (value === undefined) return { value: '' }

  // 所有值都用 JSON 格式回显
  try {
    return { value: JSON.stringify(value) }
  } catch {
    return { value: String(value) }
  }
}

// 内部状态
const internalOpen = computed(() => props.modelValue)

const isFixedProvider = computed(() => {
  const t = props.provider?.provider_type
  return !!t && t !== 'custom'
})

// 新端点表单
const newEndpoint = ref({
  api_format: '',
  base_url: '',
  custom_path: '',
})

// API 格式列表
const apiFormats = ref<Array<{ value: string; label: string; default_path: string }>>([])

// 本地端点列表
const localEndpoints = ref<ProviderEndpoint[]>([])

// 可用的格式（未添加的）
const availableFormats = computed(() => {
  const existingFormats = localEndpoints.value.map(e => e.api_format)
  return apiFormats.value.filter(f => !existingFormats.includes(f.value))
})

// 删除确认弹窗描述
const deleteConfirmDescription = computed(() => {
  if (!endpointToDelete.value) return ''
  const formatLabel = formatApiFormat(endpointToDelete.value.api_format)
  return `确定要删除 ${formatLabel} 端点吗？关联密钥将移除对该 API 格式的支持。`
})

function defaultBodyRulesCacheKey(apiFormat: string): string {
  const providerType = (props.provider?.provider_type || '').toLowerCase()
  return providerType ? `${apiFormat}:${providerType}` : apiFormat
}

function hasDefaultBodyRules(apiFormat: string): boolean {
  const cacheKey = defaultBodyRulesCacheKey(apiFormat)
  if (!defaultBodyRulesLoaded.value[cacheKey]) return false
  return (defaultBodyRulesByFormat.value[cacheKey]?.length || 0) > 0
}

function normalizeLegacyOpenAIFormatAlias(apiFormat: string): string {
  switch (apiFormat.trim().toLowerCase()) {
    case 'openai:cli':
      return 'openai:responses'
    case 'openai:compact':
      return 'openai:responses:compact'
    default:
      return apiFormat.trim().toLowerCase()
  }
}

async function loadDefaultBodyRulesForFormat(apiFormat: string, force = false): Promise<BodyRule[]> {
  if (!apiFormat) return []
  const providerType = (props.provider?.provider_type || '').toLowerCase()
  const cacheKey = defaultBodyRulesCacheKey(apiFormat)
  if (!force && defaultBodyRulesLoaded.value[cacheKey]) {
    return defaultBodyRulesByFormat.value[cacheKey] || []
  }
  if (loadingDefaultBodyRulesByFormat.value[cacheKey]) {
    return defaultBodyRulesByFormat.value[cacheKey] || []
  }

  loadingDefaultBodyRulesByFormat.value[cacheKey] = true
  try {
    const response = await getDefaultBodyRules(apiFormat, providerType || undefined)
    const rules = response.body_rules || []
    defaultBodyRulesByFormat.value[cacheKey] = rules
    defaultBodyRulesLoaded.value[cacheKey] = true
    return rules
  } catch (error: unknown) {
    defaultBodyRulesByFormat.value[cacheKey] = []
    defaultBodyRulesLoaded.value[cacheKey] = true
    log.warn('加载默认请求体规则失败', { apiFormat, error })
    return []
  } finally {
    loadingDefaultBodyRulesByFormat.value[cacheKey] = false
  }
}

async function preloadDefaultBodyRules(endpoints: ProviderEndpoint[]): Promise<void> {
  const formats = Array.from(new Set(endpoints.map(e => e.api_format).filter(Boolean)))
  await Promise.all(formats.map(fmt => loadDefaultBodyRulesForFormat(fmt)))
}

// 获取指定 API 格式的默认路径
function getDefaultPath(apiFormat: string, baseUrl?: string): string {
  const providerType = (props.provider?.provider_type || '').toLowerCase()
  const normalizedApiFormat = normalizeLegacyOpenAIFormatAlias(apiFormat)
  if (providerType === 'vertex_ai') {
    if (normalizedApiFormat === 'gemini:chat') {
      return '/v1/publishers/google/models/{model}:{action}'
    }
    if (normalizedApiFormat === 'claude:chat') {
      return '/v1/projects/{project_id}/locations/{region}/publishers/anthropic/models/{model}:{action}'
    }
  }

  const format = apiFormats.value.find(f => f.value === normalizedApiFormat)
  const defaultPath = format?.default_path || ''
  // Codex 端点使用 /responses 而非 /v1/responses
  const isCodex = providerType
    ? providerType === 'codex'
    : (!!baseUrl && isCodexUrl(baseUrl))
  if (normalizedApiFormat === 'openai:responses' && isCodex) {
    return '/responses'
  }
  return defaultPath
}

function getDisplayedPath(endpoint: ProviderEndpoint): string {
  if (isFixedProvider.value) {
    return getDefaultPath(endpoint.api_format, endpoint.base_url)
  }
  return getEndpointEditState(endpoint.id)?.path ?? (endpoint.custom_path || '')
}

// 判断是否是 Codex OAuth 端点
function isCodexUrl(baseUrl: string): boolean {
  const url = baseUrl.replace(/\/+$/, '')
  return url.includes('/backend-api/codex') || url.endsWith('/codex')
}

// 读取端点的上游流式策略（endpoint.config.upstream_stream_policy）
function getEndpointUpstreamStreamPolicy(endpoint: ProviderEndpoint): string {
  const cfg = endpoint.config || {}
  const raw = (cfg.upstream_stream_policy ?? cfg.upstreamStreamPolicy ?? cfg.upstream_stream) as unknown
  if (raw === null || raw === undefined) return 'auto'
  if (typeof raw === 'boolean') return raw ? 'force_stream' : 'force_non_stream'
  const s = String(raw).trim().toLowerCase()
  if (!s || s === 'auto' || s === 'follow' || s === 'client' || s === 'default') return 'auto'
  if (s === 'force_stream' || s === 'stream' || s === 'sse' || s === 'true' || s === '1') return 'force_stream'
  if (s === 'force_non_stream' || s === 'force_sync' || s === 'non_stream' || s === 'sync' || s === 'false' || s === '0') return 'force_non_stream'
  return 'auto'
}

function emptyHeaderRule(): EditableRule {
  return { action: 'set', key: '', value: '', from: '', to: '', condition: null }
}

function emptyBodyRule(action: BodyRuleAction = 'set'): EditableBodyRule {
  return {
    action,
    path: '',
    value: '',
    from: '',
    to: '',
    index: '',
    pattern: '',
    replacement: '',
    flags: '',
    count: '',
    style: '',
    condition: null,
  }
}

// 初始化端点的编辑状态
function initEndpointEditState(endpoint: ProviderEndpoint): EndpointEditState {
  const rules: EditableRule[] = []
  if (endpoint.header_rules && endpoint.header_rules.length > 0) {
    for (const rule of endpoint.header_rules) {
      if (rule.action === 'set') {
        rules.push({ ...emptyHeaderRule(), action: 'set', key: rule.key, value: rule.value || '', condition: conditionToEditable(rule.condition) })
      } else if (rule.action === 'drop') {
        rules.push({ ...emptyHeaderRule(), action: 'drop', key: rule.key, condition: conditionToEditable(rule.condition) })
      } else if (rule.action === 'rename') {
        rules.push({ ...emptyHeaderRule(), action: 'rename', from: rule.from, to: rule.to, condition: conditionToEditable(rule.condition) })
      }
    }
  }

  const bodyRules: EditableBodyRule[] = []
  if (endpoint.body_rules && endpoint.body_rules.length > 0) {
    for (const rule of endpoint.body_rules) {
      if (rule.action === 'set') {
        const { value } = initBodyRuleSetValueForEditor(rule.value)
        bodyRules.push({ ...emptyBodyRule('set'), path: rule.path, value, condition: conditionToEditable(rule.condition) })
      } else if (rule.action === 'drop') {
        bodyRules.push({ ...emptyBodyRule('drop'), path: rule.path, condition: conditionToEditable(rule.condition) })
      } else if (rule.action === 'rename') {
        bodyRules.push({ ...emptyBodyRule('rename'), from: rule.from, to: rule.to, condition: conditionToEditable(rule.condition) })
      } else if (rule.action === 'append') {
        const { value } = initBodyRuleSetValueForEditor(rule.value)
        bodyRules.push({ ...emptyBodyRule('append'), path: rule.path || '', value, condition: conditionToEditable(rule.condition) })
      } else if (rule.action === 'insert') {
        const { value } = initBodyRuleSetValueForEditor(rule.value)
        bodyRules.push({ ...emptyBodyRule('insert'), path: rule.path || '', value, index: String(rule.index ?? ''), condition: conditionToEditable(rule.condition) })
      } else if (rule.action === 'regex_replace') {
        bodyRules.push({
          ...emptyBodyRule('regex_replace'),
          path: rule.path || '',
          pattern: rule.pattern || '',
          replacement: rule.replacement || '',
          flags: rule.flags || '',
          count: rule.count === undefined || rule.count === null ? '' : String(rule.count),
          condition: conditionToEditable(rule.condition),
        })
      } else if (rule.action === 'name_style') {
        bodyRules.push({ ...emptyBodyRule('name_style'), path: rule.path || '', style: rule.style || 'capitalize', condition: conditionToEditable(rule.condition) })
      }
    }
  }

  return {
    url: endpoint.base_url,
    path: endpoint.custom_path || '',
    upstreamStreamPolicy: getEndpointUpstreamStreamPolicy(endpoint),
    rules,
    bodyRules,
  }
}

// 获取端点的编辑状态
function getEndpointEditState(endpointId: string): EndpointEditState | undefined {
  return endpointEditStates.value[endpointId]
}

// 更新端点字段
function updateEndpointField(endpointId: string, field: 'url' | 'path', value: string) {
  if (!endpointEditStates.value[endpointId]) {
    const endpoint = localEndpoints.value.find(e => e.id === endpointId)
    if (endpoint) {
      endpointEditStates.value[endpointId] = initEndpointEditState(endpoint)
    }
  }
  if (endpointEditStates.value[endpointId]) {
    endpointEditStates.value[endpointId][field] = value
  }
}

// 获取端点的编辑规则
function getEndpointEditRules(endpointId: string): EditableRule[] {
  const state = endpointEditStates.value[endpointId]
  if (state) {
    return state.rules
  }
  // 从原始端点加载
  const endpoint = localEndpoints.value.find(e => e.id === endpointId)
  if (endpoint) {
    const newState = initEndpointEditState(endpoint)
    endpointEditStates.value[endpointId] = newState
    return newState.rules
  }
  return []
}

// 添加规则（同时自动展开折叠）
function handleAddEndpointRule(endpointId: string) {
  const rules = getEndpointEditRules(endpointId)
  rules.push(emptyHeaderRule())
  // 自动展开折叠
  endpointRulesExpanded.value[endpointId] = true
}

// 删除规则
function removeEndpointRule(endpointId: string, index: number) {
  const rules = getEndpointEditRules(endpointId)
  rules.splice(index, 1)
  clearHeaderRuleDragState(endpointId)
  clearHeaderRuleSelectOpen(endpointId)
}

// 更新规则类型
function updateEndpointRuleAction(endpointId: string, index: number, action: 'set' | 'drop' | 'rename') {
  const rules = getEndpointEditRules(endpointId)
  if (rules[index]) {
    const currentCondition = rules[index].condition
    rules[index] = { ...emptyHeaderRule(), action, condition: currentCondition }
  }
}

// 更新规则字段
function updateEndpointRuleField(endpointId: string, index: number, field: 'key' | 'value' | 'from' | 'to', value: string) {
  const rules = getEndpointEditRules(endpointId)
  if (rules[index]) {
    rules[index][field] = value
  }
}

function toggleEndpointRuleCondition(endpointId: string, index: number) {
  const rules = getEndpointEditRules(endpointId)
  if (rules[index]) {
    rules[index].condition = rules[index].condition ? null : createEmptyConditionLeaf()
  }
}

function updateEndpointRuleCondition(endpointId: string, index: number, condition: EditableConditionNode) {
  const rules = getEndpointEditRules(endpointId)
  if (rules[index]) {
    rules[index].condition = condition
  }
}

function clearEndpointRuleCondition(endpointId: string, index: number) {
  const rules = getEndpointEditRules(endpointId)
  if (rules[index]) {
    rules[index].condition = null
  }
}

// 验证规则 key（针对特定端点）
function validateRuleKeyForEndpoint(endpointId: string, key: string, index: number): string | null {
  const trimmedKey = key.trim().toLowerCase()
  if (!trimmedKey) return null

  if (RESERVED_HEADERS.has(trimmedKey)) {
    return `"${key}" 是系统保留的请求头`
  }

  const rules = getEndpointEditRules(endpointId)
  const duplicate = rules.findIndex(
    (r, i) => i !== index && (
      ((r.action === 'set' || r.action === 'drop') && r.key.trim().toLowerCase() === trimmedKey) ||
      (r.action === 'rename' && r.to.trim().toLowerCase() === trimmedKey)
    )
  )
  if (duplicate >= 0) {
    return '请求头名称重复'
  }

  return null
}

// 验证 rename from
function validateRenameFromForEndpoint(endpointId: string, from: string, index: number): string | null {
  const trimmedFrom = from.trim().toLowerCase()
  if (!trimmedFrom) return null

  const rules = getEndpointEditRules(endpointId)
  const duplicate = rules.findIndex(
    (r, i) => i !== index &&
      ((r.action === 'set' && r.key.trim().toLowerCase() === trimmedFrom) ||
       (r.action === 'drop' && r.key.trim().toLowerCase() === trimmedFrom) ||
       (r.action === 'rename' && r.from.trim().toLowerCase() === trimmedFrom))
  )
  if (duplicate >= 0) {
    return '该请求头已被其他规则处理'
  }

  return null
}

// 验证 rename to
function validateRenameToForEndpoint(endpointId: string, to: string, index: number): string | null {
  const trimmedTo = to.trim().toLowerCase()
  if (!trimmedTo) return null

  if (RESERVED_HEADERS.has(trimmedTo)) {
    return `"${to}" 是系统保留的请求头`
  }

  const rules = getEndpointEditRules(endpointId)
  const duplicate = rules.findIndex(
    (r, i) => i !== index &&
      ((r.action === 'set' && r.key.trim().toLowerCase() === trimmedTo) ||
       (r.action === 'rename' && r.to.trim().toLowerCase() === trimmedTo))
  )
  if (duplicate >= 0) {
    return '请求头名称重复'
  }

  return null
}

// 获取端点的请求头规则数量（有效的规则）
function getEndpointRulesCount(endpoint: ProviderEndpoint): number {
  const state = endpointEditStates.value[endpoint.id]
  if (state) {
    return state.rules.filter(r => {
      if (r.action === 'set' || r.action === 'drop') return r.key.trim()
      if (r.action === 'rename') return r.from.trim() && r.to.trim()
      return false
    }).length
  }
  return endpoint.header_rules?.length || 0
}

// 检查端点是否有任何规则（包括正在编辑的空规则）
function _hasAnyRules(endpoint: ProviderEndpoint): boolean {
  const state = endpointEditStates.value[endpoint.id]
  if (state) {
    return state.rules.length > 0
  }
  return (endpoint.header_rules?.length || 0) > 0
}

// ========== 请求体规则相关函数 ==========

// 获取端点的编辑请求体规则
function getEndpointEditBodyRules(endpointId: string): EditableBodyRule[] {
  const state = endpointEditStates.value[endpointId]
  if (state) {
    return state.bodyRules
  }
  // 从原始端点加载
  const endpoint = localEndpoints.value.find(e => e.id === endpointId)
  if (endpoint) {
    const newState = initEndpointEditState(endpoint)
    endpointEditStates.value[endpointId] = newState
    return newState.bodyRules
  }
  return []
}

// 添加请求体规则（同时自动展开折叠）
function handleAddEndpointBodyRule(endpointId: string) {
  const rules = getEndpointEditBodyRules(endpointId)
  rules.push(emptyBodyRule('set'))
  // 自动展开折叠
  endpointRulesExpanded.value[endpointId] = true
}

// 删除请求体规则
function removeEndpointBodyRule(endpointId: string, index: number) {
  const rules = getEndpointEditBodyRules(endpointId)
  rules.splice(index, 1)
  clearBodyRuleDragState(endpointId)
  clearBodyRuleSelectOpen(endpointId)
}

// 更新请求体规则类型
function updateEndpointBodyRuleAction(endpointId: string, index: number, action: BodyRuleAction) {
  const rules = getEndpointEditBodyRules(endpointId)
  if (rules[index]) {
    const currentCondition = rules[index].condition
    rules[index] = { ...emptyBodyRule(action), condition: currentCondition }
  }
}

// 更新请求体规则字段
function updateEndpointBodyRuleField(endpointId: string, index: number, field: 'path' | 'value' | 'from' | 'to' | 'index' | 'pattern' | 'replacement' | 'flags' | 'count' | 'style', value: string) {
  const rules = getEndpointEditBodyRules(endpointId)
  if (rules[index]) {
    rules[index][field] = value
  }
}

function toggleBodyRuleCondition(endpointId: string, index: number) {
  const rules = getEndpointEditBodyRules(endpointId)
  if (rules[index]) {
    rules[index].condition = rules[index].condition ? null : createEmptyConditionLeaf()
  }
}

function updateEndpointBodyRuleCondition(endpointId: string, index: number, condition: EditableConditionNode) {
  const rules = getEndpointEditBodyRules(endpointId)
  if (rules[index]) {
    rules[index].condition = condition
  }
}

function clearEndpointBodyRuleCondition(endpointId: string, index: number) {
  const rules = getEndpointEditBodyRules(endpointId)
  if (rules[index]) {
    rules[index].condition = null
  }
}

// 验证请求体规则 path（针对特定端点）
function validateBodyRulePathForEndpoint(endpointId: string, path: string, index: number): string | null {
  const raw = path.trim()
  if (!raw) return null

  // 基础格式校验；对含 [N] 的路径，取方括号前的部分做 dot 校验
  const dotPart = raw.includes('[') ? raw.slice(0, raw.indexOf('[')) : raw
  const parts = dotPart ? parseBodyRulePathParts(dotPart) : [raw.split('[')[0] || raw]
  if (!parts) {
    return '路径格式无效'
  }

  // 提取顶层 key（去除数组索引部分）
  const topKey = (parts[0] || '').trim().toLowerCase()
  if (RESERVED_BODY_FIELDS.has(topKey)) {
    return `"${parts[0]}" 是系统保留的顶层字段`
  }

  const normalizedPath = raw.toLowerCase()

  const rules = getEndpointEditBodyRules(endpointId)
  const currentRule = rules[index]
  // 任意一方启用了条件，则不视为冲突（条件可能互斥，真正冲突在运行时处理）
  const duplicate = rules.findIndex(
    (r, i) => i !== index && !currentRule.condition && !r.condition && (
      ((r.action === 'set' || r.action === 'drop') && r.path.trim().toLowerCase() === normalizedPath) ||
      (r.action === 'rename' && r.to.trim().toLowerCase() === normalizedPath)
    )
  )
  if (duplicate >= 0) {
    return '字段路径重复'
  }

  return null
}

// 验证请求体 rename from
function validateBodyRenameFromForEndpoint(endpointId: string, from: string, index: number): string | null {
  const raw = from.trim()
  if (!raw) return null

  const parts = parseBodyRulePathParts(raw)
  if (!parts) {
    return '路径格式无效（不允许 .a / a. / a..b）'
  }

  const topKey = (parts[0] || '').trim().toLowerCase()
  if (RESERVED_BODY_FIELDS.has(topKey)) {
    return `"${parts[0]}" 是系统保留的顶层字段`
  }

  const normalizedFrom = raw.toLowerCase()

  const rules = getEndpointEditBodyRules(endpointId)
  const currentRule = rules[index]
  const duplicate = rules.findIndex(
    (r, i) => i !== index && !currentRule.condition && !r.condition &&
      ((r.action === 'set' && r.path.trim().toLowerCase() === normalizedFrom) ||
       (r.action === 'drop' && r.path.trim().toLowerCase() === normalizedFrom) ||
       (r.action === 'rename' && r.from.trim().toLowerCase() === normalizedFrom))
  )
  if (duplicate >= 0) {
    return '该路径已被其他规则处理'
  }

  return null
}

// 验证请求体 rename to
function validateBodyRenameToForEndpoint(endpointId: string, to: string, index: number): string | null {
  const raw = to.trim()
  if (!raw) return null

  const parts = parseBodyRulePathParts(raw)
  if (!parts) {
    return '路径格式无效（不允许 .a / a. / a..b）'
  }

  const topKey = (parts[0] || '').trim().toLowerCase()
  if (RESERVED_BODY_FIELDS.has(topKey)) {
    return `"${parts[0]}" 是系统保留的顶层字段`
  }

  const normalizedTo = raw.toLowerCase()

  const rules = getEndpointEditBodyRules(endpointId)
  const currentRule = rules[index]
  const duplicate = rules.findIndex(
    (r, i) => i !== index && !currentRule.condition && !r.condition &&
      ((r.action === 'set' && r.path.trim().toLowerCase() === normalizedTo) ||
       (r.action === 'rename' && r.to.trim().toLowerCase() === normalizedTo))
  )
  if (duplicate >= 0) {
    return '字段路径重复'
  }

  return null
}

function validateBodySetValue(rule: EditableBodyRule): string | null {
  if (rule.action !== 'set' && rule.action !== 'append' && rule.action !== 'insert') return null

  const raw = rule.value.trim()
  if (!raw) return '值不能为空'
  try {
    JSON.parse(prepareValueForJsonParse(raw))
  } catch (err: unknown) {
    const msg = err instanceof Error ? err.message : String(err)
    return `JSON 格式错误：${msg}`
  }
  return null
}

// 获取值验证状态：true=有效, false=无效, null=空
function getBodySetValueValidation(rule: EditableBodyRule): boolean | null {
  if (rule.action !== 'set' && rule.action !== 'append' && rule.action !== 'insert') return null
  const raw = rule.value.trim()
  if (!raw) return null
  try {
    JSON.parse(prepareValueForJsonParse(raw))
    return true
  } catch {
    return false
  }
}

// 正则表达式验证状态：true=有效, false=无效, null=空
function getRegexPatternValidation(rule: EditableBodyRule): boolean | null {
  if (rule.action !== 'regex_replace') return null
  const pattern = rule.pattern.trim()
  if (!pattern) return null
  try {
    new RegExp(pattern)
    // 校验 flags
    const flags = rule.flags.trim()
    if (flags) {
      const validFlags = new Set(['i', 'm', 's'])
      for (const f of flags) {
        if (!validFlags.has(f)) return false
      }
    }
    return true
  } catch {
    return false
  }
}

// 获取正则验证提示
function getRegexPatternValidationTip(rule: EditableBodyRule): string {
  const validation = getRegexPatternValidation(rule)
  if (validation === null) return '输入正则表达式'
  if (validation === true) return '有效的正则表达式'
  try {
    new RegExp(rule.pattern.trim())
    // 正则有效但 flags 无效
    return '无效的 flags（仅允许 i/m/s）'
  } catch (err: unknown) {
    return err instanceof Error ? err.message : String(err)
  }
}

// 获取验证提示
function getBodySetValueValidationTip(rule: EditableBodyRule): string {
  const validation = getBodySetValueValidation(rule)
  if (validation === null) return '点击验证 JSON'
  if (validation === true) {
    const parsed = restoreOriginalPlaceholder(JSON.parse(prepareValueForJsonParse(rule.value.trim())))
    const type = Array.isArray(parsed) ? '数组' : typeof parsed === 'object' && parsed !== null ? '对象' : typeof parsed === 'string' ? '字符串' : typeof parsed === 'number' ? '数字' : typeof parsed === 'boolean' ? '布尔' : 'null'
    return `有效的 JSON (${type})`
  }
  try {
    JSON.parse(prepareValueForJsonParse(rule.value.trim()))
    return ''
  } catch (err: unknown) {
    return err instanceof Error ? err.message : String(err)
  }
}

function isStrictIntegerString(value: string): boolean {
  return /^-?\d+$/.test(value.trim())
}

function isStrictNonNegativeIntegerString(value: string): boolean {
  return /^\d+$/.test(value.trim())
}

// 判断请求体规则是否有效（必填字段已填写）
function isBodyRuleEffective(r: EditableBodyRule): boolean {
  switch (r.action) {
    case 'set':
    case 'drop':
      return !!r.path.trim()
    case 'rename':
      return !!(r.from.trim() && r.to.trim())
    case 'append':
      return !!r.path.trim()
    case 'insert':
      return !!(r.path.trim() && r.index.trim())
    case 'regex_replace':
      return !!(r.path.trim() && r.pattern.trim())
    case 'name_style':
      return !!(r.path.trim() && r.style.trim())
    default:
      return false
  }
}

// 获取端点的请求体规则数量（有效的规则）
function getEndpointBodyRulesCount(endpoint: ProviderEndpoint): number {
  const state = endpointEditStates.value[endpoint.id]
  if (state) {
    return state.bodyRules.filter(isBodyRuleEffective).length
  }
  return endpoint.body_rules?.length || 0
}

// 检查端点是否有任何请求体规则（包括正在编辑的空规则）
function _hasAnyBodyRules(endpoint: ProviderEndpoint): boolean {
  const state = endpointEditStates.value[endpoint.id]
  if (state) {
    return state.bodyRules.length > 0
  }
  return (endpoint.body_rules?.length || 0) > 0
}

// 获取端点的总规则数量（请求头 + 请求体）
function getTotalRulesCount(endpoint: ProviderEndpoint): number {
  return getEndpointRulesCount(endpoint) + getEndpointBodyRulesCount(endpoint)
}

// 格式化请求头规则的显示标签
function _formatHeaderRuleLabel(rule: EditableRule): string {
  if (rule.action === 'set') {
    if (!rule.key) return '(未设置)'
    return `${rule.key}=${rule.value || '...'}`
  } else if (rule.action === 'drop') {
    if (!rule.key) return '(未设置)'
    return `-${rule.key}`
  } else if (rule.action === 'rename') {
    if (!rule.from || !rule.to) return '(未设置)'
    return `${rule.from}→${rule.to}`
  }
  return '(未知)'
}

// 格式化请求体规则的显示标签
function _formatBodyRuleLabel(rule: EditableBodyRule): string {
  if (rule.action === 'set') {
    if (!rule.path) return '(未设置)'
    return `${rule.path}=${rule.value || '...'}`
  } else if (rule.action === 'drop') {
    if (!rule.path) return '(未设置)'
    return `-${rule.path}`
  } else if (rule.action === 'rename') {
    if (!rule.from || !rule.to) return '(未设置)'
    return `${rule.from}→${rule.to}`
  } else if (rule.action === 'append') {
    if (!rule.path) return '(未设置)'
    return `${rule.path}[]+=${rule.value || '...'}`
  } else if (rule.action === 'insert') {
    if (!rule.path) return '(未设置)'
    const idx = rule.index?.trim() || '末尾'
    return `${rule.path}[${idx}]+=${rule.value || '...'}`
  } else if (rule.action === 'regex_replace') {
    if (!rule.path || !rule.pattern) return '(未设置)'
    const flags = rule.flags.trim()
    const count = rule.count.trim()
    return `${rule.path}: s/${rule.pattern}/${rule.replacement || ''}/${flags}${count ? ` ×${count}` : ''}`
  } else if (rule.action === 'name_style') {
    if (!rule.path || !rule.style) return '(未设置)'
    return `${rule.path}→${rule.style}`
  }
  return '(未知)'
}

// 检查端点请求体规则是否有修改
function hasBodyRulesChanges(endpoint: ProviderEndpoint): boolean {
  const state = endpointEditStates.value[endpoint.id]
  if (!state) return false

  const originalRules = endpoint.body_rules || []
  const editedRules = state.bodyRules.filter(isBodyRuleEffective)
  if (editedRules.length !== originalRules.length) return true
  for (let i = 0; i < editedRules.length; i++) {
    const edited = editedRules[i]
    const original = originalRules[i]
    if (!original) return true
    if (edited.action !== original.action) return true
    if (edited.action === 'set' && original.action === 'set') {
      const baseline = initBodyRuleSetValueForEditor(original.value)
      if (edited.path !== original.path) return true
      if (edited.value !== baseline.value) return true
    } else if (edited.action === 'drop' && original.action === 'drop') {
      if (edited.path !== original.path) return true
    } else if (edited.action === 'rename' && original.action === 'rename') {
      if (edited.from !== original.from || edited.to !== original.to) return true
    } else if (edited.action === 'append' && original.action === 'append') {
      const baseline = initBodyRuleSetValueForEditor(original.value)
      if (edited.path !== original.path) return true
      if (edited.value !== baseline.value) return true
    } else if (edited.action === 'insert' && original.action === 'insert') {
      const baseline = initBodyRuleSetValueForEditor(original.value)
      if (edited.path !== original.path) return true
      if (edited.index !== String(original.index ?? '')) return true
      if (edited.value !== baseline.value) return true
    } else if (edited.action === 'regex_replace' && original.action === 'regex_replace') {
      if (edited.path !== original.path) return true
      if (edited.pattern !== (original.pattern ?? '')) return true
      if (edited.replacement !== (original.replacement ?? '')) return true
      if (edited.flags !== (original.flags ?? '')) return true
      if (edited.count !== (original.count === undefined || original.count === null ? '' : String(original.count))) return true
    } else if (edited.action === 'name_style' && original.action === 'name_style') {
      if (edited.path !== (original.path ?? '')) return true
      if (edited.style !== (original.style ?? '')) return true
    }
    if (!conditionEquals(edited.condition, conditionToEditable(original.condition))) return true
  }
  return false
}

// 将可编辑请求体规则数组转换为 API 需要的 BodyRule[]
function rulesToBodyRules(rules: EditableBodyRule[]): BodyRule[] | null {
  const result: BodyRule[] = []

  for (const rule of rules) {
    const condition = editableConditionToApi(rule.condition)
    if (rule.action === 'set' && rule.path.trim()) {
      let value: unknown = rule.value
      try { value = restoreOriginalPlaceholder(JSON.parse(prepareValueForJsonParse(rule.value.trim()))) } catch { value = rule.value }
      result.push({ action: 'set', path: rule.path.trim(), value, ...(condition ? { condition } : {}) })
    } else if (rule.action === 'drop' && rule.path.trim()) {
      result.push({ action: 'drop', path: rule.path.trim(), ...(condition ? { condition } : {}) })
    } else if (rule.action === 'rename' && rule.from.trim() && rule.to.trim()) {
      result.push({ action: 'rename', from: rule.from.trim(), to: rule.to.trim(), ...(condition ? { condition } : {}) })
    } else if (rule.action === 'append' && rule.path.trim()) {
      let value: unknown = rule.value
      try { value = restoreOriginalPlaceholder(JSON.parse(prepareValueForJsonParse(rule.value.trim()))) } catch { value = rule.value }
      result.push({ action: 'append', path: rule.path.trim(), value, ...(condition ? { condition } : {}) })
    } else if (rule.action === 'insert' && rule.path.trim()) {
      let value: unknown = rule.value
      try { value = restoreOriginalPlaceholder(JSON.parse(prepareValueForJsonParse(rule.value.trim()))) } catch { value = rule.value }
      if (!isStrictIntegerString(rule.index)) continue
      const idx = parseInt(rule.index.trim(), 10)
      result.push({ action: 'insert', path: rule.path.trim(), index: idx, value, ...(condition ? { condition } : {}) })
    } else if (rule.action === 'regex_replace' && rule.path.trim() && rule.pattern.trim()) {
      const entry: BodyRuleRegexReplace = {
        action: 'regex_replace',
        path: rule.path.trim(),
        pattern: rule.pattern,
        replacement: rule.replacement || '',
        ...(rule.flags.trim() ? { flags: rule.flags.trim() } : {}),
        ...(isStrictNonNegativeIntegerString(rule.count) ? { count: parseInt(rule.count.trim(), 10) } : {}),
      }
      result.push({ ...entry, ...(condition ? { condition } : {}) })
    } else if (rule.action === 'name_style' && rule.path.trim() && rule.style.trim()) {
      result.push({ action: 'name_style', path: rule.path.trim(), style: rule.style.trim() as BodyRuleNameStyle['style'], ...(condition ? { condition } : {}) })
    }
  }

  return result.length > 0 ? result : null
}

function getBodyValidationErrorForEndpoint(endpointId: string): string | null {
  const rules = getEndpointEditBodyRules(endpointId)
  for (let i = 0; i < rules.length; i++) {
    const rule = rules[i]
    const prefix = `第 ${i + 1} 条请求体规则：`

    if (rule.action === 'set' || rule.action === 'drop') {
      const pathErr = validateBodyRulePathForEndpoint(endpointId, rule.path, i)
      if (pathErr) return `${prefix}${pathErr}`
      if (rule.action === 'set') {
        const valueErr = validateBodySetValue(rule)
        if (valueErr) return `${prefix}${valueErr}`
      }
    } else if (rule.action === 'rename') {
      const fromErr = validateBodyRenameFromForEndpoint(endpointId, rule.from, i)
      if (fromErr) return `${prefix}${fromErr}`
      const toErr = validateBodyRenameToForEndpoint(endpointId, rule.to, i)
      if (toErr) return `${prefix}${toErr}`
    } else if (rule.action === 'append') {
      const pathErr = validateBodyRulePathForEndpoint(endpointId, rule.path, i)
      if (pathErr) return `${prefix}${pathErr}`
      const valueErr = validateBodySetValue(rule)
      if (valueErr) return `${prefix}${valueErr}`
    } else if (rule.action === 'insert') {
      const pathErr = validateBodyRulePathForEndpoint(endpointId, rule.path, i)
      if (pathErr) return `${prefix}${pathErr}`
      const indexStr = rule.index.trim()
      if (!indexStr) return `${prefix}插入位置不能为空`
      if (!isStrictIntegerString(indexStr)) return `${prefix}位置必须为整数`
      const valueErr = validateBodySetValue(rule)
      if (valueErr) return `${prefix}${valueErr}`
    } else if (rule.action === 'regex_replace') {
      const pathErr = validateBodyRulePathForEndpoint(endpointId, rule.path, i)
      if (pathErr) return `${prefix}${pathErr}`
      if (!rule.pattern.trim()) return `${prefix}正则表达式不能为空`
      try {
        new RegExp(rule.pattern.trim())
      } catch (err: unknown) {
        return `${prefix}正则表达式无效：${err instanceof Error ? err.message : String(err)}`
      }
      const flags = rule.flags.trim()
      if (flags) {
        const validFlags = new Set(['i', 'm', 's'])
        for (const f of flags) {
          if (!validFlags.has(f)) return `${prefix}flags 仅允许 i/m/s，非法字符: ${f}`
        }
      }
      const count = rule.count.trim()
      if (count) {
        if (!isStrictNonNegativeIntegerString(count)) return `${prefix}替换次数必须是大于等于 0 的整数`
      }
    } else if (rule.action === 'name_style') {
      if (!rule.path.trim()) return `${prefix}路径不能为空`
      const validStyles = new Set(['snake_case', 'camelCase', 'PascalCase', 'kebab-case', 'capitalize'])
      if (!rule.style.trim() || !validStyles.has(rule.style.trim())) return `${prefix}请选择有效的命名风格`
    }

    const conditionErr = validateEditableCondition(rule.condition)
    if (conditionErr) return `${prefix}${conditionErr}`
  }
  return null
}

// 检查请求体规则是否有验证错误
function _hasBodyValidationErrorsForEndpoint(endpointId: string): boolean {
  return !!getBodyValidationErrorForEndpoint(endpointId)
}

// 检查端点 URL/路径是否有修改
function hasUrlChanges(endpoint: ProviderEndpoint): boolean {
  const state = endpointEditStates.value[endpoint.id]
  if (!state) return false
  if (state.url !== endpoint.base_url) return true
  if (state.path !== (endpoint.custom_path || '')) return true
  // 注：upstreamStreamPolicy 现在由头部按钮直接保存，无需在此检查
  return false
}

// 检查端点规则是否有修改
function hasRulesChanges(endpoint: ProviderEndpoint): boolean {
  const state = endpointEditStates.value[endpoint.id]
  if (!state) return false

  const originalRules = endpoint.header_rules || []
  const editedRules = state.rules.filter(r => {
    if (r.action === 'set' || r.action === 'drop') return r.key.trim()
    if (r.action === 'rename') return r.from.trim() && r.to.trim()
    return false
  })
  if (editedRules.length !== originalRules.length) return true
  for (let i = 0; i < editedRules.length; i++) {
    const edited = editedRules[i]
    const original = originalRules[i]
    if (!original) return true
    if (edited.action !== original.action) return true
    if (edited.action === 'set' && original.action === 'set') {
      if (edited.key !== original.key || edited.value !== (original.value || '')) return true
    } else if (edited.action === 'drop' && original.action === 'drop') {
      if (edited.key !== original.key) return true
    } else if (edited.action === 'rename' && original.action === 'rename') {
      if (edited.from !== original.from || edited.to !== original.to) return true
    }
    if (!conditionEquals(edited.condition, conditionToEditable(original.condition))) return true
  }
  return false
}

// 检查端点是否有修改（URL、路径或规则）
// 注：当前模板直接使用各子函数，此聚合函数保留供未来使用
function _hasEndpointChanges(endpoint: ProviderEndpoint): boolean {
  return hasUrlChanges(endpoint) || hasRulesChanges(endpoint) || hasBodyRulesChanges(endpoint)
}

// 重置端点修改
function resetEndpointChanges(endpoint: ProviderEndpoint) {
  endpointEditStates.value[endpoint.id] = initEndpointEditState(endpoint)
}

async function handleResetBodyRulesToDefault(endpoint: ProviderEndpoint) {
  resettingDefaultRulesEndpointId.value = endpoint.id
  try {
    const defaultRules = await loadDefaultBodyRulesForFormat(endpoint.api_format, true)
    if (!defaultRules.length) {
      showError('该端点没有默认请求体规则')
      return
    }

    if (!endpointEditStates.value[endpoint.id]) {
      endpointEditStates.value[endpoint.id] = initEndpointEditState(endpoint)
    }
    const state = endpointEditStates.value[endpoint.id]
    if (!state) return

    const resetState = initEndpointEditState({
      ...endpoint,
      body_rules: defaultRules,
    })
    state.bodyRules = resetState.bodyRules
    endpointRulesExpanded.value[endpoint.id] = (state.rules.length + state.bodyRules.length) > 0
    clearBodyRuleDragState(endpoint.id)
    clearBodyRuleSelectOpen(endpoint.id)
    success('已重置请求体为默认规则，请点击保存生效')
  } catch (error: unknown) {
    showError(parseApiError(error, '重置失败'), '错误')
  } finally {
    resettingDefaultRulesEndpointId.value = null
  }
}

// 将可编辑规则数组转换为 API 需要的 HeaderRule[]
function rulesToHeaderRules(rules: EditableRule[]): HeaderRule[] | null {
  const result: HeaderRule[] = []

  for (const rule of rules) {
    const condition = editableConditionToApi(rule.condition)
    if (rule.action === 'set' && rule.key.trim()) {
      result.push({ action: 'set', key: rule.key.trim(), value: rule.value, ...(condition ? { condition } : {}) })
    } else if (rule.action === 'drop' && rule.key.trim()) {
      result.push({ action: 'drop', key: rule.key.trim(), ...(condition ? { condition } : {}) })
    } else if (rule.action === 'rename' && rule.from.trim() && rule.to.trim()) {
      result.push({ action: 'rename', from: rule.from.trim(), to: rule.to.trim(), ...(condition ? { condition } : {}) })
    }
  }

  return result.length > 0 ? result : null
}

function getHeaderValidationErrorForEndpoint(endpointId: string): string | null {
  const rules = getEndpointEditRules(endpointId)
  for (let i = 0; i < rules.length; i++) {
    const rule = rules[i]
    const prefix = `第 ${i + 1} 条请求头规则：`
    if (rule.action === 'set' || rule.action === 'drop') {
      const err = validateRuleKeyForEndpoint(endpointId, rule.key, i)
      if (err) return `${prefix}${err}`
    } else if (rule.action === 'rename') {
      const fromErr = validateRenameFromForEndpoint(endpointId, rule.from, i)
      if (fromErr) return `${prefix}${fromErr}`
      const toErr = validateRenameToForEndpoint(endpointId, rule.to, i)
      if (toErr) return `${prefix}${toErr}`
    }
    const conditionErr = validateEditableCondition(rule.condition)
    if (conditionErr) return `${prefix}${conditionErr}`
  }
  return null
}

// 新端点选择的格式的默认路径
const newEndpointDefaultPath = computed(() => {
  // 使用填写的 base_url 或 provider 的 website 来判断是否是 Codex 端点
  const baseUrl = newEndpoint.value.base_url || props.provider?.website || ''
  return getDefaultPath(newEndpoint.value.api_format, baseUrl)
})

// 加载 API 格式列表
const loadApiFormats = async () => {
  try {
    const response = await adminApi.getApiFormats()
    apiFormats.value = response.formats
  } catch (error) {
    log.error('加载API格式失败:', error)
  }
}

onMounted(() => {
  loadApiFormats()
})

// 监听 props 变化
watch(() => props.modelValue, (open) => {
  bodyRuleHelpOpenEndpointId.value = null
  ruleSelectOpen.value = {}
  bodyRuleSelectOpen.value = {}
  headerRuleDraggedIndex.value = {}
  headerRuleDragOverIndex.value = {}
  bodyRuleDraggedIndex.value = {}
  bodyRuleDragOverIndex.value = {}
  if (open) {
    localEndpoints.value = [...(props.endpoints || [])]
    // 清空编辑状态，重新从端点加载
    endpointEditStates.value = {}
    endpointRulesExpanded.value = {}
    // 初始化每个端点的编辑状态，有规则时默认展开
    for (const endpoint of localEndpoints.value) {
      endpointEditStates.value[endpoint.id] = initEndpointEditState(endpoint)
      // 有规则时默认展开
      const hasRules = (endpoint.header_rules?.length || 0) + (endpoint.body_rules?.length || 0) > 0
      endpointRulesExpanded.value[endpoint.id] = hasRules
    }
    void preloadDefaultBodyRules(localEndpoints.value)
  } else {
    // 关闭对话框时完全清空新端点表单
    newEndpoint.value = { api_format: '', base_url: '', custom_path: '' }
  }
}, { immediate: true })

watch(() => props.endpoints, (endpoints) => {
  if (props.modelValue) {
    localEndpoints.value = [...(endpoints || [])]
    // 初始化新添加端点的编辑状态
    for (const endpoint of localEndpoints.value) {
      if (!endpointEditStates.value[endpoint.id]) {
        endpointEditStates.value[endpoint.id] = initEndpointEditState(endpoint)
      }
    }
    const newFormats = localEndpoints.value
      .filter(e => e.api_format && !defaultBodyRulesLoaded.value[defaultBodyRulesCacheKey(e.api_format)])
      .map(e => ({ api_format: e.api_format }) as ProviderEndpoint)
    if (newFormats.length) {
      void preloadDefaultBodyRules(newFormats)
    }
  }
}, { deep: true })

// 保存端点
async function saveEndpoint(endpoint: ProviderEndpoint) {
  const state = endpointEditStates.value[endpoint.id]
  if (!state || !state.url) return

  // 检查规则是否有验证错误
  const headerErr = getHeaderValidationErrorForEndpoint(endpoint.id)
  if (headerErr) {
    showError(headerErr)
    return
  }

  // 检查请求体规则是否有验证错误
  const bodyErr = getBodyValidationErrorForEndpoint(endpoint.id)
  if (bodyErr) {
    showError(bodyErr)
    return
  }

  savingEndpointId.value = endpoint.id
  try {
    // 仅提交变更字段，避免 fixed provider 因 base_url/custom_path 被锁定而更新失败
    const payload: Record<string, unknown> = {}

    if (!isFixedProvider.value) {
      if (state.url !== endpoint.base_url) payload.base_url = state.url
      if (state.path !== (endpoint.custom_path || '')) payload.custom_path = state.path || null
    }

    if (hasRulesChanges(endpoint)) payload.header_rules = rulesToHeaderRules(state.rules)
    if (hasBodyRulesChanges(endpoint)) payload.body_rules = rulesToBodyRules(state.bodyRules)

    // 注：upstreamStreamPolicy 现在由头部按钮直接保存，不在此处处理

    if (Object.keys(payload).length === 0) return

    await updateEndpoint(endpoint.id, payload)
    success('端点已更新')
    emit('endpointUpdated')
  } catch (error: unknown) {
    showError(parseApiError(error, '更新失败'), '错误')
  } finally {
    savingEndpointId.value = null
  }
}

// 切换格式转换（直接保存）
async function handleToggleFormatConversion(endpoint: ProviderEndpoint) {
  const currentEnabled = endpoint.format_acceptance_config?.enabled || false
  const newEnabled = !currentEnabled

  togglingFormatEndpointId.value = endpoint.id
  try {
    await updateEndpoint(endpoint.id, {
      format_acceptance_config: newEnabled ? { enabled: true } : null,
    })
    success(newEnabled ? '已启用格式转换' : '已关闭格式转换')
    emit('endpointUpdated')
  } catch (error: unknown) {
    showError(parseApiError(error, '操作失败'), '错误')
  } finally {
    togglingFormatEndpointId.value = null
  }
}

// 获取上游流式按钮的当前状态（优先使用编辑状态）
function getCurrentUpstreamStreamPolicy(endpoint: ProviderEndpoint): string {
  if (isUpstreamStreamPolicyLocked(endpoint)) return 'force_stream'
  const state = endpointEditStates.value[endpoint.id]
  return state?.upstreamStreamPolicy ?? getEndpointUpstreamStreamPolicy(endpoint)
}

function isUpstreamStreamPolicyLocked(endpoint: ProviderEndpoint): boolean {
  return (props.provider?.provider_type || '').toLowerCase() === 'codex'
    && normalizeLegacyOpenAIFormatAlias(endpoint.api_format) === 'openai:responses'
}

// 获取上游流式按钮的样式类
function getUpstreamStreamButtonClass(endpoint: ProviderEndpoint): string {
  if (isUpstreamStreamPolicyLocked(endpoint)) {
    return 'h-7 w-7 text-primary/70 cursor-not-allowed'
  }
  const policy = getCurrentUpstreamStreamPolicy(endpoint)
  const base = 'h-7 w-7'
  if (policy === 'force_stream') return `${base} text-primary`
  if (policy === 'force_non_stream') return `${base} text-destructive`
  return `${base} text-muted-foreground` // auto - 跟随请求，淡色显示
}

// 获取上游流式按钮的提示文字
function getUpstreamStreamTooltip(endpoint: ProviderEndpoint): string {
  if (isUpstreamStreamPolicyLocked(endpoint)) return '固定流式（Codex OpenAI Responses，已锁定）'
  const policy = getCurrentUpstreamStreamPolicy(endpoint)
  if (policy === 'force_stream') return '固定流式（点击切换为固定非流）'
  if (policy === 'force_non_stream') return '固定非流（点击切换为跟随请求）'
  return '跟随请求（点击切换为固定流式）'
}

// 循环切换上游流式策略并直接保存
async function handleCycleUpstreamStream(endpoint: ProviderEndpoint) {
  if (isUpstreamStreamPolicyLocked(endpoint)) return

  const currentPolicy = getCurrentUpstreamStreamPolicy(endpoint)
  let nextPolicy: string
  let nextLabel: string

  // 循环：auto -> force_stream -> force_non_stream -> auto
  if (currentPolicy === 'auto') {
    nextPolicy = 'force_stream'
    nextLabel = '固定流式'
  } else if (currentPolicy === 'force_stream') {
    nextPolicy = 'force_non_stream'
    nextLabel = '固定非流'
  } else {
    nextPolicy = 'auto'
    nextLabel = '跟随请求'
  }

  savingEndpointId.value = endpoint.id
  try {
    const merged: Record<string, unknown> = { ...(endpoint.config || {}) }
    // 清理旧的 key
    delete merged.upstream_stream_policy
    delete merged.upstreamStreamPolicy
    delete merged.upstream_stream

    if (nextPolicy !== 'auto') {
      merged.upstream_stream_policy = nextPolicy
    }

    await updateEndpoint(endpoint.id, {
      config: Object.keys(merged).length > 0 ? merged : null,
    })

    // 更新本地编辑状态
    if (endpointEditStates.value[endpoint.id]) {
      endpointEditStates.value[endpoint.id].upstreamStreamPolicy = nextPolicy
    }

    success(`已切换为${nextLabel}`)
    emit('endpointUpdated')
  } catch (error: unknown) {
    showError(parseApiError(error, '操作失败'), '错误')
  } finally {
    savingEndpointId.value = null
  }
}

// 添加端点
async function handleAddEndpoint() {
  if (!props.provider || !newEndpoint.value.api_format) return

  // 如果没有输入 base_url，使用提供商的 website 作为默认值
  const baseUrl = newEndpoint.value.base_url || props.provider.website
  if (!baseUrl) {
    showError('请输入 Base URL')
    return
  }

  addingEndpoint.value = true
  try {
    await createEndpoint(props.provider.id, {
      provider_id: props.provider.id,
      api_format: newEndpoint.value.api_format,
      base_url: baseUrl,
      custom_path: newEndpoint.value.custom_path || undefined,
      is_active: true,
    })
    success(`已添加 ${formatApiFormat(newEndpoint.value.api_format)} 端点`)
    // 重置表单，保留 URL
    newEndpoint.value = { api_format: '', base_url: baseUrl, custom_path: '' }
    emit('endpointCreated')
  } catch (error: unknown) {
    showError(parseApiError(error, '添加失败'), '错误')
  } finally {
    addingEndpoint.value = false
  }
}

// 切换端点启用状态
async function handleToggleEndpoint(endpoint: ProviderEndpoint) {
  togglingEndpointId.value = endpoint.id
  try {
    const newStatus = !endpoint.is_active
    await updateEndpoint(endpoint.id, { is_active: newStatus })
    success(newStatus ? '端点已启用' : '端点已停用')
    emit('endpointUpdated')
  } catch (error: unknown) {
    showError(parseApiError(error, '操作失败'), '错误')
  } finally {
    togglingEndpointId.value = null
  }
}

// 删除端点 - 打开确认弹窗
function handleDeleteEndpoint(endpoint: ProviderEndpoint) {
  endpointToDelete.value = endpoint
  deleteConfirmOpen.value = true
}

// 确认删除端点
async function confirmDeleteEndpoint() {
  if (!endpointToDelete.value) return

  const endpoint = endpointToDelete.value
  deleteConfirmOpen.value = false
  deletingEndpointId.value = endpoint.id

  try {
    await deleteEndpoint(endpoint.id)
    success(`已删除 ${formatApiFormat(endpoint.api_format)} 端点`)
    emit('endpointUpdated')
  } catch (error: unknown) {
    showError(parseApiError(error, '删除失败'), '错误')
  } finally {
    deletingEndpointId.value = null
    endpointToDelete.value = null
  }
}

// 关闭对话框
function handleDialogUpdate(value: boolean) {
  emit('update:modelValue', value)
}

function handleClose() {
  emit('update:modelValue', false)
}
</script>
