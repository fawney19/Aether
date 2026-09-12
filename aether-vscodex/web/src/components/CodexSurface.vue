<template>
  <div class="codex-panel">
    <div class="connection" aria-live="polite" hidden>
      <span id="connectionDot" class="dot offline"></span>
      <span id="connectionText">正在连接</span>
      <span id="roleBadge" class="badge">未认证</span>
    </div>

    <main class="chat-shell">
      <section class="chat-header" aria-label="当前会话">
        <div class="thread-heading">
          <button id="backButton" class="icon-button header-back-button" type="button" data-panel-action="back" title="返回会话列表" aria-label="返回会话列表" hidden>
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M9.75 3.5 5.25 8l4.5 4.5M5.5 8h6.25" /></svg>
          </button>
          <button id="sessionPickerButton" class="thread-picker-button" type="button" aria-haspopup="dialog" aria-expanded="false" title="打开会话历史" aria-label="打开会话历史" disabled>
            <h2 id="threadTitle">Codex</h2>
          </button>
          <span id="appState" class="status-text" aria-live="polite">等待 VS Code 主机</span>
        </div>
        <div class="thread-actions">
          <button class="icon-button" type="button" data-panel-action="menu" title="更多操作" aria-label="更多操作">
            <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3" cy="8" r="1" /><circle cx="8" cy="8" r="1" /><circle cx="13" cy="8" r="1" /></svg>
          </button>
          <button id="historyButton" class="icon-button header-history-button" type="button" data-panel-action="history" title="会话历史" aria-label="会话历史" hidden>
            <svg viewBox="0 0 20 20" aria-hidden="true"><path d="M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" /><path d="M3 3v5h5" /><path d="M12 7v5l4 2" /></svg>
          </button>
          <button class="icon-button" type="button" data-panel-action="settings" title="设置" aria-label="设置">
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M6.7 2h2.6l.4 1.6c.4.2.8.4 1.2.7l1.6-.6 1.3 2.2-1.2 1.1a5 5 0 0 1 0 1.4l1.2 1.1-1.3 2.2-1.6-.6c-.4.3-.8.5-1.2.7L9.3 14H6.7l-.4-1.6a5 5 0 0 1-1.2-.7l-1.6.6-1.3-2.2 1.2-1.1a5 5 0 0 1 0-1.4L2.2 6l1.3-2.2 1.6.6c.4-.3.8-.5 1.2-.7L6.7 2Z" /><circle cx="8" cy="8" r="1.7" /></svg>
          </button>
          <button id="newSessionButton" class="icon-button new-session-button" type="button" data-panel-action="new-session" title="创建新会话" aria-label="创建新会话" hidden>
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.25 3.25h5.5a1.5 1.5 0 0 1 1.5 1.5v2.5" /><path d="M3.25 3.25v9.5h6" /><path d="m8.2 11.35 4.55-4.55 1.25 1.25-4.55 4.55-2 .5Z" /></svg>
          </button>
        </div>
      </section>

      <div id="panelMenu" class="panel-popover panel-menu" hidden>
        <button type="button" data-menu-action="sessions" hidden>最近会话</button>
        <button type="button" data-menu-action="clear">清空当前输出</button>
        <button type="button" data-menu-action="refresh">重新同步</button>
        <button type="button" data-menu-action="expand">展开面板</button>
        <button type="button" data-menu-action="close">隐藏面板</button>
      </div>

      <div id="detailsPopover" class="panel-popover details-popover settings-popover" hidden role="dialog" aria-label="设置">
        <div class="popover-title">设置</div>
        <div class="settings-shortcuts">
          <button type="button" data-settings-action="model"><span>模型与推理强度</span><span id="settingsModelValue">默认</span></button>
          <button type="button" data-settings-action="permission"><span>修改权限</span><span id="settingsPermissionValue">工作区写入</span></button>
          <label id="localeSetting" class="settings-locale">
            <span>语言</span>
            <select id="localeSelect" aria-label="语言">
              <option value="zh-CN">中文</option>
              <option value="en-US">English</option>
            </select>
          </label>
        </div>
        <div class="settings-divider"></div>
        <div class="popover-subtitle">当前会话</div>
        <dl>
          <dt>工作区</dt><dd id="popoverCwd">-</dd>
          <dt>模式</dt><dd id="popoverMode">本地模式</dd>
          <dt>thread</dt><dd id="popoverThread">-</dd>
        </dl>
      </div>

      <div id="sessionPicker" class="panel-popover session-picker" hidden role="dialog" aria-label="最近会话">
        <div class="session-picker-header">
          <span class="popover-title">最近会话</span>
          <button id="sessionPickerRefresh" class="session-picker-refresh" type="button" title="刷新会话列表" aria-label="刷新会话列表">
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13 5V2m0 0h-3m3 0-2.1 2.1A5 5 0 1 0 13 9" /></svg>
          </button>
        </div>
        <div class="session-search">
          <svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="6.8" cy="6.8" r="3.8" /><path d="m9.7 9.7 3.2 3.2" /></svg>
          <label class="sr-only" for="sessionSearchInput">搜索最近会话</label>
          <input id="sessionSearchInput" type="search" autocomplete="off" spellcheck="false" placeholder="搜索最近会话" aria-label="搜索最近会话" aria-controls="sessionList" aria-expanded="false" />
          <button id="sessionSearchClear" class="session-search-clear" type="button" title="清除搜索" aria-label="清除搜索" hidden>
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4.5 4.5 7 7m0-7-7 7" /></svg>
          </button>
        </div>
        <div id="sessionPickerStatus" class="session-picker-status" role="status" aria-live="polite"></div>
        <div id="sessionList" class="session-list" role="listbox" aria-label="可用会话" tabindex="0"></div>
      </div>

      <section class="chat-panel" aria-label="对话内容">
        <div id="output" class="output chat-scroll" tabindex="0" aria-live="polite" aria-label="Codex 消息"></div>
        <button id="scrollToBottom" class="scroll-to-bottom" type="button" aria-label="回到最新消息" aria-hidden="true" tabindex="-1">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v9M4.5 8.5 8 12l3.5-3.5" /></svg>
          <span class="scroll-working-dots" aria-hidden="true"><i></i><i></i><i></i></span>
        </button>
        <div id="inlineRequests" class="inline-requests" aria-live="polite" aria-label="待处理的 Codex 请求"></div>
      </section>

      <section id="messageForm" class="composer" aria-label="发送消息">
        <section id="subagentsPanel" class="subagents-panel" aria-label="子代理" hidden>
          <button id="subagentsToggle" class="subagents-toggle" type="button" aria-expanded="false">
            <span class="subagents-title">子代理</span>
            <span id="subagentsCount" class="subagents-count"></span>
            <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m6 3 5 5-5 5" /></svg>
          </button>
          <div id="subagentsList" class="subagents-list"></div>
        </section>

        <div id="liveActivity" class="live-activity" role="status" aria-live="polite" hidden>
          <span class="activity-spinner" aria-hidden="true"></span>
          <span class="activity-label"></span>
          <span class="activity-dots" aria-hidden="true"><i></i><i></i><i></i></span>
          <span class="activity-elapsed"></span>
        </div>

        <div class="composer-surface">
          <div id="messageInput" class="composer-editor" contenteditable="true" role="textbox" aria-multiline="true" data-placeholder="提交后续变更要求" spellcheck="true"></div>
          <div class="composer-footer">
            <div class="composer-hint">
              <button id="composerPlusButton" class="composer-icon-button" type="button" aria-haspopup="menu" aria-expanded="false" title="添加文件及更多内容" aria-label="添加文件及更多内容">
                <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 3v10M3 8h10" /></svg>
              </button>
              <div id="composerPlusMenu" class="composer-popover composer-plus-menu" role="menu" hidden>
                <div class="composer-popover-heading">添加文件及更多内容</div>
                <button type="button" role="menuitem" data-composer-action="attach">添加文件</button>
                <button type="button" role="menuitem" data-composer-action="photo">添加照片</button>
                <button type="button" role="menuitem" data-composer-action="workspace">添加工作区上下文</button>
                <button type="button" role="menuitem" data-composer-action="web-search">网页搜索</button>
              </div>
              <input id="attachmentInput" type="file" accept=".txt,.md,.json,.js,.ts,.tsx,.jsx,.css,.html,.yml,.yaml,.xml,.py,.go,.rs,.java,.c,.cpp,.h,image/*" multiple hidden />

              <button id="permissionChip" class="permission-chip" type="button" aria-haspopup="menu" aria-expanded="false" title="修改权限" aria-label="修改权限">
                <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 1.8 13 4v3.6c0 3-2 5.6-5 6.6-3-1-5-3.6-5-6.6V4l5-2.2Z" /><path d="m5.5 8 1.6 1.6L10.8 6" /></svg>
                <span id="permissionLabel">工作区写入</span>
                <svg class="permission-chevron" viewBox="0 0 16 16" aria-hidden="true"><path d="m4.5 6 3.5 3.5L11.5 6" /></svg>
              </button>
              <div id="permissionMenu" class="composer-popover permission-menu" role="menu" aria-label="权限设置" hidden>
                <div class="composer-popover-heading">修改权限</div>
                <button type="button" role="menuitemradio" data-permission-mode="ask" aria-checked="false"><span>需要时询问</span><small>编辑外部文件和联网时始终询问</small></button>
                <button type="button" role="menuitemradio" data-permission-mode="auto" aria-checked="false"><span>由 Codex 审批</span><small>仅对可能不安全的操作询问</small></button>
                <button type="button" role="menuitemradio" data-permission-mode="full" aria-checked="false"><span>完全访问</span><small>不限制联网或文件访问</small></button>
                <button type="button" role="menuitemradio" data-permission-mode="custom" aria-checked="false"><span>自定义</span><small>使用 config.toml 中的权限</small></button>
                <button type="button" role="menuitemradio" data-permission-mode="readonly" aria-checked="false"><span>只读</span><small>仅查看文件，不修改工作区</small></button>
              </div>

              <div id="permissionConfirm" class="permission-confirm" role="dialog" aria-modal="true" aria-labelledby="permissionConfirmTitle" hidden>
                <div id="permissionConfirmTitle" class="permission-confirm-title">确认完全访问</div>
                <p>完全访问允许 Codex 执行命令、访问互联网并编辑工作区之外的文件。</p>
                <div class="permission-confirm-actions">
                  <button id="permissionConfirmCancel" type="button">取消</button>
                  <button id="permissionConfirmAccept" class="primary" type="button">确认</button>
                </div>
              </div>

              <div id="usagePicker" class="usage-picker" hidden>
                <button id="usageButton" class="usage-button" type="button" aria-haspopup="dialog" aria-expanded="false" title="查看上下文用量" aria-label="查看上下文用量"><span id="usageRing" class="usage-ring" aria-hidden="true"><span id="usageLabel">0%</span></span></button>
                <div id="usageMenu" class="composer-popover usage-menu" role="dialog" aria-label="上下文用量" hidden>
                  <div class="composer-popover-heading">上下文用量</div>
                  <div id="usageSummary" class="usage-summary">暂无用量数据</div>
                  <div class="usage-meter"><span id="usageMeterBar"></span></div>
                  <div id="usageDetails" class="usage-details"></div>
                </div>
              </div>

              <span id="factApp" class="sr-only">-</span>
              <span id="factClients" class="sr-only">-</span>
              <span id="factRequests" class="sr-only">0</span>
            </div>

            <div class="composer-actions">
              <div id="modelPicker" class="model-picker">
                <button id="modelPickerButton" class="model-picker-button" type="button" aria-haspopup="menu" aria-expanded="false" title="切换模型与推理强度" hidden>
                  <span id="modelLabel" class="model-label"></span>
                  <span id="modelEffortLabel" class="model-effort-label"></span>
                  <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m4.5 6 3.5 3.5L11.5 6" /></svg>
                </button>
                <div id="modelMenu" class="model-menu" role="menu" aria-label="模型与推理强度" hidden>
                  <div id="modelPowerView" class="model-power-view">
                    <div class="model-power-heading">
                      <span>推理强度</span>
                      <button id="modelAdvancedToggle" class="model-advanced-toggle" type="button">高级</button>
                    </div>
                    <div class="model-power-control">
                      <span class="model-power-label">更高效</span>
                      <input id="modelPowerSlider" class="model-power-slider" type="range" min="0" max="3" step="1" value="1" aria-label="强度" aria-describedby="modelPowerInstructions" />
                      <span class="model-power-label">更智能</span>
                    </div>
                    <div id="modelPowerValue" class="model-power-value"></div>
                    <span id="modelPowerInstructions" class="sr-only">使用左右方向键调整强度</span>
                  </div>
                  <div id="modelAdvancedView" class="model-advanced-view" hidden>
                    <div class="model-advanced-toolbar">
                      <button id="modelAdvancedBack" class="model-advanced-back" type="button" aria-label="返回模型强度">‹</button>
                      <span>模型与推理强度</span>
                    </div>
                    <div class="model-menu-heading">模型</div>
                    <div id="modelOptions" class="model-options" role="listbox" aria-label="模型"></div>
                    <div class="model-menu-heading effort-heading">推理强度</div>
                    <div id="effortOptions" class="effort-options" role="listbox" aria-label="推理强度"></div>
                  </div>
                </div>
              </div>

              <button id="interruptButton" class="compact-action interrupt-action" type="button" disabled title="中断当前 turn" aria-label="中断当前 turn">
                <svg viewBox="0 0 16 16" aria-hidden="true"><rect x="4.5" y="4.5" width="7" height="7" rx="1" /></svg>
              </button>
              <button id="steerButton" class="primary compact-action steer-action" type="button" disabled title="发送后续指令" aria-label="发送后续指令">
                <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 12V4M4.5 7.5 8 4l3.5 3.5" /></svg>
              </button>
              <button id="startTurnButton" class="primary send-button" type="button" disabled title="发送消息" aria-label="发送消息">
                <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M8 12V4M4.5 7.5 8 4l3.5 3.5" /></svg>
              </button>
            </div>
          </div>
        </div>

        <div class="mode-row">
          <span class="connection-mode-label">
            <svg class="mode-icon" viewBox="0 0 16 16" aria-hidden="true"><rect x="2" y="3" width="12" height="8" rx="1" /><path d="M5 13h6M8 11v2" /></svg>
            <span id="modeLabel">本地模式</span>
          </span>
          <div id="controlModeSwitch" class="control-mode-switch" role="group" aria-label="控制模式" aria-busy="false" data-mode="sync" data-switching="false">
            <button type="button" data-control-mode="sync" aria-pressed="true" title="同步模式跟随 VS Code 当前会话" disabled>同步</button>
            <button type="button" data-control-mode="async" aria-pressed="false" title="异步模式可独立管理会话" disabled>异步</button>
          </div>
        </div>
      </section>
    </main>
  </div>

  <button id="restorePanel" class="restore-panel" type="button" hidden>显示 Codex</button>

  <section class="compatibility-state" aria-hidden="true" hidden inert>
    <details id="sessionSettings">
      <summary>会话设置</summary>
      <div class="settings-grid">
        <label>工作目录<input id="cwdInput" type="text" /></label>
        <label>模型<input id="modelInput" type="text" placeholder="留空使用默认模型" /></label>
        <label>沙箱
          <select id="sandboxInput">
            <option value="workspace-write">workspace-write</option>
            <option value="read-only">read-only</option>
            <option value="danger-full-access">danger-full-access</option>
          </select>
        </label>
        <label>审批策略
          <select id="approvalInput">
            <option value="on-request">on-request</option>
            <option value="untrusted">untrusted</option>
            <option value="never">never</option>
          </select>
        </label>
        <button id="startThreadButton" class="secondary" type="button">启动新 thread</button>
        <div class="ids">
          <span>thread</span><code id="threadId">-</code>
          <span>turn</span><code id="turnId">-</code>
        </div>
      </div>
    </details>
    <details id="connectionSettings">
      <summary>连接设置</summary>
      <label class="token-field">
        <span id="tokenLabel">本机连接（无需 token）</span>
        <input id="tokenInput" type="password" autocomplete="off" placeholder="本机模式无需填写；认证模式再填写" />
      </label>
    </details>
    <span id="sessionMode">已附着当前会话</span>
    <span id="latestSeq">seq -</span>
    <span id="outputHint">等待连接</span>
    <button id="clearOutputButton" type="button">清空对话</button>
    <span id="lastEvent">-</span>
    <details id="requestsPanel"><summary><span>授权与输入</span><span id="requestCount" class="badge warning">0</span></summary><div id="requests" class="requests empty">暂无待处理请求</div></details>
  </section>
</template>
