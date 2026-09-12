(function () {
  "use strict";

  const $ = (id) => document.getElementById(id);
  const embedBridge = window.AetherVscodexEmbed;
  const embeddedInAether = Boolean(embedBridge?.active);
  const i18n = window.VscodexI18n;
  const t = (value) => i18n?.t ? i18n.t(value) : String(value ?? "");
  const uiLocale = () => i18n?.locale?.() || "zh-CN";
  // Translate renderer-owned labels while leaving values supplied by the host
  // (conversation titles, file paths, commands, and message text) untouched.
  const uiText = (zh, en) => uiLocale() === "en-US" ? en : zh;
  const uiWithRaw = (zhPrefix, enPrefix, raw, zhSuffix = "", enSuffix = "") =>
    `${uiText(zhPrefix, enPrefix)}${String(raw ?? "")}${uiText(zhSuffix, enSuffix)}`;
  const appStatusLabel = (value) => {
    const normalized = String(value || "").trim().toLowerCase();
    const labels = {
      ready: "已连接",
      online: "已连接",
      offline: "VS Code 主机未连接",
      waiting_for_host: "等待 VS Code 主机连接",
      app_not_ready: "等待 VS Code 主机连接",
      starting: "正在连接",
      connecting: "正在连接",
      stopped: "已停止",
    };
    return t(labels[normalized] || value || "未知");
  };
  const state = {
    ws: null,
    token: "",
    role: null,
    appReady: false,
    lastSeq: 0,
    // A control snapshot is authoritative for every event up to this
    // sequence. Replayed notifications from the subscribe handshake must not
    // resurrect an already-finished turn or duplicate its transcript.
    lastSnapshotSeq: 0,
    awaitingSnapshot: false,
    threadId: "",
    turnId: "",
    attachMode: false,
    authRequired: null,
    outputSynced: false,
    structuredMessages: [],
    requests: new Map(),
    responding: new Set(),
    commandResults: new Set(),
    reconnectTimer: null,
    embedTicket: "",
    embedWsUrl: "",
    embedDeviceId: "",
    embedTicketRequested: false,
    embedStopped: false,
    activeAssistantBody: null,
    activeAssistantStream: null,
    activeAssistantText: "",
    pendingUserText: "",
    retiredTurnIds: new Set(),
    syncedThreadId: null,
    snapshotNoticeShown: false,
    // Live work items are rendered as updateable transcript entries. The
    // adapter may emit item lifecycle notifications or only output chunks;
    // keeping a small client-side index lets both forms converge on one row.
    activities: new Map(),
    commandDisclosure: new Map(),
    activitySequence: 0,
    activityTimer: null,
    activeAssistantActivityKey: null,
    turnStartedAt: null,
    // The official worked-for row measures from the first work item until
    // the final assistant response starts. Keep this separate from the
    // overall turn clock because the latter also includes queue/approval time.
    turnWorkStartedAt: null,
    finalAssistantStartedAt: null,
    turnStatus: "idle",
    lastTurnDurationMs: null,
    lastWorkedDurationMs: null,
    workedDurationMs: null,
    currentActivity: "idle",
    currentActivityStartedAt: null,
    currentActivityDurationMs: null,
    currentActivityTurnId: "",
    currentModel: "",
    // Empty means the host has not reported an effort yet; null is an
    // authoritative "use the model default" value and must not be serialized
    // as medium on the next turn.
    currentEffort: "",
    sandboxPolicy: "workspace-write",
    approvalPolicy: "on-request",
    tokenUsage: null,
    availableModels: [],
    subagents: [],
    controlMode: "sync",
    modeEpoch: -1,
    capabilities: {
      followsVscodeRoute: true,
      sessionList: false,
      sessionSelect: false,
      sessionCreate: false,
      threadSettings: false,
    },
    modeSnapshotReady: false,
    modeSwitching: false,
    modeCommandId: "",
    requestedControlMode: "",
    modeRequestEpoch: -1,
    sessions: [],
    sessionPickerOpen: false,
    sessionSearch: "",
    sessionFocusedId: "",
    sessionListLoading: false,
    sessionListError: "",
    sessionListCommandId: "",
    sessionSelectCommandId: "",
    newSessionCommandId: "",
    sessionSelectedThreadId: "",
    sessionSwitching: false,
    // Keep the previous view/title visible until the host confirms the target
    // with an authoritative session snapshot. A target can be listed as
    // attachable and still time out during owner hand-off; dropping the old
    // DOM at `session.switching` would leave the browser blank in that case.
    sessionSwitchContext: null,
    modelUpdatePending: false,
    modelAdvancedOpen: false,
    // The official composer keeps the background-agent disclosure closed on
    // first render; the @ hint appears only after the reader expands it.
    subagentsCollapsed: true,
    subagentsExpanded: { active: false, done: false },
    lastRenderedDateKey: "",
    lastRenderedTimestamp: null,
    lastRenderedRole: "",
    hasRenderedUser: false,
    lastDateSeparatorTimestamp: null,
    turnDividers: new Map(),
    // Preserve an explicit worked-for toggle across authoritative snapshot
    // rebuilds. Unset entries follow the official default: the latest turn is
    // open while older completed turns remain compact.
    turnExpansion: new Map(),
    // Legacy attach snapshots may omit turnId on individual projected items.
    // Keep the derived association by object identity for the duration of a
    // snapshot so all grouping/timing paths use the same anonymous turn key.
    structuredTurnKeys: new WeakMap(),
    liveActivityKey: null,
    pendingUserArticle: null,
    outputDistanceFromBottom: 0,
    timelineAnchorLockUntil: 0,
    timelineAnchorCancel: null,
    timelineRevealCancel: null,
  };
  const RESPONDABLE_METHODS = new Set([
    "item/commandExecution/requestApproval",
    "item/fileChange/requestApproval",
    "item/permissions/requestApproval",
    "item/tool/requestUserInput",
    "mcpServer/elicitation/request",
    "applyPatchApproval",
    "execCommandApproval",
  ]);

  const requestKey = (requestId) => `${typeof requestId}:${String(requestId)}`;

  function normalizeControlMode(value) {
    const normalized = String(value || "").trim().toLowerCase();
    return normalized === "sync" || normalized === "async" ? normalized : "";
  }

  function normalizedControlCapabilities(value) {
    const source = isRecord(value) ? value : {};
    return {
      followsVscodeRoute: source.followsVscodeRoute === true,
      sessionList: source.sessionList === true,
      sessionSelect: source.sessionSelect === true,
      sessionCreate: source.sessionCreate === true,
      threadSettings: source.threadSettings === true,
    };
  }

  function sessionControlAllowed(capability) {
    return state.modeSnapshotReady
      && state.controlMode === "async"
      && state.capabilities[capability] === true;
  }

  function threadSettingsAllowed() {
    return state.modeSnapshotReady && state.capabilities.threadSettings === true;
  }

  function controlModeChangeBlocked() {
    return !state.modeSnapshotReady
      || state.modeSwitching
      || state.sessionSwitching
      || !state.appReady
      || !state.ws
      || state.ws.readyState !== WebSocket.OPEN
      || !["operator", "owner", "host"].includes(String(state.role || ""))
      || Boolean(state.turnId)
      || state.turnStartedAt !== null
      || state.turnStatus === "active"
      || state.turnStatus === "waiting"
      || Boolean(state.pendingUserText)
      || state.requests.size > 0
      || state.responding.size > 0
      || Boolean(state.newSessionCommandId)
      || Boolean(state.sessionListCommandId)
      || Boolean(state.sessionSelectCommandId)
      || state.modelUpdatePending;
  }

  function clearControlModeRequest() {
    state.modeSwitching = false;
    state.modeCommandId = "";
    state.requestedControlMode = "";
    state.modeRequestEpoch = -1;
  }

  function renderControlMode() {
    const control = $("controlModeSwitch");
    if (!control) return;
    const listAllowed = sessionControlAllowed("sessionList");
    const createAllowed = sessionControlAllowed("sessionCreate");
    const settingsAllowed = threadSettingsAllowed();
    const blocked = controlModeChangeBlocked();
    control.dataset.mode = state.controlMode;
    control.dataset.epoch = String(state.modeEpoch);
    control.dataset.switching = String(state.modeSwitching);
    control.setAttribute("aria-label", t("控制模式"));
    control.setAttribute("aria-busy", String(state.modeSwitching));
    for (const button of control.querySelectorAll("[data-control-mode]")) {
      const mode = normalizeControlMode(button.dataset.controlMode);
      const current = mode === state.controlMode;
      const pending = state.modeSwitching && mode === state.requestedControlMode;
      button.textContent = t(mode === "async" ? "异步" : "同步");
      button.title = t(mode === "async" ? "异步模式可独立管理会话" : "同步模式跟随 VS Code 当前会话");
      button.setAttribute("aria-pressed", String(current));
      button.dataset.pending = String(pending);
      button.disabled = blocked || current;
    }

    const sessionPickerButton = $("sessionPickerButton");
    if (sessionPickerButton) {
      sessionPickerButton.disabled = !listAllowed || state.modeSwitching || state.sessionSwitching;
      sessionPickerButton.setAttribute("aria-disabled", String(sessionPickerButton.disabled));
    }
    for (const id of ["backButton", "historyButton"]) {
      const button = $(id);
      if (button) button.hidden = !listAllowed;
    }
    const newSessionButton = $("newSessionButton");
    if (newSessionButton) newSessionButton.hidden = !createAllowed;
    const sessionsMenuItem = document.querySelector('[data-menu-action="sessions"]');
    if (sessionsMenuItem) sessionsMenuItem.hidden = !listAllowed;
    const refresh = $("sessionPickerRefresh");
    if (refresh) refresh.disabled = !listAllowed || state.modeSwitching;
    if (!listAllowed && state.sessionPickerOpen) setSessionPicker(false);

    for (const action of document.querySelectorAll('[data-settings-action="model"], [data-settings-action="permission"]')) {
      action.disabled = !settingsAllowed || state.modeSwitching || state.sessionSwitching;
    }
    for (const id of ["modelPickerButton", "permissionChip"]) {
      const button = $(id);
      if (button) button.disabled = !settingsAllowed || state.modeSwitching || state.sessionSwitching;
    }
    if (!settingsAllowed) {
      setModelMenu(false);
      setPermissionMenu(false);
    }
  }

  function applyControlModeSnapshot(metadata) {
    if (!isRecord(metadata)) return false;
    const mode = normalizeControlMode(metadata.controlMode);
    const epoch = finiteNumber(metadata.modeEpoch);
    if (!mode || epoch === null || (state.modeSnapshotReady && epoch < state.modeEpoch)) return false;
    const wasSwitching = state.modeSwitching;
    const requestedMode = state.requestedControlMode;
    const requestEpoch = state.modeRequestEpoch;
    state.controlMode = mode;
    state.modeEpoch = epoch;
    state.capabilities = normalizedControlCapabilities(metadata.capabilities);
    state.modeSnapshotReady = true;
    const requestResolved = wasSwitching && (epoch > requestEpoch || mode === requestedMode);
    if (requestResolved) {
      clearControlModeRequest();
      setConversationStatus(mode === requestedMode ? "控制模式已切换" : "控制模式切换失败", mode === requestedMode ? "ready" : "warning");
    }
    updateIds();
    return true;
  }

  function resolveRequestKey(requestId) {
    const exact = requestKey(requestId);
    if (state.requests.has(exact)) return exact;
    const text = String(requestId);
    const candidates = [...state.requests]
      .filter(([, request]) => String(request.requestId) === text)
      .map(([key]) => key);
    return candidates.length === 1 ? candidates[0] : exact;
  }

  function shouldFollowOutput(output) {
    return output.scrollHeight - output.scrollTop - output.clientHeight <= 24;
  }

  const disclosureAnimations = new WeakMap();
  const activityAnimations = new WeakMap();
  const commandAnimations = new WeakMap();

  function motionDuration(value = 220) {
    try {
      if (window.matchMedia?.("(prefers-reduced-motion: reduce)")?.matches) return 0;
    } catch {
      // Older embedded webviews may not expose matchMedia.
    }
    return Math.max(0, Number(value) || 0);
  }

  function motionClock() {
    return typeof performance === "object" && typeof performance.now === "function"
      ? performance.now()
      : Date.now();
  }

  function scheduleFrame(callback) {
    if (typeof requestAnimationFrame === "function") return requestAnimationFrame(callback);
    return setTimeout(callback, 0);
  }

  function cancelFrame(frame) {
    if (frame === null || frame === undefined) return;
    if (typeof cancelAnimationFrame === "function") cancelAnimationFrame(frame);
    clearTimeout(frame);
  }

  function runMeasuredCssTransition(element, from, to, duration, onFinish) {
    if (!element) return null;
    const priorTransition = element.style.transition;
    let frame = null;
    let timer = null;
    let cancelled = false;
    const setStyles = (values) => {
      for (const [property, value] of Object.entries(values)) element.style[property] = String(value);
    };
    const finish = () => {
      if (cancelled) return;
      cancelled = true;
      cancelFrame(frame);
      frame = null;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      element.style.transition = priorTransition;
      onFinish?.();
    };
    const controller = {
      cancel() {
        if (cancelled) return;
        cancelled = true;
        cancelFrame(frame);
        frame = null;
        if (timer !== null) clearTimeout(timer);
        timer = null;
        element.style.transition = priorTransition;
      },
    };
    const properties = Object.keys(to).map((property) => property.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`));
    element.style.transition = properties
      .map((property) => `${property} ${duration}ms cubic-bezier(.33,1,.68,1)`)
      .join(", ");
    setStyles(from);
    // Force a layout boundary before moving to the measured target. This is
    // the fallback for embedded Chromium builds without Element.animate().
    void element.offsetHeight;
    frame = scheduleFrame(() => {
      frame = null;
      if (cancelled) return;
      setStyles(to);
    });
    timer = setTimeout(finish, duration + 55);
    return controller;
  }

  // Keep the element nearest the reader at the same viewport coordinate while
  // a disclosure or streamed row changes height. This mirrors the official
  // preserve-timeline-anchor-position helper and avoids bottom-relative jumps
  // when the reader is inspecting an earlier turn.
  function preserveTimelineAnchor(anchor, duration = 250) {
    const target = anchor?.nodeType === 1 ? anchor : null;
    const output = target?.closest?.(".chat-scroll") || $("output");
    if (!target || !output || !target.isConnected || !output.isConnected) return;
    if (typeof state.timelineAnchorCancel === "function") state.timelineAnchorCancel();
    const initialTop = target.getBoundingClientRect().top;
    if (!Number.isFinite(initialTop)) return;
    const deadline = motionClock() + duration + 120;
    state.timelineAnchorLockUntil = Math.max(state.timelineAnchorLockUntil || 0, deadline);
    let frame = null;
    let timer = null;
    let disposed = false;
    const adjust = () => {
      if (disposed || !target.isConnected || !output.isConnected) return;
      const delta = target.getBoundingClientRect().top - initialTop;
      if (Number.isFinite(delta) && Math.abs(delta) >= 0.1) {
        const maxScroll = Math.max(0, output.scrollHeight - output.clientHeight);
        output.scrollTop = Math.max(0, Math.min(maxScroll, output.scrollTop + delta));
        updateScrollToBottom(output);
      }
      // A disclosure can change height without producing a ResizeObserver
      // callback on older Chromium builds. Keep one frame queued for the full
      // measured transition so the scrollbar and reader anchor move together.
      if (motionClock() < deadline) schedule();
    };
    const schedule = () => {
      if (disposed || frame !== null) return;
      frame = scheduleFrame(() => {
        frame = null;
        adjust();
      });
    };
    const immediate = () => {
      if (frame !== null) {
        cancelFrame(frame);
        frame = null;
      }
      adjust();
      schedule();
    };
    const observed = target.closest("[data-turn-key]") || target.closest(".message") || target;
    let observer = null;
    if (typeof ResizeObserver === "function") {
      observer = new ResizeObserver(immediate);
      observer.observe(observed);
      if (observed !== target) observer.observe(target);
    }
    schedule();
    const finish = () => {
      if (disposed) return;
      disposed = true;
      cancelFrame(frame);
      frame = null;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      observer?.disconnect();
      if (state.timelineAnchorCancel === finish) state.timelineAnchorCancel = null;
    };
    state.timelineAnchorCancel = finish;
    timer = setTimeout(finish, Math.max(250, duration + 150));
  }

  function setDisclosureBodyState(details, expanded) {
    const body = details?.querySelector?.(":scope > .details-body");
    if (!body) return;
    body.style.display = "block";
    body.style.height = expanded ? "auto" : "0px";
    body.style.opacity = expanded ? "1" : "0";
    body.style.overflow = expanded ? "" : "hidden";
    body.style.pointerEvents = expanded ? "auto" : "none";
    body.dataset.disclosureState = expanded ? "expanded" : "collapsed";
    details.dataset.expanded = String(Boolean(expanded));
    details.querySelector("summary")?.setAttribute("aria-expanded", String(Boolean(expanded)));
  }

  function animateDisclosureBody(details, expanded, options = {}) {
    const body = details?.querySelector?.(":scope > .details-body");
    if (!body) return;
    const previous = disclosureAnimations.get(body);
    if (previous) {
      try { previous.commitStyles?.(); } catch { /* animation may already be finished */ }
      previous.cancel();
    }
    disclosureAnimations.delete(body);
    const next = Boolean(expanded);
    details.dataset.expanded = String(next);
    details.dataset.animating = "true";
    body.style.display = "block";
    const duration = motionDuration(options.duration ?? 220);
    const computed = getComputedStyle(body);
    const currentHeight = Math.max(0, body.getBoundingClientRect().height || 0);
    const currentOpacity = Number.parseFloat(computed.opacity);
    const fromOpacity = Number.isFinite(currentOpacity) ? currentOpacity : (next ? 0 : 1);
    if (options.immediate || duration === 0) {
      setDisclosureBodyState(details, next);
      details.dataset.animating = "false";
      return;
    }
    let targetHeight = 0;
    if (next) {
      body.style.height = "auto";
      body.style.opacity = "1";
      body.style.overflow = "hidden";
      targetHeight = Math.max(0, body.getBoundingClientRect().height || body.scrollHeight || 0);
      body.style.height = `${currentHeight}px`;
    } else {
      body.style.height = `${currentHeight}px`;
      body.style.opacity = String(fromOpacity);
      body.style.overflow = "hidden";
    }
    if (Math.abs(targetHeight - currentHeight) < 0.5 && (next ? fromOpacity >= 0.99 : fromOpacity <= 0.01)) {
      setDisclosureBodyState(details, next);
      details.dataset.animating = "false";
      return;
    }
    if (typeof body.animate !== "function") {
      let transition;
      transition = runMeasuredCssTransition(
        body,
        { height: `${currentHeight}px`, opacity: fromOpacity },
        { height: `${targetHeight}px`, opacity: next ? 1 : 0 },
        duration,
        () => {
          if (disclosureAnimations.get(body) !== transition) return;
          disclosureAnimations.delete(body);
          setDisclosureBodyState(details, next);
          details.dataset.animating = "false";
        },
      );
      if (transition) disclosureAnimations.set(body, transition);
      return;
    }
    const animation = body.animate([
      { height: `${currentHeight}px`, opacity: fromOpacity },
      { height: `${targetHeight}px`, opacity: next ? 1 : 0 },
    ], { duration, easing: "cubic-bezier(.33,1,.68,1)", fill: "forwards" });
    disclosureAnimations.set(body, animation);
    animation.addEventListener("finish", () => {
      if (disclosureAnimations.get(body) !== animation) return;
      disclosureAnimations.delete(body);
      setDisclosureBodyState(details, next);
      details.dataset.animating = "false";
      // `fill: forwards` keeps the animation in the cascade after finish.
      // Release it only after the stable inline state is written, otherwise a
      // later open can still measure the previous collapsed height (zero).
      animation.cancel();
    }, { once: true });
    animation.addEventListener("cancel", () => {
      if (disclosureAnimations.get(body) === animation) disclosureAnimations.delete(body);
    }, { once: true });
  }

  function installDisclosure(details, initiallyOpen) {
    if (!details || details.dataset.disclosureInstalled === "true") return;
    const summary = details.querySelector(":scope > summary");
    const body = details.querySelector(":scope > .details-body");
    if (!summary || !body) return;
    details.dataset.disclosureInstalled = "true";
    // Keep the native details element mounted. The summary still supplies the
    // familiar keyboard/focus semantics, while the body itself is animated
    // with measured height so a close does not jump the transcript.
    details.open = true;
    setDisclosureBodyState(details, Boolean(initiallyOpen));
    summary.addEventListener("click", (event) => {
      event.preventDefault();
      setDetailsExpanded(details, !isDetailsExpanded(details));
    }, true);
    details.addEventListener("toggle", () => {
      // A browser/plugin script may still assign `.open = false`; restore the
      // mounted shell and leave the visual state under our measured body.
      if (!details.open && !details.__codexRestoring) {
        details.__codexRestoring = true;
        details.open = true;
        details.__codexRestoring = false;
      }
    });
  }

  function isDetailsExpanded(details) {
    if (!details) return false;
    if (details.dataset.expanded !== undefined) return details.dataset.expanded === "true";
    return details.open === true;
  }

  function setDetailsExpanded(details, expanded, options = {}) {
    if (!details) return;
    const next = Boolean(expanded);
    const previous = isDetailsExpanded(details);
    const installed = details.dataset.disclosureInstalled === "true";
    if (!installed) {
      details.open = next;
      return;
    }
    if (previous === next && !options.force) {
      if (options.immediate) animateDisclosureBody(details, next, { immediate: true });
      return;
    }
    details.dataset.expanded = String(next);
    if (options.preserve !== false && !options.immediate) preserveTimelineAnchor(details, options.duration ?? 230);
    animateDisclosureBody(details, next, { immediate: Boolean(options.immediate), duration: options.duration });
    if (next && !options.immediate && options.reveal !== false) {
      scheduleTimelineReveal(details.querySelector(":scope > .details-body") || details, options.duration ?? 220);
    }
  }

  function animateActivityArticle(article, expanded, options = {}) {
    if (!article) return;
    const previous = activityAnimations.get(article);
    if (previous) {
      try { previous.commitStyles?.(); } catch { /* animation may already be finished */ }
      previous.cancel();
    }
    activityAnimations.delete(article);
    const next = Boolean(expanded);
    const duration = motionDuration(options.duration ?? 230);
    const wasCollapsed = article.classList.contains("turn-collapsed");
    article.dataset.turnExpanded = String(next);

    // `worked-for` is an outer disclosure. The official renderer removes the
    // whole activity group from layout while it is collapsed, rather than
    // leaving one summary row per command. Animate the article shell itself;
    // nested command/read disclosures keep their own expanded state.
    if (next) article.classList.remove("turn-collapsed");
    article.style.display = "";
    article.style.height = "";
    article.style.opacity = "";
    article.style.visibility = "";
    article.style.pointerEvents = "";
    article.style.overflow = "";
    const currentHeight = Math.max(0, article.getBoundingClientRect().height || 0);
    const naturalHeight = Math.max(0, article.scrollHeight || currentHeight);
    const fromHeight = next && wasCollapsed ? 0 : currentHeight;
    const targetHeight = next ? naturalHeight : 0;
    const fromOpacity = next ? (wasCollapsed ? 0 : 1) : 1;
    const targetOpacity = next ? 1 : 0;
    const finish = () => {
      if (next) {
        article.classList.remove("turn-collapsed");
        article.style.height = "";
        article.style.opacity = "";
        article.style.visibility = "";
        article.style.pointerEvents = "";
        article.style.overflow = "";
      } else {
        article.classList.add("turn-collapsed");
        article.style.height = "0px";
        article.style.opacity = "0";
        article.style.visibility = "hidden";
        article.style.pointerEvents = "none";
        article.style.overflow = "hidden";
      }
    };
    if (options.immediate || duration === 0 || Math.abs(targetHeight - fromHeight) < 0.5) {
      finish();
      return;
    }
    article.style.height = `${fromHeight}px`;
    article.style.opacity = String(fromOpacity);
    article.style.overflow = "hidden";
    if (typeof article.animate !== "function") {
      let transition;
      transition = runMeasuredCssTransition(
        article,
        { height: `${fromHeight}px`, opacity: fromOpacity },
        { height: `${targetHeight}px`, opacity: targetOpacity },
        duration,
        () => {
          if (activityAnimations.get(article) !== transition) return;
          activityAnimations.delete(article);
          finish();
        },
      );
      if (transition) activityAnimations.set(article, transition);
      return;
    }
    const animation = article.animate([
      { height: `${fromHeight}px`, opacity: fromOpacity },
      { height: `${targetHeight}px`, opacity: targetOpacity },
    ], { duration, easing: "cubic-bezier(.33,1,.68,1)", fill: "forwards" });
    activityAnimations.set(article, animation);
    animation.addEventListener("finish", () => {
      if (activityAnimations.get(article) !== animation) return;
      activityAnimations.delete(article);
      finish();
      animation.cancel();
    }, { once: true });
    animation.addEventListener("cancel", () => {
      if (activityAnimations.get(article) === animation) activityAnimations.delete(article);
    }, { once: true });
  }

  function animateCommandRow(commandRow, expanded, options = {}) {
    if (!commandRow) return;
    const previous = commandAnimations.get(commandRow);
    if (previous) {
      try { previous.commitStyles?.(); } catch { /* animation may already be finished */ }
      previous.cancel();
    }
    commandAnimations.delete(commandRow);
    const next = Boolean(expanded);
    const duration = motionDuration(options.duration ?? 190);
    const fromHeight = Math.max(0, commandRow.getBoundingClientRect().height || 0);
    commandRow.dataset.expanded = String(next);
    commandRow.setAttribute("aria-expanded", String(next));
    commandRow.style.overflow = "hidden";
    commandRow.style.height = "auto";
    const targetHeight = Math.max(0, commandRow.getBoundingClientRect().height || 0);
    if (options.immediate || duration === 0 || Math.abs(targetHeight - fromHeight) < 0.5) {
      commandRow.style.height = "";
      commandRow.style.overflow = "";
      return;
    }
    if (typeof commandRow.animate !== "function") {
      let transition;
      transition = runMeasuredCssTransition(
        commandRow,
        { height: `${fromHeight}px` },
        { height: `${targetHeight}px` },
        duration,
        () => {
          if (commandAnimations.get(commandRow) !== transition) return;
          commandAnimations.delete(commandRow);
          commandRow.style.height = "";
          commandRow.style.overflow = "";
        },
      );
      if (transition) commandAnimations.set(commandRow, transition);
      return;
    }
    commandRow.style.height = `${fromHeight}px`;
    const animation = commandRow.animate([
      { height: `${fromHeight}px` },
      { height: `${targetHeight}px` },
    ], { duration, easing: "cubic-bezier(.33,1,.68,1)", fill: "forwards" });
    commandAnimations.set(commandRow, animation);
    animation.addEventListener("finish", () => {
      if (commandAnimations.get(commandRow) !== animation) return;
      commandAnimations.delete(commandRow);
      commandRow.style.height = "";
      commandRow.style.overflow = "";
      animation.cancel();
    }, { once: true });
    animation.addEventListener("cancel", () => {
      if (commandAnimations.get(commandRow) === animation) commandAnimations.delete(commandRow);
    }, { once: true });
  }

  function updateScrollToBottom(output = $("output")) {
    const button = $("scrollToBottom");
    if (!button || !output) return;
    const distance = Math.max(0, output.scrollHeight - output.scrollTop - output.clientHeight);
    state.outputDistanceFromBottom = distance;
    const visible = distance > 24;
    const working = state.turnStartedAt !== null || state.turnStatus === "active" || state.turnStatus === "waiting";
    button.dataset.visible = String(visible);
    button.dataset.working = String(working);
    button.setAttribute("aria-label", t(working ? "正在工作，回到最新消息" : "回到最新消息"));
    button.setAttribute("aria-hidden", String(!visible));
    button.tabIndex = visible ? 0 : -1;
  }

  function scrollOutput(output, force = false) {
    if (!output) return;
    if (force || shouldFollowOutput(output)) {
      if (typeof output.scrollTo === "function") output.scrollTo({ top: output.scrollHeight, behavior: "auto" });
      else output.scrollTop = output.scrollHeight;
    }
    updateScrollToBottom(output);
  }

  function updateScrollPadding() {
    const output = $("output");
    const panel = document.querySelector(".chat-panel");
    const composer = $("messageForm");
    if (!output || !panel || !composer) return;
    const outputRect = output.getBoundingClientRect();
    const composerRect = composer.getBoundingClientRect();
    // The composer is an overlay in the official panel. Reserve only the
    // portion that actually covers the scroll viewport, plus a small gap.
    const overlap = Math.max(0, Math.ceil(outputRect.bottom - composerRect.top));
    const reserve = Math.max(72, overlap + 16);
    output.style.setProperty("--thread-scroll-padding-bottom", `${reserve}px`);
    panel.style.setProperty("--thread-scroll-padding-bottom", `${reserve}px`);
    updateScrollToBottom(output);
  }

  function timelineVisibleBounds(output) {
    if (!output) return null;
    const outputRect = output.getBoundingClientRect();
    const composer = $("messageForm");
    const composerRect = composer?.getBoundingClientRect?.();
    const top = outputRect.top + 8;
    const composerTop = composerRect && Number.isFinite(composerRect.top) ? composerRect.top - 10 : outputRect.bottom - 8;
    const bottom = Math.min(outputRect.bottom - 8, composerTop);
    return { top, bottom: Math.max(top, bottom) };
  }

  // Keep an expanding activity row inside the portion of the transcript that
  // is actually readable above the composer. This runs across the measured
  // height animation because one layout pass is not enough when streamed
  // command output arrives at the same time.
  function ensureTimelineVisible(target) {
    const element = target?.nodeType === 1 ? target : null;
    const output = element?.closest?.(".chat-scroll") || $("output");
    if (!element || !output || !element.isConnected || !output.isConnected) return;
    const bounds = timelineVisibleBounds(output);
    if (!bounds) return;
    const rect = element.getBoundingClientRect();
    let delta = 0;
    if (rect.top < bounds.top) delta = rect.top - bounds.top;
    else if (rect.bottom > bounds.bottom) delta = rect.bottom - bounds.bottom;
    if (!Number.isFinite(delta) || Math.abs(delta) < 0.25) return;
    const maxScroll = Math.max(0, output.scrollHeight - output.clientHeight);
    output.scrollTop = Math.max(0, Math.min(maxScroll, output.scrollTop + delta));
    updateScrollToBottom(output);
  }

  function scheduleTimelineReveal(target, duration = 220) {
    const element = target?.nodeType === 1 ? target : null;
    if (!element) return;
    if (typeof state.timelineRevealCancel === "function") state.timelineRevealCancel();
    const deadline = motionClock() + motionDuration(duration) + 90;
    let frame = null;
    let timer = null;
    let disposed = false;
    const tick = () => {
      if (disposed) return;
      frame = null;
      ensureTimelineVisible(element);
      if (motionClock() < deadline) frame = scheduleFrame(tick);
    };
    const cancel = () => {
      if (disposed) return;
      disposed = true;
      cancelFrame(frame);
      frame = null;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      if (state.timelineRevealCancel === cancel) state.timelineRevealCancel = null;
    };
    state.timelineRevealCancel = cancel;
    tick();
    timer = setTimeout(cancel, Math.max(180, motionDuration(duration) + 130));
  }

  function comparableText(value) {
    return String(value ?? "").replace(/\s+/g, " ").trim();
  }

  function hasRenderedMessage(text, turnId, role) {
    const target = comparableText(text);
    if (!target) return false;
    const output = $("output");
    if (!output) return false;
    return [...output.querySelectorAll(`.message.${role}`)].some((article) => {
      if (turnId && article.dataset.turnId && article.dataset.turnId !== String(turnId)) return false;
      return comparableText(article.dataset.rawText) === target;
    });
  }

  function hasRenderedCompletedMessage(text, turnId, role) {
    const target = comparableText(text);
    if (!target) return false;
    const output = $("output");
    if (!output) return false;
    return [...output.querySelectorAll(`.message.${role}`)].some((article) => {
      if (article.classList.contains("streaming")) return false;
      if (turnId && article.dataset.turnId && article.dataset.turnId !== String(turnId)) return false;
      const raw = comparableText(article.dataset.rawText);
      return raw === target || raw.includes(target) || target.includes(raw);
    });
  }

  function appendInlineMarkdown(parent, source) {
    const pattern = /(\[[^\]]+\]\(https?:\/\/[^)\s]+\)|`[^`\n]+`|\*\*[^*\n]+\*\*|__[^_\n]+__|~~[^~\n]+~~|\*[^*\n]+\*|_[^_\n]+_)/g;
    let cursor = 0;
    const appendText = (value) => {
      const parts = String(value).split("\n");
      parts.forEach((part, index) => {
        if (part) parent.append(document.createTextNode(part));
        if (index < parts.length - 1) parent.append(document.createElement("br"));
      });
    };
    for (const match of String(source).matchAll(pattern)) {
      if (match.index > cursor) appendText(String(source).slice(cursor, match.index));
      const token = match[0];
      if (token.startsWith("[") && token.endsWith(")")) {
        const split = token.match(/^\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)$/);
        if (split) {
          const link = document.createElement("a");
          link.href = split[2];
          link.target = "_blank";
          link.rel = "noopener noreferrer";
          link.textContent = split[1];
          parent.append(link);
        } else appendText(token);
      } else if (token.startsWith("`") && token.endsWith("`")) {
        const code = document.createElement("code");
        code.textContent = token.slice(1, -1);
        parent.append(code);
      } else if (token.startsWith("**") || token.startsWith("__")) {
        const strong = document.createElement("strong");
        strong.textContent = token.slice(2, -2);
        parent.append(strong);
      } else if (token.startsWith("~~")) {
        const deleted = document.createElement("del");
        deleted.textContent = token.slice(2, -2);
        parent.append(deleted);
      } else if (token.startsWith("*") || token.startsWith("_")) {
        const emphasis = document.createElement("em");
        emphasis.textContent = token.slice(1, -1);
        parent.append(emphasis);
      } else appendText(token);
      cursor = match.index + token.length;
    }
    if (cursor < String(source).length) appendText(String(source).slice(cursor));
  }

  function tableCells(line) {
    let value = String(line || "").trim();
    if (value.startsWith("|")) value = value.slice(1);
    if (value.endsWith("|")) value = value.slice(0, -1);
    return value.split("|").map((cell) => cell.trim());
  }

  function isTableDivider(line) {
    const cells = tableCells(line);
    return cells.length > 0 && cells.every((cell) => /^:?-{3,}:?$/.test(cell));
  }

  function appendTable(container, headerLine, bodyLines) {
    const table = document.createElement("table");
    const thead = document.createElement("thead");
    const header = document.createElement("tr");
    for (const cell of tableCells(headerLine)) {
      const th = document.createElement("th");
      appendInlineMarkdown(th, cell);
      header.append(th);
    }
    thead.append(header);
    table.append(thead);
    const tbody = document.createElement("tbody");
    for (const line of bodyLines) {
      const row = document.createElement("tr");
      for (const cell of tableCells(line)) {
        const td = document.createElement("td");
        appendInlineMarkdown(td, cell);
        row.append(td);
      }
      tbody.append(row);
    }
    table.append(tbody);
    container.append(table);
  }

  /**
   * Render the small, safe Markdown subset used by Codex messages. The
   * official webview uses a full Markdown/ProseMirror pipeline; the relay
   * intentionally keeps this browser-side renderer dependency-free and never
   * assigns untrusted text to innerHTML.
   */
  function renderMarkdown(container, source) {
    container.replaceChildren();
    const lines = String(source ?? "").replace(/\r\n?/g, "\n").split("\n");
    let index = 0;
    const addParagraph = (paragraph) => {
      if (!paragraph.length) return;
      const element = document.createElement("p");
      appendInlineMarkdown(element, paragraph.join("\n"));
      container.append(element);
    };
    while (index < lines.length) {
      const line = lines[index];
      if (!line.trim()) { index += 1; continue; }
      const fence = line.match(/^\s*```\s*([\w.+-]*)\s*$/);
      if (fence) {
        index += 1;
        const codeLines = [];
        while (index < lines.length && !/^\s*```\s*$/.test(lines[index])) codeLines.push(lines[index++]);
        if (index < lines.length) index += 1;
        const pre = document.createElement("pre");
        const code = document.createElement("code");
        if (fence[1]) code.dataset.language = fence[1];
        code.textContent = codeLines.join("\n");
        pre.append(code);
        container.append(pre);
        continue;
      }
      if (index + 1 < lines.length && line.includes("|") && isTableDivider(lines[index + 1])) {
        const body = [];
        index += 2;
        while (index < lines.length && lines[index].trim() && lines[index].includes("|")) body.push(lines[index++]);
        appendTable(container, line, body);
        continue;
      }
      if (/^\s*(?:---+|___+|\*\*\*+)\s*$/.test(line)) {
        container.append(document.createElement("hr"));
        index += 1;
        continue;
      }
      const heading = line.match(/^\s*(#{1,3})\s+(.+?)\s*#*$/);
      if (heading) {
        const element = document.createElement(`h${heading[1].length}`);
        appendInlineMarkdown(element, heading[2]);
        container.append(element);
        index += 1;
        continue;
      }
      if (/^\s*>\s?/.test(line)) {
        const quote = document.createElement("blockquote");
        while (index < lines.length && /^\s*>\s?/.test(lines[index])) {
          const paragraph = document.createElement("p");
          appendInlineMarkdown(paragraph, lines[index].replace(/^\s*>\s?/, ""));
          quote.append(paragraph);
          index += 1;
        }
        container.append(quote);
        continue;
      }
      const list = line.match(/^\s*([-*+]|\d+[.)])\s+(.+)$/);
      if (list) {
        const ordered = /^\d/.test(list[1]);
        const listElement = document.createElement(ordered ? "ol" : "ul");
        while (index < lines.length) {
          const item = lines[index].match(/^\s*([-*+]|\d+[.)])\s+(.+)$/);
          if (!item || /^\d/.test(item[1]) !== ordered) break;
          const li = document.createElement("li");
          const task = item[2].match(/^\[([ xX])\]\s+(.+)$/);
          if (task) {
            li.className = "task-list-item";
            const checkbox = document.createElement("input");
            checkbox.type = "checkbox";
            checkbox.checked = task[1].toLowerCase() === "x";
            checkbox.disabled = true;
            checkbox.setAttribute("aria-label", t(checkbox.checked ? "已完成" : "未完成"));
            li.append(checkbox);
            appendInlineMarkdown(li, task[2]);
          } else appendInlineMarkdown(li, item[2]);
          listElement.append(li);
          index += 1;
        }
        container.append(listElement);
        continue;
      }
      const paragraph = [line];
      index += 1;
      while (index < lines.length
        && lines[index].trim()
        && !/^\s*```/.test(lines[index])
        && !/^\s*(#{1,3})\s+/.test(lines[index])
        && !/^\s*>\s?/.test(lines[index])
        && !/^\s*([-*+]|\d+[.)])\s+/.test(lines[index])) {
        paragraph.push(lines[index++]);
      }
      addParagraph(paragraph);
    }
  }

  function renderMessageBody(body, text, role, tone, kind) {
    const value = String(text ?? "");
    body.dataset.rawText = value;
    // User-authored prompts use the same safe Markdown subset as assistant
    // messages. Tool/status rows stay literal so command output cannot be
    // mistaken for formatted content.
    const markdownRole = role === "assistant" || role === "user"
      || kind === "reasoning" || kind === "plan" || kind === "subagent" || kind === "commentary";
    body.classList.toggle("markdown-body", markdownRole && tone !== "meta" && tone !== "error" && kind !== "tool");
    body.classList.toggle("diff-body", kind === "edit");
    if (kind === "edit") {
      body.replaceChildren();
      const output = document.createElement("pre");
      output.className = "diff-output";
      for (const line of value.replace(/\r\n?/g, "\n").split("\n")) {
        const row = document.createElement("span");
        row.className = line.startsWith("+") && !line.startsWith("+++")
          ? "diff-line added"
          : line.startsWith("-") && !line.startsWith("---")
            ? "diff-line removed"
            : line.startsWith("@@") || line.startsWith("diff ") || line.startsWith("[")
              ? "diff-line context"
              : "diff-line";
        row.textContent = line || " ";
        output.append(row);
      }
      body.append(output);
    } else if (kind === "tool" || kind === "read") body.textContent = value;
    else if (markdownRole && tone !== "meta" && tone !== "error") renderMarkdown(body, value);
    else body.textContent = value;
  }

  function createTerminalIcon() {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.className.baseVal = "activity-summary-icon terminal-icon";
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("aria-hidden", "true");
    const frame = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    frame.setAttribute("x", "2.25");
    frame.setAttribute("y", "2.75");
    frame.setAttribute("width", "11.5");
    frame.setAttribute("height", "10.5");
    frame.setAttribute("rx", "1.4");
    const prompt = document.createElementNS("http://www.w3.org/2000/svg", "path");
    prompt.setAttribute("d", "m4.5 6 2 2-2 2");
    const cursor = document.createElementNS("http://www.w3.org/2000/svg", "path");
    cursor.setAttribute("d", "M8.5 10h2.5");
    svg.append(frame, prompt, cursor);
    return svg;
  }

  function createSubagentIcon() {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.className.baseVal = "activity-summary-icon subagent-summary-icon";
    svg.setAttribute("viewBox", "0 0 24 24");
    svg.setAttribute("aria-hidden", "true");
    // The installed Codex webview uses the filled blossom mark for both
    // sub-agent activity rows and the composer agent rail. Keep the path
    // inline so the standalone relay page does not depend on VS Code's URI
    // resolver or on an extension-owned asset path.
    const mark = document.createElementNS("http://www.w3.org/2000/svg", "path");
    mark.setAttribute("fill", "currentColor");
    mark.setAttribute("d", "M13.795 23.856q-1.188 0-2.256-.448a6.1 6.1 0 0 1-1.9-1.247 5.8 5.8 0 0 1-1.875.306 5.8 5.8 0 0 1-2.944-.777 6.1 6.1 0 0 1-2.184-2.12q-.807-1.34-.808-2.99 0-.682.19-1.482a6.3 6.3 0 0 1-1.472-2.002 5.76 5.76 0 0 1 .024-4.85q.546-1.177 1.52-2.024a5.5 5.5 0 0 1 2.303-1.2A5.55 5.55 0 0 1 5.485 2.62 6.06 6.06 0 0 1 7.575.925 5.85 5.85 0 0 1 10.21.313q1.187 0 2.255.447a6.1 6.1 0 0 1 1.9 1.248 5.8 5.8 0 0 1 1.875-.306q1.59 0 2.944.776a5.9 5.9 0 0 1 2.16 2.12q.832 1.34.832 2.99 0 .682-.19 1.483a6.2 6.2 0 0 1 1.472 2.024q.522 1.13.522 2.378 0 1.272-.546 2.449a6.1 6.1 0 0 1-1.543 2.048 5.45 5.45 0 0 1-2.28 1.177 5.4 5.4 0 0 1-1.115 2.402 5.8 5.8 0 0 1-2.066 1.695 5.85 5.85 0 0 1-2.635.612M7.93 20.913q1.188 0 2.066-.495l4.463-2.542a.52.52 0 0 0 .238-.448v-2.024L8.95 18.676a.97.97 0 0 1-1.044 0L3.419 16.11a.7.7 0 0 1-.024.165v.282q0 1.201.57 2.213.594.99 1.639 1.554 1.044.59 2.326.589m.238-3.838q.143.07.26.07a.46.46 0 0 0 .238-.07l1.781-1.012-5.722-3.296q-.522-.306-.522-.918v-5.11a4.27 4.27 0 0 0-1.9 1.602 4.13 4.13 0 0 0-.712 2.354q0 1.155.594 2.213.593 1.06 1.543 1.601zm5.627 5.227q1.258 0 2.279-.565a4.25 4.25 0 0 0 1.614-1.554q.594-.99.594-2.213v-5.085q0-.283-.237-.424l-1.805-1.036v6.568q0 .613-.522.919l-4.487 2.566q1.163.825 2.564.824m.902-8.617v-3.202l-2.683-1.507-2.707 1.507v3.202l2.707 1.507zm-6.933-7.51q0-.612.522-.918l4.488-2.567a4.34 4.34 0 0 0-2.564-.824q-1.26 0-2.28.565a4.25 4.25 0 0 0-1.614 1.554q-.57.99-.57 2.213v5.062q0 .283.237.447l1.781 1.036zm12.061 11.253a4.13 4.13 0 0 0 1.876-1.6 4.2 4.2 0 0 0 .712-2.355q0-1.154-.593-2.213-.594-1.06-1.544-1.6l-4.44-2.543q-.142-.095-.26-.071a.46.46 0 0 0-.238.07l-1.78.99 5.745 3.319q.26.141.38.377a.9.9 0 0 1 .142.518zm-4.772-11.96q.522-.33 1.045 0l4.51 2.614v-.424q0-1.13-.57-2.142a4.1 4.1 0 0 0-1.59-1.648q-1.02-.613-2.374-.613-1.187 0-2.066.495L9.545 6.292a.52.52 0 0 0-.238.448v2.025z");
    svg.append(mark);
    return svg;
  }

  function createReadIcon() {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.className.baseVal = "activity-summary-icon read-summary-icon";
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("aria-hidden", "true");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", "M2.5 4.5h4l1.2 1.4h5.8v6.2c0 .8-.5 1.4-1.3 1.4H3.8c-.8 0-1.3-.6-1.3-1.4V4.5Z");
    const top = document.createElementNS("http://www.w3.org/2000/svg", "path");
    top.setAttribute("d", "M2.5 6h11");
    svg.append(path, top);
    return svg;
  }

  // The official sub-agent activity renderer keeps the agent name in a
  // bordered chip and places the lifecycle text beside it. Keeping those two
  // nodes separate also lets a running row update only its status without
  // replacing the blossom icon or the accessible label.
  function setSubagentSummary(summary, name, statusText = "") {
    if (!summary) return;
    let chip = summary.querySelector(".subagent-summary-chip");
    let label = chip?.querySelector(".subagent-summary-label");
    let status = summary.querySelector(".subagent-summary-status");
    if (!chip || !label || !status) {
      chip = document.createElement("span");
      chip.className = "subagent-summary-chip";
      const icon = createSubagentIcon();
      label = document.createElement("span");
      label.className = "subagent-summary-label";
      chip.append(icon, label);
      status = document.createElement("span");
      status.className = "subagent-summary-status";
      summary.replaceChildren(chip, status);
    }
    label.textContent = String(name || t("子代理"));
    status.textContent = statusText ? t(statusText) : "";
    summary.setAttribute("aria-label", [label.textContent, status.textContent].filter(Boolean).join(" "));
  }

  function setActivitySummary(summary, text, kind = "") {
    if (!summary) return;
    if (kind === "subagent") {
      const value = String(text || "");
      const match = value.match(/^(.*?)(?:\s+(已开始工作|已完成|失败|已中断|等待中|处理中|Started working|Completed|Failed|Interrupted|Waiting|Working))$/);
      setSubagentSummary(summary, match ? match[1] : value || t("子代理"), match ? t(match[2]) : "");
      return;
    }
    if (kind !== "tool" && kind !== "read" && kind !== "subagent") {
      summary.textContent = String(text || "");
      return;
    }
    let icon = summary.querySelector(".activity-summary-icon");
    let label = summary.querySelector(".activity-summary-label");
    if (!icon || !label) {
      icon = kind === "subagent" ? createSubagentIcon() : kind === "read" ? createReadIcon() : createTerminalIcon();
      label = document.createElement("span");
      label.className = "activity-summary-label";
      summary.replaceChildren(icon, label);
    }
    label.textContent = String(text || "");
  }

  function stripShellQuotes(value) {
    let text = String(value || "").trim();
    let changed = true;
    while (changed) {
      changed = false;
      if (text.startsWith("$'") && text.endsWith("'")) {
        text = text.slice(2, -1).replace(/\\'/g, "'");
        changed = true;
      } else if ((text.startsWith("'") && text.endsWith("'"))
        || (text.startsWith('"') && text.endsWith('"'))) {
        const quoted = text.startsWith('"');
        text = text.slice(1, -1);
        if (quoted) text = text.replace(/\\"/g, '"');
        changed = true;
      }
    }
    return text.trim();
  }

  // The official command renderer hides the shell bootstrap used by the IPC
  // runner (`/bin/zsh -lc '…'`) and shows the actual user command instead.
  function terminalCommandText(value) {
    const command = stripShellQuotes(value);
    const match = command.match(/^(?:.*[/\\])?(?:bash|cmd(?:\.exe)?|fish|powershell(?:\.exe)?|pwsh(?:\.exe)?|sh|zsh)\s+-lc\s+([\s\S]+)$/i);
    return match ? stripShellQuotes(match[1]) : command;
  }

  function addMessageActions(article, enabled = true) {
    // Tool/reasoning rows have their own disclosure controls and should not
    // grow a second action rail. Editing an earlier turn is intentionally not
    // exposed until the relay can perform the official branch/edit operation.
    if (!enabled || (!article.classList.contains("user") && !article.classList.contains("assistant"))) return null;
    if (article.classList.contains("assistant") && article.classList.contains("streaming")) return null;
    const actions = document.createElement("div");
    actions.className = "message-actions";
    const copy = document.createElement("button");
    copy.type = "button";
    copy.className = "message-action";
    copy.title = t("复制消息");
    copy.setAttribute("aria-label", t("复制消息"));
    const setCopyIcon = (copied = false) => {
      copy.replaceChildren();
      const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      svg.setAttribute("viewBox", "0 0 16 16");
      svg.setAttribute("aria-hidden", "true");
      if (copied) {
        const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
        path.setAttribute("d", "m3.5 8.2 2.7 2.7 6.3-6.3");
        svg.append(path);
      } else {
        const back = document.createElementNS("http://www.w3.org/2000/svg", "path");
        back.setAttribute("d", "M5.5 5.5V4c0-.8.7-1.5 1.5-1.5h5c.8 0 1.5.7 1.5 1.5v5c0 .8-.7 1.5-1.5 1.5h-1.5");
        const front = document.createElementNS("http://www.w3.org/2000/svg", "rect");
        front.setAttribute("x", "2.5");
        front.setAttribute("y", "5.5");
        front.setAttribute("width", "7.5");
        front.setAttribute("height", "7.5");
        front.setAttribute("rx", "1.2");
        svg.append(back, front);
      }
      copy.append(svg);
    };
    setCopyIcon();
    copy.addEventListener("click", async () => {
      const value = article.dataset.rawText || "";
      try {
        await navigator.clipboard?.writeText(value);
        setCopyIcon(true);
        setTimeout(() => { setCopyIcon(false); }, 1_200);
      } catch {
        copy.textContent = "!";
      }
    });
    actions.append(copy);
    article.append(actions);
    return actions;
  }

  function appendMessage(text, role = "assistant", tone = "text", meta = "", options = {}) {
    if (text === undefined || text === null) return null;
    const output = $("output");
    const follow = shouldFollowOutput(output);
    const article = document.createElement("article");
    article.className = `message ${role}${tone && tone !== "text" ? ` ${tone}` : ""}`;
    if (options.kind) article.dataset.kind = options.kind;
    if (options.status) article.dataset.status = String(options.status);
    if (options.turnId) article.dataset.turnId = String(options.turnId);
    if (options.structuredKey) article.dataset.structuredKey = String(options.structuredKey);
    if (options.activityKey) article.dataset.activityKey = String(options.activityKey);
    if (options.itemId !== undefined && options.itemId !== null) article.dataset.itemId = String(options.itemId);
    if (options.itemType) article.dataset.itemType = String(options.itemType);
    if (options.agentThreadId) article.dataset.agentThreadId = String(options.agentThreadId);
    if (options.timestamp) {
      const parsedTimestamp = timestampMs(options.timestamp);
      if (parsedTimestamp !== null) article.dataset.timestamp = String(parsedTimestamp);
    }
    article.dataset.rawText = String(text);
    const content = document.createElement("div");
    content.className = "message-content";
    let body = document.createElement("div");
    body.className = "message-body";
    let details = null;
    const initiallyOpen = options.open !== false;
    if (options.collapsible) {
      details = document.createElement("details");
      details.className = `message-details ${options.kind || ""}`;
      const summary = document.createElement("summary");
      const summaryText = options.summary || options.label || t("详情");
      if (options.kind === "tool" || options.kind === "read" || options.kind === "subagent") {
        setActivitySummary(summary, summaryText, options.kind);
        if (options.command) summary.title = terminalCommandText(options.command);
      } else {
        summary.textContent = summaryText;
        if (options.command && !options.summary) {
          // Translate only the renderer-owned label. The command is host data
          // and must remain byte-for-byte unchanged in every locale.
          summary.textContent = `${t(options.label || "命令")} · ${String(options.command)}`;
          summary.title = String(options.command);
        }
      }
      body.classList.add("details-body");
      details.append(summary, body);
      content.append(details);
    } else content.append(body);
    renderMessageBody(body, text, role, tone, options.kind);
    article.append(content);
    const actions = addMessageActions(article, options.showActions !== false);
    const metaParts = [];
    if (meta) metaParts.push(String(meta));
    if (options.showTimestamp === true && options.timestamp) {
      const timestamp = formatMessageTime(options.timestamp);
      if (timestamp) metaParts.push(timestamp);
    }
    if (options.showDuration === true && options.durationMs !== undefined && options.durationMs !== null) {
      const duration = formatDuration(options.durationMs);
    if (duration) metaParts.push(uiWithRaw("用时 ", "Worked for ", duration));
    }
    if (metaParts.length) {
      const stamp = document.createElement("div");
      stamp.className = "message-meta";
      stamp.textContent = metaParts.join(" · ");
      if (actions) actions.prepend(stamp);
      else content.append(stamp);
    }
    output.append(article);
    if (details) installDisclosure(details, initiallyOpen);
    if (follow) scrollOutput(output, true);
    return { article, content: body, wrapper: content };
  }

  function appendDateSeparator(timestamp, options = {}) {
    const parsedTimestamp = timestampMs(timestamp);
    const role = String(options.role || "user");
    // The official timestamp projection treats an item without a usable time
    // as an adjacency break. Do not carry the previous role/time across an
    // untimestamped item, otherwise a later assistant message can inherit an
    // unrelated 10-minute/1-hour gap.
    if (parsedTimestamp === null) {
      state.lastRenderedDateKey = "";
      state.lastRenderedTimestamp = null;
      state.lastRenderedRole = "";
      if (role === "user") state.hasRenderedUser = true;
      return;
    }
    const previousTimestamp = state.lastRenderedTimestamp;
    const previousRole = state.lastRenderedRole;
    const hour = 60 * 60 * 1000;
    const tenMinutes = 10 * 60 * 1000;
    const gap = previousTimestamp === null ? null : parsedTimestamp - previousTimestamp;
    // This mirrors the official timestamps projection: a first/next user turn
    // is separated only after a substantial pause, while consecutive assistant
    // entries can be separated after a shorter gap.
    const threshold = previousRole === "assistant"
      ? role === "user" ? hour : tenMinutes
      : Infinity;
    const firstUserIsOld = !state.hasRenderedUser && role === "user"
      && Date.now() - parsedTimestamp > hour;
    // The official local composer always gives the first user message a
    // centered date/time anchor, even when that message was sent today. It is
    // also the visual boundary that separates a freshly attached history from
    // the input composer, so do not hide it merely because the turn is recent.
    const firstUserTurn = !state.hasRenderedUser && role === "user";
    const show = options.force === true || options.breaksPreviousAdjacency === true || firstUserIsOld
      || firstUserTurn
      || (gap !== null && gap > 0 && previousRole === "assistant" && gap > threshold);
    if (show && state.lastDateSeparatorTimestamp !== parsedTimestamp) {
      const label = formatMessageDate(parsedTimestamp);
      if (label) {
        const separator = document.createElement("div");
        separator.className = "date-separator";
        separator.setAttribute("role", "separator");
        separator.setAttribute("aria-label", label);
        const time = document.createElement("time");
        time.dateTime = new Date(parsedTimestamp).toISOString();
        const splitAt = label.lastIndexOf(" ");
        if (splitAt > 0 && splitAt < label.length - 1) {
          const dateLabel = document.createElement("span");
          dateLabel.className = "date-label";
          dateLabel.textContent = label.slice(0, splitAt);
          const timeLabel = document.createElement("span");
          timeLabel.className = "date-time";
          timeLabel.textContent = label.slice(splitAt + 1);
          time.append(dateLabel, " ", timeLabel);
        } else time.textContent = label;
        separator.append(time);
        $("output").append(separator);
        state.lastDateSeparatorTimestamp = parsedTimestamp;
      }
    }
    state.lastRenderedDateKey = messageDateKey(parsedTimestamp);
    state.lastRenderedTimestamp = parsedTimestamp;
    state.lastRenderedRole = role;
    if (role === "user") state.hasRenderedUser = true;
  }

  function turnDividerLabel(status, durationMs) {
    const duration = elapsedDuration(durationMs);
    const normalized = normalizeActivityStatus(status, "completed");
    if (normalized === "interrupted") return duration
      ? uiWithRaw("你在 ", "You stopped after ", duration, " 后停止了", "")
      : uiText("你停止了工作", "You stopped working");
    if (normalized === "failed") return duration
      ? uiWithRaw("执行失败 · ", "Action failed · ", duration)
      : uiText("执行失败", "Action failed");
    if (normalized === "inProgress") {
      const visible = elapsedDuration(durationMs);
      return visible ? uiWithRaw("用时 ", "Worked for ", visible) : uiText("正在处理", "Working");
    }
    return duration ? uiWithRaw("用时 ", "Worked for ", duration) : uiText("已完成", "Completed");
  }

  // A worked-for disclosure only has meaning when the turn owns at least one
  // concrete activity row. Keep this cleanup centralized so a transient
  // status-only row cannot leave an empty divider behind after it is retired.
  function removeEmptyTurnDivider(turnId) {
    const key = String(turnId || "");
    if (!key) return;
    const output = $("output");
    if (!output) return;
    const hasActivity = [...output.querySelectorAll(".message.activity")]
      .some((entry) => entry.dataset.turnId === key);
    if (hasActivity) return;
    const divider = state.turnDividers.get(key)
      || [...output.querySelectorAll(".turn-divider")]
        .find((entry) => entry.dataset.turnId === key)
      || null;
    if (divider?.parentNode) divider.remove();
    state.turnDividers.delete(key);
  }

  function retireActivity(activity) {
    if (!activity) return;
    const key = String(activity.key || "");
    const turnId = String(activity.turnId || "");
    if (key && state.activities.get(key) === activity) state.activities.delete(key);
    if (key) state.commandDisclosure.delete(key);
    if (state.liveActivityKey === key) state.liveActivityKey = null;
    if (state.activeAssistantActivityKey === key) {
      state.activeAssistantBody = null;
      state.activeAssistantStream = null;
      state.activeAssistantText = "";
      state.activeAssistantActivityKey = null;
    }
    if (activity.article?.parentNode) activity.article.remove();
    removeEmptyTurnDivider(turnId);
    stopActivityTimerIfIdle();
    updateScrollToBottom($("output"));
  }

  function retireStatusOnlyActivities(turnId = "") {
    const key = String(turnId || "");
    for (const activity of [...state.activities.values()]) {
      if (activity.statusOnly !== true || activity.concrete === true) continue;
      if (key && activity.turnId && activity.turnId !== key) continue;
      retireActivity(activity);
    }
  }

  function appendTurnDivider(turnId, status = "completed", durationMs, beforeArticle = null, options = {}) {
    const key = String(turnId || `anonymous-${state.activitySequence}`);
    const output = $("output");
    const autoPosition = beforeArticle === null;
    const turnEntries = [...output.querySelectorAll(".message")]
      .filter((entry) => entry.dataset.turnId === key);
    // The official worked-for disclosure owns the activity portion of a turn.
    // A final assistant answer remains visible below the disclosure; commentary
    // and concrete work rows are the entries that collapse underneath it.
    const firstTurnActivity = turnEntries.find((entry) => entry.classList.contains("activity")) || null;
    // Do not manufacture an empty worked-for row for a final-only turn. This
    // can happen when completion metadata arrives before any item lifecycle
    // event, or when a transient status row has just been retired.
    if (!firstTurnActivity) {
      removeEmptyTurnDivider(key);
      return null;
    }
    const firstTurnContent = firstTurnActivity
      || turnEntries.find((entry) => !entry.classList.contains("user"))
      || null;
    if (firstTurnContent) beforeArticle = firstTurnContent;
    else if (autoPosition) beforeArticle = turnEntries.at(-1) || null;
    let divider = state.turnDividers.get(key);
    if (!divider) {
      divider = [...output.querySelectorAll(".turn-divider")].find((entry) => entry.dataset.turnId === key) || null;
    }
    if (!divider) {
      divider = document.createElement("div");
      divider.className = "turn-divider";
      divider.dataset.turnId = key;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "turn-divider-toggle";
      const initialStatus = normalizeActivityStatus(status, "completed");
      const rememberedExpansion = state.turnExpansion.get(key);
      const initialExpanded = rememberedExpansion !== undefined
        ? rememberedExpansion
        : options.defaultExpanded === true || initialStatus === "inProgress";
      if (rememberedExpansion !== undefined || options.defaultExpanded === true) {
        button.dataset.userToggled = rememberedExpansion !== undefined ? "true" : "false";
      }
      button.setAttribute("aria-expanded", String(initialExpanded));
      const label = document.createElement("span");
      label.className = "turn-divider-label";
      const icon = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      icon.setAttribute("viewBox", "0 0 16 16");
      icon.setAttribute("aria-hidden", "true");
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", "m6 3 5 5-5 5");
      icon.append(path);
      button.append(label, icon);
      const rule = document.createElement("div");
      rule.className = "turn-divider-rule";
      divider.append(button, rule);
      button.addEventListener("click", () => {
        const expanded = button.getAttribute("aria-expanded") === "true";
        const next = !expanded;
        // Hydration may choose an expanded latest turn for parity with the
        // official panel. Once the reader explicitly toggles it, preserve
        // that choice across subsequent structured snapshots.
        button.dataset.userToggled = "true";
        state.turnExpansion.set(key, next);
        button.setAttribute("aria-expanded", String(next));
        setTurnActivityVisibility(key, next);
        if (next) {
          const firstVisibleEntry = [...output.querySelectorAll(".message.activity")]
            .filter((entry) => entry.dataset.turnId === key)
            .find((entry) => !entry.classList.contains("turn-collapsed"));
          if (firstVisibleEntry) scheduleTimelineReveal(firstVisibleEntry, 280);
        }
      });
      if (beforeArticle && beforeArticle.parentNode === output) output.insertBefore(divider, beforeArticle);
      else output.append(divider);
      state.turnDividers.set(key, divider);
    } else if (beforeArticle === false && divider.parentNode === output && output.lastElementChild !== divider) {
      // An additional activity can arrive after an already-created live
      // divider. Explicit `false` means place the divider at the current tail.
      output.append(divider);
    } else if (beforeArticle && beforeArticle.parentNode === output && divider !== beforeArticle) {
      // Completion can arrive after the final assistant article. Move the
      // existing disclosure instead of creating a second one at the tail.
      output.insertBefore(divider, beforeArticle);
    }
    const label = divider.querySelector(".turn-divider-label");
    if (durationMs !== null && durationMs !== undefined && finiteNumber(durationMs) !== null) {
      divider.dataset.durationMs = String(Math.max(0, Number(durationMs)));
    }
    const storedDuration = finiteNumber(durationMs, divider.dataset.durationMs);
    const liveDuration = durationMs === null || durationMs === undefined
      ? key === state.turnId && state.turnWorkStartedAt !== null
        ? Math.max(0, Date.now() - state.turnWorkStartedAt)
        : storedDuration
      : storedDuration;
    if (label) label.textContent = turnDividerLabel(status, liveDuration);
    const normalizedStatus = normalizeActivityStatus(status, "completed");
    divider.dataset.status = normalizedStatus;
    const toggle = divider.querySelector(".turn-divider-toggle");
    if (options.defaultExpanded === true && toggle?.dataset.userToggled !== "true") {
      toggle.dataset.defaultExpanded = "true";
    }
    if (normalizedStatus === "inProgress" && toggle?.dataset.userToggled !== "true") {
      toggle.setAttribute("aria-expanded", "true");
    }
    const rememberedExpansion = state.turnExpansion.get(key);
    // The latest completed turn is expanded on first render in the official
    // panel. A remembered click always wins, including an explicit collapse.
    const expanded = rememberedExpansion !== undefined
      ? rememberedExpansion
      : toggle?.dataset.userToggled === "true"
        ? toggle.getAttribute("aria-expanded") === "true"
        : normalizedStatus === "inProgress" || toggle?.dataset.defaultExpanded === "true";
    setTurnActivityVisibility(key, expanded);
    return divider;
  }

  function appendCompletedTurnDivider(turnId, status = "completed", durationMs) {
    const key = String(turnId || "");
    if (!key) return null;
    const output = $("output");
    if (!output) return null;
    const hasTurnMessage = [...output.querySelectorAll(".message")]
      .some((article) => article.dataset.turnId === key);
    if (!hasTurnMessage) return null;
    const hasTurnActivity = [...output.querySelectorAll(".message.activity")]
      .some((article) => article.dataset.turnId === key);
    // A final assistant article by itself is not a worked-for group. The
    // official UI shows its duration in the assistant block without an empty
    // disclosure row above it.
    if (!hasTurnActivity) return null;
    const firstContent = [...output.querySelectorAll(".message")]
      .find((entry) => entry.dataset.turnId === key && entry.classList.contains("activity"))
      || [...output.querySelectorAll(".message")]
        .find((entry) => entry.dataset.turnId === key && !entry.classList.contains("user"))
      || null;
    return appendTurnDivider(key, status, durationMs, firstContent);
  }

  // Structured snapshots can append bookkeeping or collaboration items after
  // the final assistant item. Reconcile the outer worked-for boundary after
  // hydration so every activity row remains in the same disclosure group.
  function reconcileTurnDividers() {
    const output = $("output");
    if (!output) return;
    const turnIds = new Set(
      [...output.querySelectorAll(".message.activity")]
        .map((article) => article.dataset.turnId)
        .filter(Boolean),
    );
    for (const key of turnIds) {
      const activities = [...output.querySelectorAll(".message.activity")]
        .filter((article) => article.dataset.turnId === key);
      if (!activities.length) continue;
      let divider = state.turnDividers.get(key)
        || [...output.querySelectorAll(".turn-divider")].find((entry) => entry.dataset.turnId === key)
        || null;
      const finalAssistant = [...output.querySelectorAll(".message")]
        .filter((article) => article.dataset.turnId === key
          && !article.classList.contains("activity")
          && article.dataset.kind === "assistant")
        .at(-1)
        || null;
      const activeTurn = key === state.turnId
        && state.turnStartedAt !== null
        && ["active", "waiting"].includes(state.turnStatus);
      if (!divider) {
        divider = appendTurnDivider(key, activeTurn ? "inProgress" : "completed", state.workedDurationMs, activities[0]);
      } else {
        if (activities[0].parentNode === output && divider.parentNode === output && divider !== activities[0]) {
          output.insertBefore(divider, activities[0]);
        }
        const toggle = divider.querySelector(".turn-divider-toggle");
        if (toggle && toggle.dataset.userToggled !== "true") {
          const rememberedExpansion = state.turnExpansion.get(key);
          toggle.setAttribute("aria-expanded", String(rememberedExpansion !== undefined
            ? rememberedExpansion
            : activeTurn));
        }
        const status = activeTurn ? "inProgress" : normalizeActivityStatus(divider.dataset.status, "completed");
        divider.dataset.status = status;
        setTurnActivityVisibility(key, toggle?.getAttribute("aria-expanded") === "true");
      }
      // Keep a divider before the activity group even when a final answer was
      // rendered first. The answer itself intentionally stays outside it.
      if (divider && activities[0].parentNode === output && divider.nextSibling !== activities[0]) {
        output.insertBefore(divider, activities[0]);
      }
      if (finalAssistant && divider) divider.dataset.hasFinalAssistant = "true";
      if (finalAssistant) {
        // Keep trailing bookkeeping/collaboration rows inside the same outer
        // group. Moving only rows that currently follow the final response
        // preserves the chronological order of the already-correct prefix.
        const trailing = activities.filter((activity) => (
          activity.compareDocumentPosition(finalAssistant) & Node.DOCUMENT_POSITION_PRECEDING
        ));
        for (const activity of trailing) output.insertBefore(activity, finalAssistant);
        // The moves above can carry the first activity past the existing
        // disclosure. Re-anchor it after every move so the worked-for button
        // always remains the first node in the activity group.
        if (activities[0].parentNode === output && divider.parentNode === output) {
          output.insertBefore(divider, activities[0]);
        }
      }
    }
  }

  function expandLatestTurnActivity() {
    const output = $("output");
    if (!output) return;
    const dividers = [...output.querySelectorAll(".turn-divider")];
    const divider = dividers.at(-1);
    // The latest worked-for group is expanded after a completed snapshot so
    // the reader can see the activity stream immediately. Older groups stay
    // compact, and an explicit user toggle always wins.
    if (!divider) return;
    const toggle = divider.querySelector(".turn-divider-toggle");
    if (!toggle || toggle.dataset.userToggled === "true") return;
    const key = divider.dataset.turnId || "";
    if (!key || ![...output.querySelectorAll(".message.activity")].some((entry) => entry.dataset.turnId === key)) return;
    const rememberedExpansion = state.turnExpansion.get(key);
    const shouldExpand = rememberedExpansion !== undefined
      ? rememberedExpansion
      : divider.dataset.status === "inProgress" || divider === dividers.at(-1);
    if (!shouldExpand) return;
    if (toggle.getAttribute("aria-expanded") === "true") return;
    toggle.setAttribute("aria-expanded", "true");
    setTurnActivityVisibility(key, true);
  }

  function setTurnActivityVisibility(turnId, expanded) {
    const key = String(turnId || "");
    if (!key) return;
    const output = $("output");
    if (!output) return;
    const divider = state.turnDividers.get(key)
      || [...output.querySelectorAll(".turn-divider")].find((entry) => entry.dataset.turnId === key)
      || null;
    // Keep the final assistant response and its timestamp/actions mounted.
    // Only rows explicitly marked as activity participate in the outer turn
    // disclosure, matching the official conversation-blocks split.
    const activities = [...output.querySelectorAll(".message.activity")]
      .filter((entry) => entry.dataset.turnId === key);
    if (!activities.length) return;
    const knownState = activities.some((activity) => activity.dataset.turnExpanded !== undefined);
    if (!knownState && !expanded) {
      for (const activity of activities) {
        activity.dataset.turnExpanded = "false";
        animateActivityArticle(activity, false, { immediate: true });
      }
      return;
    }
    if (divider || activities[0]) preserveTimelineAnchor(divider || activities[0], 280);
    for (const activity of activities) {
      const previous = activity.dataset.turnExpanded === "true";
      activity.dataset.turnExpanded = String(Boolean(expanded));
      // The outer worked-for disclosure controls every non-user entry. Each
      // nested command/reasoning disclosure keeps its own state, matching the
      // official panel where opening a completed turn does not open every
      // terminal body at once.
      animateActivityArticle(activity, expanded, {
        immediate: !knownState && !previous,
        fromHeight: expanded && !previous ? 0 : undefined,
        preserve: false,
      });
    }
    if (expanded) revealTerminalOutputs(key);
  }

  function revealTerminalOutputs(turnId) {
    const key = String(turnId || "");
    if (!key) return;
    requestAnimationFrame(() => {
      const output = $("output");
      if (!output) return;
      for (const activity of output.querySelectorAll(".message.activity")) {
        if (activity.dataset.turnId !== key) continue;
        const terminal = activity.querySelector(".terminal-output");
        if (!terminal || terminal.scrollHeight <= terminal.clientHeight) continue;
        // Hidden turn sections have no layout while hydrated. Once revealed,
        // show the newest terminal lines just like the official reverse list.
        terminal.scrollTop = terminal.scrollHeight;
        updateTerminalOutputFade(terminal);
      }
    });
  }

  function ensureLiveTurnDivider(turnId) {
    const key = String(turnId || "");
    if (!key) return null;
    const output = $("output");
    const entries = [...output.querySelectorAll(".message")]
      .filter((entry) => entry.dataset.turnId === key);
    const firstContent = entries.find((entry) => entry.classList.contains("activity"))
      || entries.find((entry) => !entry.classList.contains("user"))
      || null;
    if (firstContent) return appendTurnDivider(key, "inProgress", null, firstContent);
    return appendTurnDivider(key, "inProgress", null, false);
  }

  const isRecord = (value) => Boolean(value && typeof value === "object" && !Array.isArray(value));

  function finiteNumber(...values) {
    for (const value of values) {
      if (value === null || value === undefined || value === "" || typeof value === "boolean") continue;
      const number = Number(value);
      if (Number.isFinite(number)) return number;
    }
    return null;
  }

  function timestampMs(...values) {
    let value = null;
    for (const candidate of values) {
      if (candidate === null || candidate === undefined || candidate === "" || typeof candidate === "boolean") continue;
      if (typeof candidate === "string" && candidate.trim() && !/^\d+(?:\.\d+)?$/.test(candidate.trim())) {
        const parsed = Date.parse(candidate);
        if (Number.isFinite(parsed)) { value = parsed; break; }
      }
      const numeric = Number(candidate);
      if (Number.isFinite(numeric)) { value = numeric; break; }
    }
    if (value === null || value <= 0) return null;
    // Accept ISO epoch seconds from older followers as well as the current
    // app-server's millisecond fields.
    return value < 100_000_000_000 ? value * 1_000 : value;
  }

  function workedDurationFor(value, fallback = null) {
    const source = isRecord(value) ? value : {};
    const nestedTurn = isRecord(source.turn) ? source.turn : {};
    const nestedStatus = isRecord(source.status) ? source.status : {};
    const explicit = finiteNumber(
      source.workedDurationMs,
      source.workDurationMs,
      source.workedForMs,
      source.turnWorkedDurationMs,
      source.worked_for_ms,
      nestedTurn.workedDurationMs,
      nestedTurn.workDurationMs,
      nestedTurn.workedForMs,
      nestedStatus.workedDurationMs,
      nestedStatus.workDurationMs,
    );
    if (explicit !== null) return Math.max(0, explicit);
    const started = timestampMs(
      source.firstTurnWorkItemStartedAtMs,
      source.workStartedAtMs,
      source.turnWorkStartedAtMs,
      nestedTurn.firstTurnWorkItemStartedAtMs,
      nestedTurn.workStartedAtMs,
      nestedTurn.turnStartedAtMs,
    );
    const completed = timestampMs(
      source.finalAssistantStartedAtMs,
      source.workCompletedAtMs,
      nestedTurn.finalAssistantStartedAtMs,
      nestedTurn.workCompletedAtMs,
      source.completedAtMs,
    );
    if (started !== null && completed !== null) return Math.max(0, completed - started);
    return finiteNumber(fallback);
  }

  function formatDuration(value) {
    const duration = finiteNumber(value);
    if (duration === null || duration < 0) return "";
    if (uiLocale() === "en-US") {
      if (duration < 1_000) return `${Math.round(duration)}ms`;
      const totalSeconds = Math.floor(duration / 1_000);
      const minutes = Math.floor(totalSeconds / 60);
      const seconds = totalSeconds % 60;
      if (!minutes) return `${seconds}s`;
      return seconds ? `${minutes}m ${seconds}s` : `${minutes}m`;
    }
    if (duration < 1_000) return `${Math.round(duration)}毫秒`;
    const totalSeconds = Math.floor(duration / 1_000);
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    if (!minutes) return `${seconds}秒`;
    // The Chinese locale in the official panel uses the long form for one
    // minute and the compact form for longer durations.
    const minuteLabel = minutes === 1 ? "1分钟" : `${minutes}分`;
    return seconds ? `${minuteLabel}${seconds}秒` : minuteLabel;
  }

  // Working indicators in the official panel stay textual until a full
  // second has elapsed. This avoids the visually noisy "0ms"/"1ms" state on
  // every newly-created activity row while preserving precise durations once
  // an operation is complete.
  function elapsedDuration(value) {
    const duration = finiteNumber(value);
    return duration !== null && duration >= 1_000 ? formatDuration(duration) : "";
  }

  function formatMessageTime(value) {
    const timestamp = timestampMs(value);
    if (timestamp === null) return "";
    try {
      return new Intl.DateTimeFormat(uiLocale(), { hour: "2-digit", minute: "2-digit" }).format(new Date(timestamp));
    } catch {
      return "";
    }
  }

  function formatMessageDate(value, nowValue = Date.now()) {
    const timestamp = timestampMs(value);
    if (timestamp === null) return "";
    try {
      const date = new Date(timestamp);
      const now = new Date(nowValue);
      // Match the official timestamp separator: compare calendar dates while
      // avoiding DST changes in the local timezone.
      const dateDay = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
      const nowDay = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate());
      const dayDifference = Math.max(0, Math.round((nowDay - dateDay) / 86_400_000));
      const locale = uiLocale();
      const time = new Intl.DateTimeFormat(locale, { hour: "numeric", minute: "2-digit" }).format(date);
      if (dayDifference <= 1) {
        try {
          const relative = new Intl.RelativeTimeFormat(locale, { numeric: "auto" }).format(-Math.max(dayDifference, 0), "day");
          return `${relative} ${time}`;
        } catch {
          return `${t(dayDifference === 1 ? "昨天" : "今天")} ${time}`;
        }
      }
      if (dayDifference <= 7 && dayDifference > 0) {
        const weekday = new Intl.DateTimeFormat(locale, { weekday: "long" }).format(date);
        return `${weekday} ${time}`;
      }
      const datePart = dayDifference <= 365
        ? new Intl.DateTimeFormat(locale, { month: "short", day: "numeric", weekday: "short" }).format(date)
        : new Intl.DateTimeFormat(locale, { year: "numeric", month: "short", day: "numeric" }).format(date);
      return `${datePart} ${time}`;
    } catch {
      return "";
    }
  }

  function messageDateKey(value) {
    const timestamp = timestampMs(value);
    if (timestamp === null) return "";
    const date = new Date(timestamp);
    return `${date.getFullYear()}-${date.getMonth()}-${date.getDate()}`;
  }

  function normalizeActivityStatus(value, fallback = "inProgress") {
    const normalized = String(value ?? fallback).replace(/[\s_-]+/g, "").toLowerCase();
    if (["inprogress", "running", "started", "active", "pending"].includes(normalized)) return "inProgress";
    if (["completed", "complete", "success", "succeeded", "done"].includes(normalized)) return "completed";
    if (["failed", "failure", "error"].includes(normalized)) return "failed";
    if (["declined", "denied", "rejected"].includes(normalized)) return "declined";
    if (["interrupted", "cancelled", "canceled", "aborted"].includes(normalized)) return "interrupted";
    return fallback;
  }

  function activityStatusLabel(status) {
    if (status === "inProgress") return "进行中";
    if (status === "completed") return "已完成";
    if (status === "failed") return "失败";
    if (status === "declined") return "已拒绝";
    if (status === "interrupted") return "已中断";
    return String(status || "");
  }

  const isRunningActivity = (activity) => activity && activity.status === "inProgress";

  function eventParams(payload) {
    if (!isRecord(payload)) return {};
    return isRecord(payload.params) ? payload.params : payload;
  }

  function eventThreadId(payload) {
    const params = eventParams(payload);
    return payload?.threadId || params.threadId || params.thread?.id || params.turn?.threadId || "";
  }

  function eventTurnId(payload) {
    const params = eventParams(payload);
    return payload?.turnId || params.turnId || params.turn?.id || params.item?.turnId || "";
  }

  function itemFromPayload(payload) {
    const params = eventParams(payload);
    if (isRecord(params.item)) return params.item;
    if (isRecord(payload?.item)) return payload.item;
    return isRecord(params) && (params.type || params.kind) ? params : {};
  }

  function normalizedItemType(item) {
    return String(item?.type || item?.kind || "").replace(/[\s/_.-]+/g, "").toLowerCase();
  }

  function activityKindForItem(item) {
    const type = normalizedItemType(item);
    if (isReadActivity(item)) return "read";
    if (!type) return "";
    if (type.includes("subagent") || type.includes("collabagent")) return "subagent";
    if (type.includes("filechange") || type.includes("patch") || type.includes("edit")) return "edit";
    if (type.includes("reasoning") || type.includes("contextcompaction")) return "reasoning";
    if (type.includes("plan") || type.includes("reviewmode")) return "plan";
    if (type.includes("command") || type.includes("exec") || type.includes("process")) return "tool";
    if (type.includes("tool") || type.includes("mcp") || type.includes("websearch") || type.includes("imageview")) return "tool";
    if (type.includes("usermessage")) return "user";
    if (type.includes("agentmessage") || type.includes("assistantmessage")) return "assistant";
    return "";
  }

  function activityLabelForItem(item, kind) {
    const type = normalizedItemType(item);
    if (kind === "subagent" || type.includes("subagent") || type.includes("collabagent")) {
      return firstString(item.displayName, item.agentPath, item.action) || "子代理";
    }
    if (type.includes("contextcompaction")) return "整理上下文";
    if (type.includes("websearch")) return "搜索";
    if (type.includes("imageview")) return "查看图像";
    if (type.includes("mcp")) return "MCP 工具";
    if (type.includes("dynamictool")) return "工具";
    if (kind === "edit") return "编辑文件";
    if (kind === "read") return "读取文件";
    if (kind === "plan") return "计划";
    if (kind === "reasoning") return "思考";
    if (kind === "commentary") return "工作说明";
    if (kind === "tool") return "运行命令";
    return "执行步骤";
  }

  function historyActivitySummary(item, kind, status, duration) {
    const elapsed = duration ? ` · ${duration}` : "";
    if (kind === "subagent") {
      const name = firstString(item.displayName, item.agentPath, item.agentThreadId ? `thread ${item.agentThreadId}` : "") || t("子代理");
      const uiStatus = firstString(item.displayStatus, item.activityKind);
      if (status === "inProgress" || uiStatus === "active" || uiStatus === "updated") return `${name} ${t("已开始工作")}`;
      if (status === "failed") return `${name} ${t("失败")}`;
      if (status === "interrupted") return `${name} ${t("已中断")}`;
      return `${name} ${t("已完成")}`;
    }
    if (kind === "tool" || kind === "read") {
      const command = terminalCommandText(commandText(item));
      const actionSource = String(item.label || "工具");
      const action = t(actionSource);
      const path = firstString(...readPathList(item));
      if (kind === "read" && status === "inProgress") return readSummaryLabel(item, status);
      if (kind === "read" && !command) {
        return readSummaryLabel(item, status);
      }
      if (kind === "read" && status !== "inProgress") {
        // Parsed read/search actions are coalesced by the official renderer
        // into one compact exploration row rather than a path plus a second
        // timed command row.
        if (command) return status === "failed" ? t("读取文件运行命令失败") : status === "interrupted" ? t("已停止读取文件运行命令") : t("已读取文件运行了命令");
        return readSummaryLabel(item, status);
      }
      if (status === "inProgress") return command
        ? uiWithRaw("正在运行 ", "Running ", command)
        : uiLocale() === "en-US" ? `Running ${action}` : `正在${action}`;
      if (status === "failed") return command
        ? `${uiWithRaw("命令运行失败 · ", "Command failed · ", command)}${elapsed}`
        : `${action}${uiText("失败", " failed")}${elapsed}`;
      if (status === "interrupted") return command
        ? `${uiWithRaw("已停止 ", "Stopped ", command)}${elapsed}`
        : `${uiText("已停止", "Stopped ")}${action}${elapsed}`;
      if (command) return duration
        ? `${uiWithRaw("已在 ", "Ran ", command, " 内运行 ", " in ")}${duration}`
        : uiWithRaw("已运行 ", "Ran ", command);
      return uiLocale() === "en-US"
        ? `Ran ${action}${elapsed}`
        : `已${action}${elapsed}`;
    }
    if (kind === "edit") return status === "inProgress" ? t("正在编辑文件") : `${t("编辑了文件")}${elapsed}`;
    if (kind === "reasoning") return status === "inProgress" ? t("正在思考") : duration ? uiWithRaw("已思考 ", "Thought for ", duration) : t("已完成思考");
    if (kind === "plan") return status === "inProgress" ? t("正在制定计划") : `${t("已完成计划")}${elapsed}`;
    if (kind === "commentary") {
      if (status === "inProgress") return t("正在处理");
      return t("工作说明");
    }
    return "";
  }

  function textFromValue(value, depth = 0) {
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean") return String(value);
    if (Array.isArray(value)) {
      const parts = value.map((entry) => textFromValue(entry, depth + 1)).filter(Boolean);
      return parts.length ? parts.join("\n") : "";
    }
    if (!isRecord(value) || depth > 3) return "";
    for (const key of ["text", "value", "output", "stdout", "stderr", "delta", "summary", "message"]) {
      const text = textFromValue(value[key], depth + 1);
      if (text) return text;
    }
    return "";
  }

  function displayValue(value) {
    const text = textFromValue(value);
    if (text) return text;
    if (value === undefined || value === null) return "";
    try { return JSON.stringify(value, null, 2); } catch { return String(value); }
  }

  function commandText(item) {
    if (Array.isArray(item.command)) return item.command.map(String).join(" ");
    if (typeof item.command === "string") return item.command;
    if (typeof item.commandLine === "string") return item.commandLine;
    if (Array.isArray(item.commandActions)) {
      return item.commandActions
        .map((action) => isRecord(action) ? action.command || action.description || "" : "")
        .filter(Boolean)
        .join("\n");
    }
    return "";
  }

  function fileChangesText(changes) {
    if (!Array.isArray(changes)) return displayValue(changes);
    return changes.map((change) => {
      if (!isRecord(change)) return displayValue(change);
      const path = change.path || change.file || change.filePath || change.name || "文件";
      const kind = change.kind || change.type || change.status || "";
      const diff = displayValue(change.diff || change.patch || change.output || change.text);
      const heading = `${kind ? `[${kind}] ` : ""}${path}`;
      return diff ? `${heading}\n${diff}` : heading;
    }).filter(Boolean).join("\n\n");
  }

  function planText(value) {
    const plan = Array.isArray(value) ? value : value?.plan || value?.steps;
    if (!Array.isArray(plan)) return displayValue(value?.text || value);
    return plan.map((entry) => {
      if (!isRecord(entry)) return `- ${displayValue(entry)}`;
      const status = normalizeActivityStatus(entry.status, "pending");
      const marker = status === "completed" ? "[x]" : status === "inProgress" ? "[~]" : "[ ]";
      const step = entry.step || entry.text || entry.title || entry.description || "步骤";
      return `${marker} ${step}`;
    }).join("\n");
  }

  function activityHeader(item, kind) {
    // Terminal activities render command/cwd as separate fields below. Keep
    // this helper for non-terminal activity kinds and future item types.
    return "";
  }

  function activityOutput(item, kind) {
    if (kind === "reasoning") return displayValue(item.summary || item.content || item.text);
    if (kind === "plan") return planText(item.plan || item.steps || item.content || item.text);
    if (kind === "edit") return fileChangesText(item.changes || item.files || item.diff || item.patch || item.output || item.text);
    if (kind === "tool" || kind === "read") {
      if (kind === "read") {
        // Paths are rendered as their own compact exploration rows. Keep only
        // the actual file content here; adapter summaries such as
        // "已读取 ..." must not be repeated inside a Shell block.
        let value = displayValue(item.aggregatedOutput ?? item.output ?? item.stdout ?? item.stderr ?? item.result);
        const summary = firstString(item.text, item.summary);
        if (summary && value === summary) return "";
        if (summary && value.startsWith(`${summary}\n`)) value = value.slice(summary.length + 1);
        return value;
      }
      return displayValue(item.aggregatedOutput ?? item.output ?? item.stdout ?? item.stderr ?? item.result ?? item.error ?? item.text);
    }
    return displayValue(item.text || item.content || item.output);
  }

  function activityKey(payload, item, kind, explicitKey) {
    if (explicitKey) return explicitKey;
    const params = eventParams(payload);
    const id = item.id ?? params.itemId ?? payload?.itemId;
    const threadId = eventThreadId(payload) || state.threadId || "thread";
    const turnId = eventTurnId(payload) || state.turnId || "turn";
    if (id !== undefined && id !== null) return `${threadId}:${turnId}:${kind}:${typeof id}:${String(id)}`;
    state.activitySequence += 1;
    return `${threadId}:${turnId}:${kind}:anonymous:${state.activitySequence}`;
  }

  function ensureActivity(key, config = {}) {
    const kind = config.kind || "reasoning";
    let existing = state.activities.get(key);
    if (!existing) {
      const requestedItemId = config.itemId === undefined || config.itemId === null ? "" : String(config.itemId);
      const sameTurn = (entry) => entry.kind === kind
        && (!config.turnId || !entry.turnId || entry.turnId === String(config.turnId));
      const entries = [...state.activities.entries()].reverse();
      const exact = requestedItemId
        ? entries.find(([, entry]) => sameTurn(entry) && entry.itemId === requestedItemId)
        : null;
      // Status snapshots may arrive before the item lifecycle notification.
      // When the concrete item appears, adopt the still-running anonymous row
      // instead of appending a second "正在思考/读取/编辑" entry.
      const anonymous = entries.find(([, entry]) => sameTurn(entry)
        && entry.anonymous
        && isRunningActivity(entry));
      const unkeyed = !requestedItemId
        ? entries.find(([, entry]) => sameTurn(entry) && !entry.itemId && isRunningActivity(entry))
        : null;
      const match = exact || anonymous || unkeyed;
      if (match) {
        const [oldKey, candidate] = match;
        existing = candidate;
        if (oldKey !== key) {
          state.activities.delete(oldKey);
          existing.key = key;
          existing.article.dataset.activityKey = key;
          if (state.liveActivityKey === oldKey) state.liveActivityKey = key;
          if (state.activeAssistantActivityKey === oldKey) state.activeAssistantActivityKey = key;
        }
        if (requestedItemId && existing.anonymous) {
          existing.itemId = requestedItemId;
          existing.anonymous = false;
        }
        state.activities.set(key, existing);
      }
    }
    if (existing) {
      // A lifecycle event or real output upgrades a transient status row into
      // a concrete activity. Once upgraded, it must survive turn completion;
      // status-only rows are retired when the host reports a terminal state.
      if (config.concrete === true) {
        existing.concrete = true;
        existing.statusOnly = false;
        // Keep the anonymous marker for id-less lifecycle streams so the
        // terminal status path can still close them when the host omits an
        // explicit item/completed event. An identified item is authoritative.
        if (config.itemId !== undefined && config.itemId !== null && String(config.itemId)) {
          existing.anonymous = false;
        }
      } else if (config.statusOnly === true && existing.concrete !== true) {
        existing.statusOnly = true;
      }
      if (config.label) existing.label = config.label;
      if (config.itemId !== undefined) existing.itemId = String(config.itemId);
      if (config.command) existing.command = kind === "tool" || kind === "read" ? terminalCommandText(config.command) : String(config.command);
      if (config.filePath !== undefined) existing.filePath = String(config.filePath || "");
      if (config.cwd !== undefined) existing.cwd = String(config.cwd || "");
      if (config.shellName) existing.shellName = String(config.shellName);
      if (config.agentThreadId !== undefined) existing.agentThreadId = String(config.agentThreadId || "");
      if (config.displayName !== undefined) existing.displayName = String(config.displayName || "");
      if (config.objective !== undefined) existing.objective = String(config.objective || "");
      if (config.activityKind !== undefined) existing.activityKind = String(config.activityKind || "");
      if (config.displayStatus !== undefined) existing.displayStatus = String(config.displayStatus || "");
      if (config.model !== undefined) existing.model = String(config.model || "");
      if (config.action !== undefined) existing.action = String(config.action || "");
      if (config.prompt !== undefined) existing.prompt = String(config.prompt || "");
      if (config.senderThreadId !== undefined) existing.senderThreadId = String(config.senderThreadId || "");
      if (config.receiverThreadIds !== undefined) existing.receiverThreadIds = Array.isArray(config.receiverThreadIds) ? config.receiverThreadIds.map(String) : [];
      if (config.agentsStates !== undefined) existing.agentsStates = isRecord(config.agentsStates) ? config.agentsStates : {};
      if (config.canInteract !== undefined) existing.canInteract = config.canInteract !== false;
      if (config.exitCode !== undefined) existing.exitCode = finiteNumber(config.exitCode);
      if (config.turnId) existing.turnId = String(config.turnId);
      if (config.startedAt) existing.startedAt = timestampMs(config.startedAt) || existing.startedAt;
      if (existing.agentThreadId) existing.article.dataset.agentThreadId = existing.agentThreadId;
      else delete existing.article.dataset.agentThreadId;
      if (kind === "tool" || kind === "read" || kind === "subagent" || kind === "commentary") renderActivityText(existing);
      refreshActivity(existing);
      if (existing.turnId && state.turnStartedAt !== null && ["active", "waiting"].includes(state.turnStatus)) {
        ensureLiveTurnDivider(existing.turnId);
      }
      return existing;
    }
    const role = kind === "commentary"
      ? "assistant"
      : kind === "tool" || kind === "read" || kind === "edit" || kind === "subagent" ? "tool" : "system";
    const messageKind = kind === "tool" || kind === "read" ? "tool" : kind === "reasoning" ? "reasoning" : kind === "plan" ? "plan" : kind;
    const message = appendMessage("", role, "activity", "", {
      kind: messageKind,
      label: config.label || "执行步骤",
      command: config.command ? (kind === "tool" || kind === "read" ? terminalCommandText(config.command) : String(config.command)) : "",
      turnId: config.turnId || "",
      agentThreadId: config.agentThreadId || "",
      // Commentary is an assistant paragraph in the official transcript, not
      // a nested disclosure. The outer worked-for group still owns its layout.
      collapsible: kind !== "commentary",
      showActions: false,
      // Keep commentary visible while a turn is open; command/read/reasoning
      // bodies remain independently collapsible.
      open: config.open === true || kind === "commentary",
    });
    if (!message) return null;
    const activity = {
      key,
      kind,
      role,
      messageKind,
      label: config.label || "执行步骤",
      command: config.command ? (kind === "tool" || kind === "read" ? terminalCommandText(config.command) : String(config.command)) : "",
      cwd: config.cwd ? String(config.cwd) : "",
      shellName: config.shellName ? String(config.shellName) : "Shell",
      agentThreadId: config.agentThreadId ? String(config.agentThreadId) : "",
      displayName: config.displayName ? String(config.displayName) : "",
      objective: config.objective ? String(config.objective) : "",
      activityKind: config.activityKind ? String(config.activityKind) : "",
      displayStatus: config.displayStatus ? String(config.displayStatus) : "",
      model: config.model ? String(config.model) : "",
      action: config.action ? String(config.action) : "",
      filePath: config.filePath ? String(config.filePath) : "",
      prompt: config.prompt ? String(config.prompt) : "",
      senderThreadId: config.senderThreadId ? String(config.senderThreadId) : "",
      receiverThreadIds: Array.isArray(config.receiverThreadIds) ? config.receiverThreadIds.map(String) : [],
      agentsStates: isRecord(config.agentsStates) ? config.agentsStates : {},
      canInteract: config.canInteract !== false,
      exitCode: config.exitCode === undefined ? null : finiteNumber(config.exitCode),
      itemId: config.itemId === undefined ? "" : String(config.itemId),
      threadId: config.threadId || state.threadId || "",
      turnId: config.turnId || state.turnId || "",
      startedAt: timestampMs(config.startedAt) || Date.now(),
      finishedAt: null,
      durationMs: null,
      durationExplicit: false,
      status: normalizeActivityStatus(config.status, "inProgress"),
      headerText: "",
      outputText: "",
      anonymous: Boolean(config.anonymous),
      concrete: config.concrete === true,
      statusOnly: config.statusOnly === true,
      article: message.article,
      body: message.content,
      wrapper: message.wrapper,
      details: message.article.querySelector("details"),
      summary: message.article.querySelector("summary"),
    };
    activity.article.dataset.activityKey = key;
    activity.article.dataset.activityKind = kind;
    if (activity.agentThreadId) activity.article.dataset.agentThreadId = activity.agentThreadId;
    state.activities.set(key, activity);
    while (state.activities.size > 500) {
      const removable = [...state.activities].find(([, entry]) => !isRunningActivity(entry));
      if (!removable) break;
      state.activities.delete(removable[0]);
    }
    renderActivityText(activity);
    refreshActivity(activity);
    if (activity.turnId && state.turnStartedAt !== null && ["active", "waiting"].includes(state.turnStatus)) {
      ensureLiveTurnDivider(activity.turnId);
    }
    ensureActivityTimer();
    return activity;
  }

  function activityText(activity) {
    if (activity.kind === "tool" || activity.kind === "read") {
      const command = terminalCommandText(activity.command);
      const commandLine = command ? `$ ${command}` : "";
      return [commandLine, activity.outputText].filter(Boolean).join("\n");
    }
    if (activity.kind === "subagent") {
      const name = activity.displayName || activity.label || t("子代理");
      const objective = activity.objective || activity.outputText;
      return [name, objective].filter(Boolean).join("\n");
    }
    if (activity.headerText && activity.outputText) return `${activity.headerText}\n\n${activity.outputText}`;
    return activity.headerText || activity.outputText || "";
  }

  function normalizeSubagentActionStatus(value) {
    const normalized = String(value || "").replace(/[\s_-]+/g, "").toLowerCase();
    if (["pendinginit", "pending", "waiting"].includes(normalized)) return "waiting";
    if (["running", "working", "active", "started", "interacted", "updated", "inprogress"].includes(normalized)) return "working";
    if (["completed", "complete", "done", "interrupted", "shutdown"].includes(normalized)) return "done";
    if (["errored", "error", "failed", "notfound"].includes(normalized)) return "failed";
    return "waiting";
  }

  function renderSubagentBody(activity) {
    const body = activity?.body;
    if (!body) return;
    body.replaceChildren();
    body.classList.remove("terminal-body", "diff-body");
    body.classList.add("subagent-body");

    const prompt = firstString(activity.prompt, activity.objective, activity.outputText);
    if (prompt) {
      const promptNode = document.createElement("div");
      promptNode.className = "subagent-prompt markdown-body";
      renderMarkdown(promptNode, prompt);
      body.append(promptNode);
    }
    if (activity.model || activity.action) {
      const meta = document.createElement("div");
      meta.className = "subagent-action-meta";
      if (activity.action) {
        const action = document.createElement("span");
        action.textContent = activity.action === "spawnAgent" ? t("启动子代理")
          : activity.action === "sendInput" ? t("发送输入")
            : activity.action === "resumeAgent" ? t("恢复子代理")
              : activity.action === "closeAgent" ? t("关闭子代理") : activity.action;
        meta.append(action);
      }
      if (activity.model) {
        const model = document.createElement("span");
        model.className = "subagent-model";
        model.textContent = activity.model;
        meta.append(model);
      }
      body.append(meta);
    }

    const states = isRecord(activity.agentsStates) ? activity.agentsStates : {};
    const receiverIds = Array.isArray(activity.receiverThreadIds) ? activity.receiverThreadIds : [];
    const ids = [...new Set([...receiverIds, ...Object.keys(states)])].filter(Boolean);
    if (!ids.length) return;
    const rows = document.createElement("div");
    rows.className = "subagent-action-rows";
    for (const threadId of ids) {
      const raw = isRecord(states[threadId]) ? states[threadId] : {};
      const status = normalizeSubagentActionStatus(raw.status);
      const row = document.createElement("div");
      row.className = "subagent-action-row";
      row.dataset.status = status;
      const icon = document.createElement("span");
      icon.className = "subagent-action-icon";
      icon.setAttribute("aria-hidden", "true");
      const label = document.createElement("span");
      label.className = "subagent-action-label";
      label.textContent = threadId === activity.agentThreadId
        ? firstString(activity.displayName, threadId)
        : `thread ${threadId}`;
      const statusNode = document.createElement("span");
      statusNode.className = "subagent-action-status";
      statusNode.textContent = subagentStatusLabel(status);
      row.append(icon, label, statusNode);
      const message = firstString(raw.message, raw.statusMessage);
      if (message) {
        const note = document.createElement("div");
        note.className = "subagent-action-note";
        note.textContent = message;
        row.append(note);
      }
      rows.append(row);
    }
    body.append(rows);
  }

  function terminalOutputText(activity) {
    const value = String(activity?.outputText || "");
    // A few older bridge payloads included an exit-code suffix in the output
    // string. Strip only that exact synthetic line; real command output stays
    // untouched.
    return normalizeTerminalOutput(value.replace(/\n?exit code:\s*-?\d+\s*$/i, ""));
  }

  function normalizeTerminalOutput(value) {
    // Commands often use carriage returns/backspaces for progress updates and
    // ANSI SGR sequences for color. Resolve the control characters before the
    // lightweight renderer turns the result into safe DOM nodes.
    const stripped = String(value || "")
      .replace(/\x1b\][^\x07]*(?:\x07|\x1b\\)/g, "")
      .replace(/\x1b\[(?![0-9;]*m)[0-?]*[ -/]*[@-~]/g, "");
    return stripped.replace(/\r\n/g, "\n").split("\n").map((line) => {
      const cells = [];
      let cursor = 0;
      for (const character of line) {
        if (character === "\r") { cursor = 0; continue; }
        if (character === "\b") { cursor = Math.max(0, cursor - 1); continue; }
        cells[cursor] = character;
        cursor += 1;
      }
      return cells.join("");
    }).join("\n");
  }

  function renderTerminalOutput(parent, value) {
    parent.replaceChildren();
    const text = String(value || "");
    const sgr = /\x1b\[([0-9;]*)m/g;
    let cursor = 0;
    const style = { fg: "", bg: "", bold: false, dim: false, italic: false, underline: false, strike: false };
    const appendSegment = (segment) => {
      if (!segment) return;
      const classes = [];
      if (style.fg) classes.push(`ansi-${style.fg}-fg`);
      if (style.bg) classes.push(`ansi-${style.bg}-bg`);
      if (style.bold) classes.push("ansi-bold");
      if (style.dim) classes.push("ansi-dim");
      if (style.italic) classes.push("ansi-italic");
      if (style.underline) classes.push("ansi-underline");
      if (style.strike) classes.push("ansi-strikethrough");
      if (!classes.length) parent.append(document.createTextNode(segment));
      else {
        const span = document.createElement("span");
        span.className = classes.join(" ");
        span.textContent = segment;
        parent.append(span);
      }
    };
    const applySgr = (codes) => {
      const values = codes.length ? codes : [0];
      for (const code of values) {
        if (code === 0) Object.assign(style, { fg: "", bg: "", bold: false, dim: false, italic: false, underline: false, strike: false });
        else if (code === 1) style.bold = true;
        else if (code === 2) style.dim = true;
        else if (code === 3) style.italic = true;
        else if (code === 4) style.underline = true;
        else if (code === 9) style.strike = true;
        else if (code === 22) { style.bold = false; style.dim = false; }
        else if (code === 23) style.italic = false;
        else if (code === 24) style.underline = false;
        else if (code === 29) style.strike = false;
        else if (code === 39) style.fg = "";
        else if (code === 49) style.bg = "";
        else if (code >= 30 && code <= 37) style.fg = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"][code - 30];
        else if (code >= 90 && code <= 97) style.fg = ["bright-black", "bright-red", "bright-green", "bright-yellow", "bright-blue", "bright-magenta", "bright-cyan", "bright-white"][code - 90];
        else if (code >= 40 && code <= 47) style.bg = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"][code - 40];
        else if (code >= 100 && code <= 107) style.bg = ["bright-black", "bright-red", "bright-green", "bright-yellow", "bright-blue", "bright-magenta", "bright-cyan", "bright-white"][code - 100];
      }
    };
    for (const match of text.matchAll(sgr)) {
      appendSegment(text.slice(cursor, match.index));
      applySgr(match[1] ? match[1].split(";").map((part) => Number(part) || 0) : [0]);
      cursor = match.index + match[0].length;
    }
    appendSegment(text.slice(cursor));
  }

  function updateTerminalOutputFade(output) {
    if (!output) return;
    const overflow = output.scrollHeight - output.clientHeight;
    output.dataset.fadeTop = String(output.scrollTop > 1);
    output.dataset.fadeBottom = String(overflow - output.scrollTop > 1);
  }

  function terminalStatusText(activity) {
    if (!activity) return "";
    // File-read rows do not have a process exit code. The official renderer
    // closes them with the read summary, not a misleading "unknown" status.
    if (activity.kind === "read" && (activity.exitCode === null || activity.exitCode === undefined)) return "";
    if (activity.status === "inProgress") return "";
    if (activity.status === "interrupted") return t("已停止");
    if (activity.status === "failed" || activity.status === "declined") {
      return activity.exitCode === null || activity.exitCode === undefined
        ? t("退出码 未知")
        : t(`退出码 ${activity.exitCode}`);
    }
    if (activity.status === "completed") {
      if (activity.exitCode === 0) return t("成功");
      if (activity.exitCode !== null && activity.exitCode !== undefined) return t(`退出码 ${activity.exitCode}`);
      return t("退出码 未知");
    }
    return "";
  }

  function appendTerminalCheckIcon(parent) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.className.baseVal = "terminal-status-icon";
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("aria-hidden", "true");
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", "m3.5 8.2 2.7 2.7 6.3-6.3");
    svg.append(path);
    parent.append(svg);
  }

  function setTerminalActionIcon(button, kind = "copy") {
    if (!button) return;
    button.replaceChildren();
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("aria-hidden", "true");
    if (kind === "check") {
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", "m3.5 8.2 2.7 2.7 6.3-6.3");
      svg.append(path);
    } else if (kind === "collapse" || kind === "expand") {
      const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
      path.setAttribute("d", kind === "collapse" ? "m5 6 3 3 3-3" : "m5 10 3-3 3 3");
      svg.append(path);
    } else {
      const back = document.createElementNS("http://www.w3.org/2000/svg", "path");
      back.setAttribute("d", "M5.5 5.5V4c0-.8.7-1.5 1.5-1.5h5c.8 0 1.5.7 1.5 1.5v5c0 .8-.7 1.5-1.5 1.5h-1.5");
      const front = document.createElementNS("http://www.w3.org/2000/svg", "rect");
      front.setAttribute("x", "2.5");
      front.setAttribute("y", "5.5");
      front.setAttribute("width", "7.5");
      front.setAttribute("height", "7.5");
      front.setAttribute("rx", "1.2");
      svg.append(back, front);
    }
    button.append(svg);
  }

  function createTerminalAction(kind, title) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "terminal-action";
    button.title = t(title);
    button.setAttribute("aria-label", t(title));
    setTerminalActionIcon(button, kind);
    return button;
  }

  async function copyTerminalValue(value, button) {
    try {
      await navigator.clipboard?.writeText(String(value || ""));
      setTerminalActionIcon(button, "check");
      button.dataset.copied = "true";
      setTimeout(() => {
        if (!button.isConnected) return;
        setTerminalActionIcon(button, "copy");
        button.dataset.copied = "false";
      }, 1_500);
    } catch {
      button.dataset.copied = "false";
    }
  }

  function createReadPathIcon() {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.className.baseVal = "read-path-icon";
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("aria-hidden", "true");
    const folder = document.createElementNS("http://www.w3.org/2000/svg", "path");
    folder.setAttribute("d", "M2.5 4.5h3l1.2 1.4h6.8v6.2a1 1 0 0 1-1 1h-9a1 1 0 0 1-1-1z");
    const top = document.createElementNS("http://www.w3.org/2000/svg", "path");
    top.setAttribute("d", "M2.5 4.5v-1h3l1.1 1h5.9");
    svg.append(folder, top);
    return svg;
  }

  function renderReadBody(activity) {
    const body = activity?.body;
    if (!body) return;
    body.replaceChildren();
    body.classList.remove("terminal-body", "diff-body", "markdown-body");
    body.classList.add("read-body");
    const paths = String(activity.filePath || "").split("\n").map((value) => value.trim()).filter(Boolean);
    if (paths.length) {
      const list = document.createElement("div");
      list.className = "read-path-list";
      for (const path of paths) {
        const row = document.createElement("div");
        row.className = "read-path-row";
        row.append(createReadPathIcon());
        const value = document.createElement("span");
        value.textContent = path;
        value.title = path;
        row.append(value);
        list.append(row);
      }
      body.append(list);
    }
    const output = terminalOutputText(activity);
    if (output) {
      const outputBlock = document.createElement("pre");
      outputBlock.className = "read-output";
      outputBlock.textContent = output;
      body.append(outputBlock);
    }
    if (!paths.length && !output) {
      const empty = document.createElement("span");
      empty.className = "read-empty";
      empty.textContent = t(activity.status === "inProgress" ? "正在读取文件" : "读取完成");
      body.append(empty);
    }
  }

  function renderTerminalBody(activity) {
    const body = activity?.body;
    if (!body) return;
    if (activity.kind === "read" && !terminalCommandText(activity.command)) {
      renderReadBody(activity);
      return;
    }
    const previousOutput = body.querySelector(".terminal-output");
    const previousScrollTop = previousOutput?.scrollTop || 0;
    const previousScrollLeft = previousOutput?.scrollLeft || 0;
    const previousAtBottom = previousOutput
      ? previousOutput.scrollHeight - previousOutput.scrollTop - previousOutput.clientHeight <= 2
      : true;
    body.replaceChildren();
    body.classList.add("terminal-body");
    body.classList.remove("markdown-body", "diff-body");

    const shell = document.createElement("div");
    shell.className = "terminal-shell";
    const command = terminalCommandText(activity.command);
    const output = terminalOutputText(activity);

    const shellHeader = document.createElement("div");
    shellHeader.className = "terminal-shell-header";
    const shellLabel = document.createElement("div");
    shellLabel.className = "terminal-shell-label";
    shellLabel.textContent = activity.shellName || "Shell";
    if (activity.cwd) shellLabel.title = `cwd\\n${activity.cwd}`;
    // The official embedded shell uses a lightweight label row. The parent
    // activity disclosure owns collapse; command and output each expose their
    // own copy action on hover.
    shellHeader.append(shellLabel);
    shell.append(shellHeader);

    if (activity.kind === "read" && activity.filePath && !command) {
      const pathRow = document.createElement("div");
      pathRow.className = "terminal-file-path";
      const paths = activity.filePath.split("\n").filter(Boolean);
      pathRow.textContent = paths.length > 1 ? `${paths[0]} (+${paths.length - 1})` : paths[0];
      pathRow.title = activity.filePath;
      shell.append(pathRow);
    }

    if (command) {
      const commandRow = document.createElement("div");
      commandRow.className = "terminal-command-line";
      const prompt = document.createElement("span");
      prompt.className = "terminal-prompt";
      prompt.textContent = "$";
      const commandCode = document.createElement("code");
      commandCode.textContent = command;
      const commandChevron = document.createElementNS("http://www.w3.org/2000/svg", "svg");
      commandChevron.classList.add("terminal-command-chevron");
      commandChevron.setAttribute("viewBox", "0 0 16 16");
      commandChevron.setAttribute("aria-hidden", "true");
      const commandChevronPath = document.createElementNS("http://www.w3.org/2000/svg", "path");
      commandChevronPath.setAttribute("d", "m4.5 6 3.5 3.5L11.5 6");
      commandChevron.append(commandChevronPath);
      const commandExpanded = activity.commandExpanded === true
        || state.commandDisclosure.get(activity.key) === true;
      commandRow.dataset.expanded = String(commandExpanded);
      commandRow.setAttribute("role", "button");
      commandRow.setAttribute("tabindex", "0");
      commandRow.setAttribute("aria-expanded", String(commandExpanded));
      commandRow.setAttribute("aria-label", `$ ${command}`);
      const toggleCommand = (event) => {
        if (event.target.closest(".terminal-action")) return;
        event.preventDefault();
        const expanded = commandRow.dataset.expanded === "true";
        const next = !expanded;
        activity.commandExpanded = next;
        if (activity.key) state.commandDisclosure.set(activity.key, next);
        preserveTimelineAnchor(commandRow, 210);
        animateCommandRow(commandRow, next);
        if (next) scheduleTimelineReveal(commandRow, 190);
      };
      commandRow.addEventListener("click", toggleCommand);
      commandRow.addEventListener("keydown", (event) => {
        if (event.key === "Enter" || event.key === " ") toggleCommand(event);
      });
      const copyCommand = createTerminalAction("copy", "复制命令");
      copyCommand.classList.add("terminal-command-action");
      copyCommand.addEventListener("click", (event) => {
        event.stopPropagation();
        copyTerminalValue(command, copyCommand);
      });
      commandRow.append(prompt, commandCode, commandChevron, copyCommand);
      shell.append(commandRow);
    }

    const outputWrap = document.createElement("div");
    outputWrap.className = "terminal-output-wrap";
    if (output) {
      const outputBlock = document.createElement("pre");
      outputBlock.className = "terminal-output";
      const outputContent = document.createElement("div");
      outputContent.className = "terminal-output-content";
      renderTerminalOutput(outputContent, output);
      outputBlock.append(outputContent);
      // Activity deltas rebuild the lightweight DOM. Preserve the reader's
      // position, while still following the tail when they were already at
      // the bottom of the terminal stream.
      outputBlock.scrollLeft = previousScrollLeft;
      requestAnimationFrame(() => {
        if (previousAtBottom) outputBlock.scrollTop = outputBlock.scrollHeight;
        else outputBlock.scrollTop = previousScrollTop;
        outputBlock.scrollLeft = previousScrollLeft;
        updateTerminalOutputFade(outputBlock);
      });
      outputBlock.addEventListener("scroll", () => updateTerminalOutputFade(outputBlock), { passive: true });
      const copyOutput = createTerminalAction("copy", "复制输出");
      copyOutput.classList.add("terminal-output-action");
      copyOutput.addEventListener("click", (event) => {
        event.stopPropagation();
        copyTerminalValue(output, copyOutput);
      });
      outputWrap.append(outputBlock, copyOutput);
    } else if (activity.status !== "inProgress") {
      const outputBlock = document.createElement("pre");
      outputBlock.className = "terminal-output terminal-output-empty";
      const outputContent = document.createElement("div");
      outputContent.className = "terminal-output-content terminal-no-output";
      outputContent.textContent = t("无输出");
      outputBlock.append(outputContent);
      outputWrap.append(outputBlock);
    }
    shell.append(outputWrap);

    const statusText = terminalStatusText(activity);
    const footer = document.createElement("div");
    footer.className = "terminal-footer";
    footer.dataset.status = activity.status;
    if (statusText) {
      const status = document.createElement("span");
      status.className = "terminal-status";
      if (activity.status === "completed" && activity.exitCode === 0) appendTerminalCheckIcon(status);
      status.append(document.createTextNode(statusText));
      footer.append(status);
    }
    shell.append(footer);
    body.append(shell);
  }

  function renderActivityText(activity) {
    const text = activityText(activity);
    const visibleText = text || t(isRunningActivity(activity) ? "等待输出…" : activityStatusLabel(activity.status));
    if (activity.kind === "tool" || activity.kind === "read") renderTerminalBody(activity);
    else if (activity.kind === "subagent") renderSubagentBody(activity);
    else renderMessageBody(activity.body, visibleText, activity.role, "activity", activity.messageKind);
    activity.article.dataset.rawText = text;
  }

  function setActivityContent(activity, header, output, append = false) {
    if (!activity) return;
    if (header !== undefined && header !== null && String(header)) activity.headerText = String(header);
    if (output !== undefined && output !== null) {
      activity.outputText = append ? `${activity.outputText}${String(output)}` : String(output);
    }
    renderActivityText(activity);
  }

  function activityElapsed(activity) {
    if (activity.durationMs !== null) return Math.max(0, activity.durationMs);
    if (!activity.startedAt) return null;
    // A completed legacy row without an end marker has an unknown duration;
    // showing the current wall clock makes old work appear to keep running.
    if (!isRunningActivity(activity) && !activity.finishedAt) return null;
    const end = activity.finishedAt || Date.now();
    return Math.max(0, end - activity.startedAt);
  }

  function refreshActivity(activity) {
    if (!activity?.summary) return;
    const elapsed = activityElapsed(activity);
    // The official activity rows only show an item duration when the owner
    // supplied one. A timestamp interval inferred while hydrating history is
    // useful for the turn clock, but should not become a misleading per-row
    // wall-clock label.
    const duration = activity.durationExplicit === true ? elapsedDuration(elapsed) : "";
    const label = t(activity.label || "执行步骤");
    const setSummary = (value) => setActivitySummary(activity.summary, value, activity.kind);
    // Recompute the summary in the active locale. Dynamic command/path/name
    // values are appended as raw strings by historyActivitySummary().
    if (activity.kind === "subagent") {
      const name = activity.displayName || activity.label || t("子代理");
      const statusText = activity.status === "inProgress" ? t("已开始工作")
        : activity.status === "completed" ? t("已完成")
          : activity.status === "failed" ? t("失败")
            : activity.status === "interrupted" ? t("已中断") : "";
      setSubagentSummary(activity.summary, name, statusText);
    } else {
      const localizedSummary = historyActivitySummary(activity, activity.kind, activity.status, duration);
      if (localizedSummary) setSummary(localizedSummary);
      else if (activity.status === "inProgress") setSummary(label);
      else if (activity.status === "failed") setSummary(`${label} · ${t("失败")}`);
      else if (activity.status === "interrupted") setSummary(`${label} · ${t("已中断")}`);
      else setSummary(label);
    }
    activity.article.dataset.status = activity.status;
  }

  function finishActivity(activity, status = "completed", durationMs, finishedAt) {
    if (!activity) return;
    // `updateLiveActivity` may create a short-lived row before the host emits
    // an item lifecycle event. It is useful while the turn is live, but the
    // official transcript does not retain a blank "思考/编辑/读取" item once
    // the turn ends. A concrete lifecycle row (or a row that received real
    // output) has already cleared `statusOnly` and follows the normal path.
    if (activity.statusOnly === true && activity.concrete !== true) {
      retireActivity(activity);
      return;
    }
    activity.status = normalizeActivityStatus(status, "completed");
    activity.finishedAt = timestampMs(finishedAt) || Date.now();
    const explicitDuration = finiteNumber(durationMs);
    activity.durationExplicit = explicitDuration !== null;
    const inferredDuration = activity.startedAt === null
      ? null
      : Math.max(0, activity.finishedAt - activity.startedAt);
    activity.durationMs = explicitDuration === null
      ? inferredDuration
      : Math.max(0, explicitDuration);
    activity.article.classList.remove("streaming");
    renderActivityText(activity);
    refreshActivity(activity);
    if (activity.details && (activity.kind === "reasoning" || activity.kind === "plan" || activity.kind === "subagent")) {
      setDetailsExpanded(activity.details, false);
    }
    if (state.activeAssistantActivityKey === activity.key) {
      state.activeAssistantBody = null;
      state.activeAssistantStream = null;
      state.activeAssistantText = "";
      state.activeAssistantActivityKey = null;
    }
    stopActivityTimerIfIdle();
  }

  function latestRunningActivity(kind, context = {}) {
    const itemId = context.itemId === undefined || context.itemId === null ? "" : String(context.itemId);
    const turnId = context.turnId || state.turnId || "";
    const activities = [...state.activities.values()].reverse();
    if (itemId) {
      const exact = activities.find((activity) => activity.itemId === itemId && isRunningActivity(activity));
      if (exact) return exact;
    }
    const acceptedKinds = kind === "reasoning" ? new Set(["reasoning", "plan"]) : new Set([kind]);
    return activities.find((activity) => isRunningActivity(activity)
      && acceptedKinds.has(activity.kind)
      && (!turnId || !activity.turnId || activity.turnId === turnId)) || null;
  }

  function appendActivityChunk(activity, text) {
    if (!activity || !text) return;
    // A stream chunk is evidence of a real work item, even when the host
    // omitted its item id. It should not be mistaken for a status-only row at
    // turn completion.
    activity.concrete = true;
    activity.statusOnly = false;
    activity.status = "inProgress";
    setActivityContent(activity, undefined, text, true);
    activity.article.classList.add("streaming");
    if ((activity.kind === "reasoning" || activity.kind === "plan" || activity.kind === "commentary") && activity.details) {
      // Reasoning is the one activity the official transcript expands while
      // it is streaming; completed reasoning collapses again in finishActivity.
      setDetailsExpanded(activity.details, true);
    }
    refreshActivity(activity);
    ensureActivityTimer();
  }

  function handleItemLifecycle(phase, payload) {
    const params = eventParams(payload);
    const item = itemFromPayload(payload);
    const kind = activityKindForItem(item);
    if (!kind || kind === "user" || kind === "assistant") return false;
    const itemId = item.id ?? params.itemId ?? payload?.itemId;
    const duration = finiteNumber(item.durationMs, params.durationMs);
    const finishedAt = timestampMs(item.completedAtMs, item.finishedAtMs, params.completedAtMs, params.emittedAtMs);
    const startedAt = timestampMs(item.startedAtMs, item.startedAt, params.startedAtMs)
      || (duration !== null && finishedAt ? finishedAt - duration : null);
    const fallbackStatus = phase === "completed" ? "completed" : "inProgress";
    const activity = ensureActivity(activityKey(payload, item, kind), {
      kind,
      label: activityLabelForItem(item, kind),
      command: kind === "tool" || kind === "read" ? terminalCommandText(commandText(item)) : "",
      filePath: kind === "read" ? readPathList(item).join("\n") : "",
      cwd: (kind === "tool" || kind === "read") && typeof item.cwd === "string" ? item.cwd : "",
      shellName: (kind === "tool" || kind === "read") && typeof item.shellName === "string" ? item.shellName : "Shell",
      agentThreadId: firstString(item.agentThreadId, item.childThreadId, item.threadId),
      displayName: firstString(item.displayName, item.agentNickname, item.agentName, item.agentPath),
      objective: firstString(item.objective, item.prompt, item.statusMessage, item.message),
      activityKind: firstString(item.activityKind, item.kind),
      displayStatus: firstString(item.displayStatus, item.status),
      model: firstString(item.model, item.modelId),
      action: firstString(item.action, item.tool),
      prompt: item.prompt === null ? "" : firstString(item.prompt),
      senderThreadId: firstString(item.senderThreadId),
      receiverThreadIds: Array.isArray(item.receiverThreadIds)
        ? item.receiverThreadIds.map(String)
        : Array.isArray(item.receiverThreads) ? item.receiverThreads.map(String) : [],
      agentsStates: isRecord(item.agentsStates) ? item.agentsStates : {},
      canInteract: item.canInteract !== false,
      exitCode: kind === "tool" || kind === "read" ? finiteNumber(item.exitCode, item.exit_code) : undefined,
      itemId,
      threadId: eventThreadId(payload),
      turnId: eventTurnId(payload),
      startedAt,
      status: normalizeActivityStatus(item.status, fallbackStatus),
      concrete: true,
      anonymous: itemId === undefined || itemId === null,
    });
    if (!activity) return true;
    if (kind === "tool" || kind === "read") {
      const command = terminalCommandText(commandText(item));
      if (command) activity.command = command;
      if (kind === "read") activity.filePath = readPathList(item).join("\n") || activity.filePath;
      if (typeof item.cwd === "string") activity.cwd = item.cwd;
      if (typeof item.shellName === "string" && item.shellName) activity.shellName = item.shellName;
      const exitCode = finiteNumber(item.exitCode, item.exit_code);
      if (exitCode !== null) activity.exitCode = exitCode;
    }
    if (kind === "subagent") {
      activity.agentThreadId = firstString(item.agentThreadId, item.childThreadId, item.threadId, activity.agentThreadId);
      activity.displayName = firstString(item.displayName, item.agentNickname, item.agentName, item.agentPath, activity.displayName);
      activity.objective = firstString(item.objective, item.prompt, item.statusMessage, item.message, activity.objective);
      activity.activityKind = firstString(item.activityKind, item.kind, activity.activityKind);
      activity.displayStatus = firstString(item.displayStatus, item.status, activity.displayStatus);
      activity.model = firstString(item.model, item.modelId, activity.model);
      activity.action = firstString(item.action, item.tool, activity.action);
      activity.prompt = item.prompt === null ? "" : firstString(item.prompt, activity.prompt);
      activity.senderThreadId = firstString(item.senderThreadId, activity.senderThreadId);
      if (Array.isArray(item.receiverThreadIds)) activity.receiverThreadIds = item.receiverThreadIds.map(String);
      else if (Array.isArray(item.receiverThreads)) activity.receiverThreadIds = item.receiverThreads.map(String);
      if (isRecord(item.agentsStates)) activity.agentsStates = item.agentsStates;
      if (item.canInteract !== undefined) activity.canInteract = item.canInteract !== false;
      if (activity.agentThreadId) activity.article.dataset.agentThreadId = activity.agentThreadId;
      else delete activity.article.dataset.agentThreadId;
    }
    const header = activityHeader(item, kind);
    const output = activityOutput(item, kind);
    if (header || output) setActivityContent(activity, header, output);
    if (phase === "completed") {
      const status = normalizeActivityStatus(item.status, item.error ? "failed" : "completed");
      finishActivity(activity, status, duration, finishedAt);
    } else {
      activity.status = normalizeActivityStatus(item.status, "inProgress");
      refreshActivity(activity);
    }
    return true;
  }

  function handlePlanUpdate(payload) {
    const params = eventParams(payload);
    const steps = Array.isArray(params.plan) ? params.plan : Array.isArray(params.steps) ? params.steps : null;
    const text = planText(steps || params.text || params);
    if (!text) return false;
    const threadId = eventThreadId(payload) || state.threadId || "thread";
    const turnId = eventTurnId(payload) || state.turnId || "turn";
    const activity = ensureActivity(`plan:${threadId}:${turnId}`, {
      kind: "plan",
      label: "计划",
      threadId,
      turnId,
      status: "inProgress",
      concrete: true,
    });
    setActivityContent(activity, "", text);
    const statuses = (steps || []).map((step) => normalizeActivityStatus(step?.status, "pending"));
    if (statuses.length && statuses.every((status) => status === "completed")) finishActivity(activity, "completed");
    else refreshActivity(activity);
    return true;
  }

  function handleDiffUpdate(payload) {
    const params = eventParams(payload);
    const diff = fileChangesText(params.changes || params.diff || params.patch || params.delta || params.text || params.output);
    if (!diff) return false;
    const threadId = eventThreadId(payload) || state.threadId || "thread";
    const turnId = eventTurnId(payload) || state.turnId || "turn";
    const activity = ensureActivity(`diff:${threadId}:${turnId}`, {
      kind: "edit",
      label: "文件变更",
      threadId,
      turnId,
      status: "inProgress",
      concrete: true,
    });
    setActivityContent(activity, "", diff);
    refreshActivity(activity);
    return true;
  }

  function finishActivitiesForTurn(turnId, status) {
    for (const activity of state.activities.values()) {
      if (!isRunningActivity(activity)) continue;
      if (turnId && activity.turnId && activity.turnId !== turnId) continue;
      finishActivity(activity, status);
    }
  }

  function turnStatusLabel(status) {
    if (status === "waiting") return t("等待授权");
    if (status === "generating") return t("正在生成");
    if (status === "interrupted") return t("已中断");
    if (status === "failed") return t("失败");
    if (status === "completed") return t("已完成");
    return t("正在工作");
  }

  function statusActivityLabel(activity, flags = []) {
    const normalized = String(activity || "").replace(/[\s-]+/g, "_").toLowerCase();
    if (normalized === "waiting_approval" || normalized === "waiting_for_approval" || flags.some((flag) => /approval|permission/.test(String(flag)))) return t("等待授权");
    if (normalized === "waiting_input" || normalized === "waiting_for_user_input" || flags.some((flag) => /user.?input/.test(String(flag)))) return t("正在等待你的回答");
    if (normalized === "thinking" || normalized === "reasoning") return t("正在思考");
    if (normalized === "editing" || normalized === "edit") return t("正在编辑文件");
    if (normalized === "reading" || normalized === "reading_file" || normalized === "file_read") return t("正在读取文件");
    if (normalized === "running" || normalized === "running_command" || normalized === "tool") return t("正在运行命令");
    if (normalized === "searching" || normalized === "searching_web") return t("正在搜索网页");
    if (normalized === "responding" || normalized === "generating") return t("正在生成");
    if (normalized === "failed") return t("执行失败");
    if (normalized === "interrupted") return t("已中断");
    if (normalized === "completed") return t("已完成");
    return normalized && normalized !== "idle" ? t("处理中") : "";
  }

  function updateLiveActivity(activity, startedAt, durationMs, flags = [], turnId = "") {
    const element = $("liveActivity");
    if (!element) return;
    const normalized = String(activity || "idle").replace(/[\s-]+/g, "_").toLowerCase();
    const terminal = ["idle", "ready", "completed", "failed", "interrupted", "cancelled", "canceled"].includes(normalized);
    state.currentActivity = normalized;
    state.currentActivityStartedAt = timestampMs(startedAt) || state.currentActivityStartedAt;
    state.currentActivityDurationMs = finiteNumber(durationMs);
    state.currentActivityTurnId = turnId || state.currentActivityTurnId || "";
    if (terminal) {
      const live = state.liveActivityKey ? state.activities.get(state.liveActivityKey) : null;
      // Concrete lifecycle rows are finalized by item/turn completion events.
      // A status transition may only close an anonymous streaming fallback;
      // closing a concrete row here can race the owner snapshot and erase its
      // command/edit label before the real completion payload arrives.
      if (live?.anonymous && isRunningActivity(live)) {
        finishActivity(live, normalized === "failed" ? "failed" : normalized === "interrupted" ? "interrupted" : "completed", durationMs);
      }
      state.liveActivityKey = null;
      element.hidden = true;
      element.dataset.activity = normalized;
      element.dataset.active = "false";
      return;
    }
    const label = statusActivityLabel(normalized, flags);
    if (!label) {
      element.hidden = true;
      return;
    }
    // The live status node remains available to assistive technology, while
    // the visible transcript is the source of truth for work-in-progress rows.
    element.hidden = false;
    element.dataset.activity = normalized;
    element.dataset.active = "true";
    const labelElement = element.querySelector(".activity-label");
    const elapsedElement = element.querySelector(".activity-elapsed");
    const activeTranscript = latestRunningActivity(
      normalized === "thinking" || normalized === "reasoning" ? "reasoning"
        : normalized === "editing" || normalized === "edit" ? "edit"
          : normalized === "reading" || normalized === "reading_file" || normalized === "file_read" ? "read"
          : normalized === "searching" || normalized === "searching_web" ? "tool"
            : normalized === "running" || normalized === "running_command" || normalized === "tool" ? "tool" : "",
      { turnId: turnId || state.turnId || "" },
    );
    let visibleLabel = label;
    if (activeTranscript) {
      if (activeTranscript.kind === "tool" && activeTranscript.command) {
        visibleLabel = uiWithRaw("正在运行 ", "Running ", terminalCommandText(activeTranscript.command));
      } else if (activeTranscript.kind === "edit") visibleLabel = t("正在编辑文件");
      else if (activeTranscript.kind === "read") visibleLabel = t("正在读取文件");
      else if (activeTranscript.kind === "reasoning") visibleLabel = t("正在思考");
      else if (activeTranscript.label) visibleLabel = activeTranscript.label;
    }
    if (labelElement) labelElement.textContent = visibleLabel;
    const elapsed = state.currentActivityStartedAt === null
      ? state.currentActivityDurationMs
      : Math.max(0, Date.now() - state.currentActivityStartedAt);
    if (elapsedElement) elapsedElement.textContent = elapsedDuration(elapsed);

    // Status snapshots are projections of the current turn, not work items.
    // Bind to an existing concrete lifecycle row when possible. For the few
    // builds that publish a status before its item, the fallback below creates
    // a transient, explicitly `statusOnly` row that is removed at completion;
    // waiting approvals and ordinary status ticks never become history items.
    const effectiveTurnId = turnId || state.turnId || "";
    const transcriptKind = normalized === "thinking" || normalized === "reasoning"
      ? "reasoning"
      : normalized === "editing" || normalized === "edit"
        ? "edit"
        : normalized === "reading" || normalized === "reading_file" || normalized === "file_read"
          ? "read"
          : normalized === "running" || normalized === "running_command" || normalized === "tool"
            ? "tool"
            : normalized === "searching" || normalized === "searching_web"
              ? "tool"
              : "";
    let transcript = transcriptKind
      ? latestRunningActivity(transcriptKind, { turnId: effectiveTurnId })
      : null;
    if (!transcript && effectiveTurnId && (transcriptKind === "reasoning" || transcriptKind === "edit" || transcriptKind === "read")) {
      // A few official builds publish the turn activity before the first
      // reasoning/diff item. Materialize one stable anonymous row so the
      // reader sees "正在思考"/"正在编辑文件" immediately; a later concrete
      // lifecycle item is merged into the same visual stream by kind/turn.
      const key = `status:${state.threadId || "thread"}:${effectiveTurnId}:${transcriptKind}`;
      transcript = ensureActivity(key, {
        kind: transcriptKind,
        label: transcriptKind === "reasoning" ? "思考" : transcriptKind === "edit" ? "编辑文件" : "读取文件",
        threadId: state.threadId,
        turnId: effectiveTurnId,
        startedAt: state.currentActivityStartedAt || state.turnStartedAt,
        status: "inProgress",
        anonymous: true,
        statusOnly: true,
        open: false,
      });
    }
    if (transcript) {
      state.liveActivityKey = transcript.key;
      // Keep the concrete item label (command, tool name, or edit summary)
      // supplied by its lifecycle event. The global status must not overwrite
      // it with a generic "正在运行命令"/"正在思考" label.
      transcript.status = "inProgress";
      if (state.currentActivityStartedAt && !transcript.startedAt) transcript.startedAt = state.currentActivityStartedAt;
      refreshActivity(transcript);
      if (effectiveTurnId) ensureLiveTurnDivider(effectiveTurnId);
    } else {
      state.liveActivityKey = null;
    }
  }

  function refreshTurnClock() {
    if (state.turnStartedAt === null) return;
    const elapsed = Math.max(0, Date.now() - state.turnStartedAt);
    const workElapsed = state.turnWorkStartedAt === null
      ? elapsed
      : Math.max(0, Date.now() - state.turnWorkStartedAt);
    const activity = state.currentActivity && state.currentActivity !== "idle"
      ? statusActivityLabel(state.currentActivity)
      : turnStatusLabel(state.turnStatus);
    const visibleElapsed = elapsedDuration(workElapsed);
    setConversationStatus(visibleElapsed ? `${activity} · ${visibleElapsed}` : activity, state.turnStatus === "waiting" ? "warning" : "active");
    const divider = state.turnDividers.get(state.turnId)
      || [...$("output").querySelectorAll(".turn-divider")]
        .find((entry) => entry.dataset.turnId === state.turnId);
    if (divider?.dataset.status === "inProgress") {
      const label = divider.querySelector(".turn-divider-label");
      if (label) label.textContent = turnDividerLabel("inProgress", workElapsed);
    }
    updateLiveActivity(state.currentActivity || "running", state.currentActivityStartedAt || state.turnStartedAt, null, [], state.currentActivityTurnId || state.turnId);
  }

  function startTurnClock(turnId, startedAt, elapsedMs) {
    const nextTurnId = turnId || state.turnId || "";
    const explicitStartedAt = timestampMs(startedAt);
    const explicitElapsed = finiteNumber(elapsedMs);
    if (nextTurnId && state.turnId && nextTurnId !== state.turnId) {
      state.turnStartedAt = null;
      state.turnWorkStartedAt = null;
      state.finalAssistantStartedAt = null;
      state.workedDurationMs = null;
      state.currentActivityStartedAt = null;
      state.currentActivityDurationMs = null;
      state.currentActivity = "running";
    }
    if (nextTurnId) state.turnId = nextTurnId;
    if (state.turnStartedAt === null) {
      state.turnStartedAt = explicitStartedAt
        || (explicitElapsed !== null ? Date.now() - Math.max(0, explicitElapsed) : Date.now());
    }
    state.turnStatus = "active";
    state.lastTurnDurationMs = null;
    state.lastWorkedDurationMs = null;
    if (state.turnWorkStartedAt === null) state.turnWorkStartedAt = state.turnStartedAt;
    const output = $("output");
    if (output && [...output.querySelectorAll(".message.activity")]
      .some((article) => article.dataset.turnId === nextTurnId)) ensureLiveTurnDivider(nextTurnId);
    refreshTurnClock();
    ensureActivityTimer();
  }

  function stopTurnClock(status = "completed", durationMs, finishedAt, workedDurationMs) {
    // Some completion envelopes do not carry an item lifecycle event and do
    // not leave `liveActivityKey` pointing at the transient status row. Sweep
    // those rows here as a final safety net before clearing the turn clock.
    retireStatusOnlyActivities(state.turnId);
    const end = timestampMs(finishedAt) || Date.now();
    const explicitDuration = finiteNumber(durationMs);
    const elapsed = explicitDuration !== null
      ? Math.max(0, explicitDuration)
      : state.turnStartedAt === null ? null : Math.max(0, end - state.turnStartedAt);
    // A hydrated terminal snapshot can arrive immediately after metadata. In
    // that case the metadata duration is authoritative even though the
    // terminal status envelope does not repeat it.
    const authoritativeWorkedDuration = finiteNumber(
      workedDurationMs,
      state.workedDurationMs,
      state.lastWorkedDurationMs,
    );
    const worked = workedDurationFor({
      workedDurationMs: authoritativeWorkedDuration,
      firstTurnWorkItemStartedAtMs: state.turnWorkStartedAt,
      finalAssistantStartedAtMs: state.finalAssistantStartedAt,
      completedAtMs: end,
    }, state.turnWorkStartedAt === null ? elapsed : Math.max(0, end - state.turnWorkStartedAt));
    state.turnStartedAt = null;
    state.turnWorkStartedAt = null;
    state.turnStatus = status;
    state.lastTurnDurationMs = elapsed;
    state.lastWorkedDurationMs = worked;
    state.workedDurationMs = worked;
    const duration = elapsedDuration(worked ?? elapsed);
    setConversationStatus(`${turnStatusLabel(status)}${duration ? ` · ${duration}` : ""}`, status === "completed" ? "ready" : "warning");
    state.currentActivity = status;
    state.currentActivityStartedAt = null;
    state.currentActivityDurationMs = worked ?? elapsed;
    updateLiveActivity(status, null, worked ?? elapsed, [], state.turnId);
    stopActivityTimerIfIdle();
  }

  function turnStatusFromValue(value, fallback = "active") {
    const normalized = normalizeActivityStatus(value, fallback);
    if (normalized === "inProgress") return "active";
    if (normalized === "declined" || normalized === "interrupted") return "interrupted";
    return normalized;
  }

  function applyStatusSnapshot(payload, options = {}) {
    const status = isRecord(payload?.status) ? payload.status : {};
    const metadata = isRecord(payload?.metadata) ? payload.metadata : {};
    const rawTurnStatus = payload?.turnStatus ?? status.turnStatus ?? metadata.turnStatus ?? payload?.state;
    const activity = String(payload?.activity ?? status.activity ?? metadata.activity ?? "").toLowerCase();
    const flags = [
      ...(Array.isArray(payload?.activeFlags) ? payload.activeFlags : []),
      ...(Array.isArray(status.activeFlags) ? status.activeFlags : []),
      ...(Array.isArray(metadata.activeFlags) ? metadata.activeFlags : []),
    ].map((flag) => String(flag).toLowerCase());
    const turnId = payload?.turnId || state.turnId || "";
    const projectedWorkedDuration = workedDurationFor(payload,
      workedDurationFor(status, workedDurationFor(metadata, null)));
    const projectedWorkStart = timestampMs(
      payload?.firstTurnWorkItemStartedAtMs,
      payload?.workStartedAtMs,
      status.firstTurnWorkItemStartedAtMs,
      status.workStartedAtMs,
      metadata.firstTurnWorkItemStartedAtMs,
      metadata.workStartedAtMs,
    );
    const projectedFinalAssistantStart = timestampMs(
      payload?.finalAssistantStartedAtMs,
      status.finalAssistantStartedAtMs,
      metadata.finalAssistantStartedAtMs,
    );
    if (projectedWorkStart !== null) state.turnWorkStartedAt = projectedWorkStart;
    if (projectedFinalAssistantStart !== null) state.finalAssistantStartedAt = projectedFinalAssistantStart;
    if (projectedWorkedDuration !== null) state.workedDurationMs = projectedWorkedDuration;
    const rawNormalized = String(rawTurnStatus || "").replace(/[\s-]+/g, "_").toLowerCase();
    const normalized = turnStatusFromValue(rawTurnStatus || (turnId ? "active" : "idle"), turnId ? "active" : "idle");
    const terminal = ["completed", "complete", "done", "failed", "error", "interrupted", "cancelled", "canceled"].includes(rawNormalized)
      || ["completed", "failed", "interrupted"].includes(normalized);
    const effectiveActivity = activity || (flags.some((flag) => /approval|permission/.test(flag)) ? "waiting_approval" : turnId ? "running" : normalized);
    const explicitlyActive = !terminal && (Boolean(turnId)
      || ["active", "running", "working", "inprogress", "thinking", "reasoning", "editing", "edit", "reading", "readingfile", "fileread", "searching", "responding", "generating"].includes(activity.replace(/[\s_-]+/g, ""))
      || normalized === "active");
    if (explicitlyActive) {
      startTurnClock(
        turnId,
        payload?.startedAtMs ?? status.startedAtMs ?? metadata.startedAtMs,
        payload?.elapsedMs ?? status.elapsedMs ?? metadata.elapsedMs,
      );
      state.turnStatus = flags.some((flag) => /approval|permission|input|waiting/.test(flag)) ? "waiting" : "active";
      state.currentActivity = effectiveActivity;
      updateLiveActivity(effectiveActivity, payload?.startedAtMs ?? status.startedAtMs ?? metadata.startedAtMs ?? state.turnStartedAt, payload?.durationMs ?? status.durationMs ?? metadata.durationMs, flags, turnId);
      refreshTurnClock();
      return;
    }
    if (options.allowTerminal === false) return;
    const duration = payload?.durationMs ?? status.durationMs ?? metadata.durationMs;
    if (["completed", "interrupted", "failed"].includes(normalized) || terminal) {
      stopTurnClock(normalized, duration, payload?.completedAtMs ?? status.completedAtMs ?? metadata.completedAtMs, projectedWorkedDuration);
    }
    else if (state.turnStartedAt === null && options.showIdle !== false) setConversationStatus("ready");
  }

  function snapshotStatusProjection(snapshot = {}, appState = {}) {
    const snapshotRecord = isRecord(snapshot) ? snapshot : {};
    const stateRecord = isRecord(appState) ? appState : {};
    const statusRecord = [
      snapshotRecord.executionStatus,
      snapshotRecord.status,
      stateRecord.executionStatus,
      stateRecord.status,
    ].find(isRecord) || {};
    const rawTurnStatus = firstDefined(
      snapshotRecord.turnStatus,
      statusRecord.turnStatus,
      statusRecord.status,
      stateRecord.turnStatus,
      typeof stateRecord.status === "string" ? stateRecord.status : undefined,
    );
    const rawActivity = firstString(
      snapshotRecord.activity,
      statusRecord.activity,
      stateRecord.activity,
    ).toLowerCase();
    const rawNormalized = String(rawTurnStatus || "").replace(/[\s-]+/g, "_").toLowerCase();
    const fallbackStatus = rawActivity === "completed" || rawActivity === "complete" || rawActivity === "done"
      ? "completed"
      : rawActivity === "failed" || rawActivity === "error"
        ? "failed"
        : rawActivity === "interrupted" || rawActivity === "cancelled" || rawActivity === "canceled"
          ? "interrupted"
          : snapshotRecord.turnId || stateRecord.activeTurnId ? "active" : "idle";
    const normalized = turnStatusFromValue(rawTurnStatus ?? fallbackStatus, fallbackStatus);
    const terminal = ["completed", "complete", "done", "failed", "error", "interrupted", "cancelled", "canceled"].includes(rawNormalized)
      || ["completed", "failed", "interrupted"].includes(normalized)
      || ["completed", "failed", "interrupted"].includes(rawActivity);
    const activeActivity = ["active", "running", "working", "inprogress", "thinking", "reasoning", "editing", "edit", "reading", "reading_file", "file_read", "searching", "searching_web", "responding", "generating"].includes(rawActivity.replace(/[\s-]+/g, "_"));
    const explicitTurnId = snapshotRecord.turnId !== undefined
      ? snapshotRecord.turnId
      : stateRecord.activeTurnId;
    return {
      normalized,
      terminal,
      hasActiveTurn: Boolean(explicitTurnId) || activeActivity || normalized === "active" || normalized === "waiting",
      durationMs: finiteNumber(
        snapshotRecord.durationMs,
        snapshotRecord.elapsedMs,
        statusRecord.durationMs,
        statusRecord.elapsedMs,
        stateRecord.durationMs,
        stateRecord.elapsedMs,
      ),
      workedDurationMs: workedDurationFor(snapshotRecord,
        workedDurationFor(statusRecord, workedDurationFor(stateRecord, null))),
      firstTurnWorkItemStartedAtMs: timestampMs(
        snapshotRecord.firstTurnWorkItemStartedAtMs,
        snapshotRecord.workStartedAtMs,
        statusRecord.firstTurnWorkItemStartedAtMs,
        statusRecord.workStartedAtMs,
        stateRecord.firstTurnWorkItemStartedAtMs,
        stateRecord.workStartedAtMs,
      ),
      finalAssistantStartedAtMs: timestampMs(
        snapshotRecord.finalAssistantStartedAtMs,
        statusRecord.finalAssistantStartedAtMs,
        stateRecord.finalAssistantStartedAtMs,
      ),
      completedAtMs: timestampMs(snapshotRecord.completedAtMs, statusRecord.completedAtMs, stateRecord.completedAtMs),
    };
  }

  // A late authoritative snapshot can arrive after replayed lifecycle events.
  // Once it says there is no active turn, clear only the transient projection;
  // hydrated history remains intact and a locally queued user message is kept.
  function reconcileSnapshotTerminalState(snapshot = {}, appState = {}) {
    const projection = snapshotStatusProjection(snapshot, appState);
    if (!projection.terminal || projection.hasActiveTurn || state.pendingUserText) return;
    const hadTransientTurn = Boolean(
      state.turnId
      || state.turnStartedAt !== null
      || state.currentActivity === "active"
      || state.currentActivity === "running"
      || state.currentActivity === "working"
      || state.currentActivity === "thinking"
      || state.currentActivity === "editing"
      || state.currentActivity === "generating"
      || [...state.activities.values()].some(isRunningActivity),
    );
    if (!hadTransientTurn) return;
    const finishedTurnId = state.turnId;
    finishAssistantStream();
    finishActivitiesForTurn(finishedTurnId, projection.normalized);
    stopTurnClock(projection.normalized, projection.durationMs, projection.completedAtMs, projection.workedDurationMs);
    state.turnId = "";
    state.currentActivityTurnId = "";
    state.currentActivityStartedAt = null;
    state.currentActivityDurationMs = projection.durationMs;
    updateIds();
  }

  function appendFileChangeChunk(payload, text) {
    if (!text) return false;
    const params = eventParams(payload);
    const threadId = eventThreadId(payload) || state.threadId || "thread";
    const turnId = eventTurnId(payload) || state.turnId || "turn";
    const itemId = params.itemId ?? payload?.itemId;
    let activity = latestRunningActivity("edit", { itemId, turnId });
    if (!activity) {
      const key = itemId === undefined
        ? `diff:${threadId}:${turnId}`
        : `${threadId}:${turnId}:edit:${typeof itemId}:${String(itemId)}`;
      activity = ensureActivity(key, {
        kind: "edit",
        label: "编辑文件",
        itemId,
        threadId,
        turnId,
        status: "inProgress",
        concrete: true,
      });
    }
    appendActivityChunk(activity, text);
    return true;
  }

  function refreshElapsedDisplays() {
    refreshTurnClock();
    for (const activity of state.activities.values()) if (isRunningActivity(activity)) refreshActivity(activity);
    refreshSubagentElapsed();
    if (state.currentActivity && state.currentActivity !== "idle") {
      updateLiveActivity(
        state.currentActivity,
        state.currentActivityStartedAt || state.turnStartedAt,
        state.currentActivityDurationMs,
        [],
        state.currentActivityTurnId || state.turnId,
      );
    }
    stopActivityTimerIfIdle();
  }

  function ensureActivityTimer() {
    if (state.activityTimer !== null) return;
    state.activityTimer = setInterval(refreshElapsedDisplays, 250);
  }

  function stopActivityTimerIfIdle() {
    if (state.turnStartedAt !== null || [...state.activities.values()].some(isRunningActivity)) return;
    if (state.activityTimer !== null) clearInterval(state.activityTimer);
    state.activityTimer = null;
  }

  function renderEmptyOutput() {
    const output = $("output");
    output.replaceChildren();
    output.dataset.outputTail = "";
    state.activeAssistantBody = null;
    state.activeAssistantStream = null;
    state.activeAssistantText = "";
    state.activeAssistantActivityKey = null;
    state.activities.clear();
    state.turnDividers.clear();
    state.liveActivityKey = null;
    state.pendingUserArticle = null;
    state.lastRenderedDateKey = "";
    state.lastRenderedTimestamp = null;
    state.lastRenderedRole = "";
    state.hasRenderedUser = false;
    state.lastDateSeparatorTimestamp = null;
    state.outputDistanceFromBottom = 0;
    state.structuredMessages = [];
    stopActivityTimerIfIdle();
  }

  function appendOutput(text, tone) {
    if (!text) return;
    const visibleText = tone === "error" || tone === "meta" ? t(text) : text;
    if (tone === "meta") {
      setConversationStatus(String(visibleText));
      return;
    }
    finishAssistantStream();
    const role = tone === "error" ? "error" : tone === "meta" ? "system" : "assistant";
    appendMessage(visibleText, role, tone || "text", tone === "meta" ? t("状态") : "");
  }

  function setConversationStatus(text, tone = "ready") {
    const value = t(text || "");
    const status = $("appState");
    if (status) {
      status.textContent = value;
      status.dataset.tone = tone;
    }
    const hint = $("outputHint");
    if (hint && value) hint.textContent = value;
  }

  function sessionCommandMethod(value) {
    return String(value || "").trim().replace(/\./g, "/").toLowerCase();
  }

  function sessionErrorMessage(value, fallback = "会话操作失败") {
    const source = isRecord(value) ? value : {};
    const error = isRecord(source.error) ? source.error : {};
    const code = firstString(source.code, error.code).toLowerCase();
    const message = value instanceof Error
      ? value.message
      : firstString(source.message, error.message, typeof value === "string" ? value : "");
    const normalized = `${code} ${message}`.toLowerCase();
    if (/app_not_ready|app-server is not ready|waiting_for_host/.test(normalized)) return "等待 VS Code 主机连接";
    if (/host_unavailable|host_disconnected|vscode host is disconnected/.test(normalized)) return "VS Code 主机未连接";
    if (/mode_switch_pending/.test(normalized)) return "正在切换控制模式";
    if (/mode_busy|cannot switch control mode/.test(normalized)) return "当前任务或请求完成后才能切换控制模式";
    if (/session_busy|turn_active|running turn|pending request/.test(normalized)) return "当前任务结束或请求处理后才能切换";
    if (/timed out waiting for a snapshot|snapshot from vscode|找不到会话.*owner|no live vscode owner/.test(normalized)) {
      return "目标会话没有返回 VS Code 快照，请先在官方 Codex 面板打开它";
    }
    if (/method_not_allowed/.test(normalized)) return "当前 relay 版本不支持此会话操作，请重启 relay";
    return message || fallback;
  }

  function setSessionSwitchingVisual(switching) {
    const active = Boolean(switching);
    const panel = document.querySelector(".chat-panel");
    if (panel) panel.dataset.sessionSwitching = String(active);
    const output = $("output");
    if (output) output.setAttribute("aria-busy", String(active));
    updateIds();
    renderRequests();
  }

  function sessionSwitchTargetTitle(threadId) {
    const id = String(threadId || "");
    if (!id || !Array.isArray(state.sessions)) return "";
    const entry = state.sessions.find((candidate) => sessionEntryId(candidate) === id);
    return entry ? sessionEntryTitle(entry) : "";
  }

  function beginSessionSwitchContext(threadId, title = "") {
    const targetThreadId = String(threadId || "");
    if (state.sessionSwitchContext) {
      if (!targetThreadId || state.sessionSwitchContext.targetThreadId === targetThreadId) {
        return state.sessionSwitchContext;
      }
      // A newer VS Code navigation can supersede an in-flight target. Retain
      // the original fallback, but reset both completion gates for the new
      // target so an acknowledgement/snapshot from the older route cannot
      // unlock the composer.
      state.sessionSwitchContext.targetThreadId = targetThreadId;
      state.sessionSwitchContext.targetTitle = String(title || sessionSwitchTargetTitle(targetThreadId) || "");
      state.sessionSwitchContext.targetSnapshotReady = false;
      state.sessionSwitchContext.selectedAckReady = false;
      return state.sessionSwitchContext;
    }
    const titleNode = $("threadTitle");
    const previousThreadId = String(state.threadId || state.syncedThreadId || "");
    state.sessionSwitchContext = {
      previousThreadId,
      previousTitle: titleNode?.textContent || "Codex",
      targetThreadId,
      targetTitle: String(title || sessionSwitchTargetTitle(targetThreadId) || ""),
      targetSnapshotReady: false,
      selectedAckReady: false,
    };
    return state.sessionSwitchContext;
  }

  function finishSessionSwitchContext() {
    state.sessionSwitchContext = null;
    setSessionSwitchingVisual(false);
  }

  function restoreSessionSwitchContext() {
    const context = state.sessionSwitchContext;
    if (!context) {
      state.sessionSwitching = false;
      state.sessionSelectCommandId = "";
      setSessionSwitchingVisual(false);
      return;
    }
    const titleNode = $("threadTitle");
    if (titleNode && context.previousTitle) titleNode.textContent = context.previousTitle;
    if (context.previousThreadId) {
      state.threadId = context.previousThreadId;
      state.sessionSelectedThreadId = context.previousThreadId;
    } else {
      state.sessionSelectedThreadId = "";
    }
    state.sessionSwitching = false;
    state.sessionSelectCommandId = "";
    finishSessionSwitchContext();
  }

  function failSessionSwitch(value, fallback = "会话切换失败") {
    // A target projection may arrive before the adapter's final owner check.
    // A later failure must still restore the old routing context; treating the
    // early snapshot as success strands the browser on an unconfirmed target.
    restoreSessionSwitchContext();
    return sessionErrorMessage(value, fallback);
  }

  function finishSessionSwitchIfReady() {
    const context = state.sessionSwitchContext;
    if (!context || !context.targetSnapshotReady || !context.selectedAckReady) return false;
    const targetThreadId = String(context.targetThreadId || "");
    if (!targetThreadId || state.syncedThreadId !== targetThreadId) return false;
    const titleNode = $("threadTitle");
    if (context.targetTitle && titleNode) titleNode.textContent = context.targetTitle;
    state.sessionSelectedThreadId = "";
    state.sessionSwitching = false;
    state.sessionSelectCommandId = "";
    finishSessionSwitchContext();
    syncSessionActive(targetThreadId);
    return true;
  }

  function sessionEntryId(entry) {
    if (!isRecord(entry)) return "";
    const thread = isRecord(entry.thread) ? entry.thread : {};
    return firstString(entry.threadId, entry.conversationId, entry.conversation_id, entry.id,
      thread.threadId, thread.conversationId, thread.conversation_id, thread.id);
  }

  function sessionEntryTitle(entry) {
    if (!isRecord(entry)) return "";
    const thread = isRecord(entry.thread) ? entry.thread : {};
    return firstString(entry.title, entry.name, entry.preview, entry.firstUserMessage, entry.first_user_message,
      entry.threadTitle, entry.thread_name, thread.title, thread.name, thread.preview, thread.thread_name);
  }

  function sessionEntryCwd(entry) {
    if (!isRecord(entry)) return "";
    const thread = isRecord(entry.thread) ? entry.thread : {};
    return firstString(entry.cwd, entry.workspace, entry.workspacePath, entry.workspace_path,
      thread.cwd, thread.workspace, thread.workspacePath, thread.workspace_path);
  }

  function sessionEntryUpdatedAt(entry) {
    if (!isRecord(entry)) return null;
    const thread = isRecord(entry.thread) ? entry.thread : {};
    // App-server history is ordered by recency_at. Older relay versions only
    // expose updatedAt, so keep those fields as a compatibility fallback.
    return timestampMs(
      entry.recencyAtMs, entry.recencyAt, entry.recency_at_ms, entry.recency_at,
      thread.recencyAtMs, thread.recencyAt, thread.recency_at_ms, thread.recency_at,
      entry.updatedAtMs, entry.updatedAt, entry.lastUpdatedAtMs, entry.lastUpdatedAt,
      entry.updated_at_ms, entry.updated_at, entry.last_updated_at,
      entry.mtime, entry.modifiedAt, thread.updatedAtMs, thread.updatedAt,
      thread.updated_at_ms, thread.updated_at,
    );
  }

  function sessionEntryStatus(entry) {
    if (!isRecord(entry)) return { kind: "idle", label: "", active: false, attention: false, unread: false };
    const thread = isRecord(entry.thread) ? entry.thread : {};
    const nestedStatus = isRecord(entry.status) ? entry.status : {};
    const nestedExecutionStatus = isRecord(entry.executionStatus) ? entry.executionStatus : {};
    const nestedThreadStatus = isRecord(thread.status) ? thread.status : {};
    let rawStatus = firstString(
      entry.activity, entry.activityStatus, entry.status, entry.executionStatus, entry.turnStatus,
      entry.threadRuntimeStatus, entry.thread_runtime_status, entry.runtimeStatus, entry.lastTurnStatus,
      entry.last_turn_status, entry.phase, entry.state,
      nestedStatus.activity, nestedStatus.type, nestedStatus.status, nestedStatus.kind,
      nestedExecutionStatus.activity, nestedExecutionStatus.type, nestedExecutionStatus.status, nestedExecutionStatus.kind,
      nestedThreadStatus.activity, nestedThreadStatus.type, nestedThreadStatus.status, nestedThreadStatus.kind,
      thread.threadRuntimeStatus, thread.thread_runtime_status, thread.turnStatus, thread.lastTurnStatus, thread.state,
    ).replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase().replace(/[\s-]+/g, "_");
    if ((!rawStatus || rawStatus === "idle") && entry.active) {
      rawStatus = firstString(
        state.currentActivity !== "idle" ? state.currentActivity : "",
        state.turnStatus,
        state.turnId ? "working" : "",
      ).replace(/([a-z])([A-Z])/g, "$1_$2").toLowerCase().replace(/[\s-]+/g, "_");
    }
    const unread = [
      entry.hasUnreadTurn, entry.has_unread_turn, entry.unread, entry.isUnread,
      entry.needsAttention, entry.needs_attention, thread.hasUnreadTurn, thread.has_unread_turn,
    ].some((value) => value === true || value === 1 || ["true", "1", "yes"].includes(String(value || "").toLowerCase()));
    let kind = "idle";
    if (entry.isApproval === true || /approval|permission|request_approval|awaiting_authorization|needs_authorization|requires_action/.test(rawStatus)) kind = "approval";
    else if (entry.isWaiting === true || /needs?_?input|waiting_for_input|pending_input|pending|queued/.test(rawStatus)) kind = "waiting";
    else if (entry.isEditing === true || /edit|apply_patch|file_change|writing/.test(rawStatus)) kind = "editing";
    else if (entry.isThinking === true || /think|reason/.test(rawStatus)) kind = "thinking";
    else if (entry.isWorking === true || entry.isRunning === true || /run|stream|working|in_progress|active|busy|generat/.test(rawStatus)) kind = "working";
    else if (/error|fail|cancel|interrupt/.test(rawStatus)) kind = "error";
    else if (unread) kind = "unread";
    const labels = {
      approval: "等待授权",
      waiting: "等待输入",
      editing: "编辑中",
      thinking: "思考中",
      working: "进行中",
      error: "异常",
      unread: "未读",
      idle: "",
    };
    return {
      kind,
      label: labels[kind] || "",
      active: ["approval", "waiting", "editing", "thinking", "working"].includes(kind),
      attention: ["approval", "waiting", "error", "unread"].includes(kind) || unread,
      unread,
    };
  }

  function sessionEntrySearchText(entry) {
    if (!isRecord(entry)) return "";
    const thread = isRecord(entry.thread) ? entry.thread : {};
    const status = sessionEntryStatus(entry);
    return [
      sessionEntryTitle(entry), sessionEntryCwd(entry), sessionEntryId(entry),
      status.label, entry.mode, entry.source, entry.threadSource, thread.mode,
    ].filter((value) => typeof value === "string" && value.trim()).join(" ").toLowerCase();
  }

  function sessionOptionDomId(threadId) {
    try {
      return `session-option-${encodeURIComponent(String(threadId)).replace(/%/g, "_")}`;
    } catch {
      return `session-option-${String(threadId).replace(/[^a-z0-9_-]/gi, "_")}`;
    }
  }

  function sessionEntryIsActive(entry, activeId = "") {
    if (!isRecord(entry)) return false;
    const id = sessionEntryId(entry);
    return id === activeId || entry.active === true || entry.active === 1
      || ["true", "1", "yes"].includes(String(entry.active || "").toLowerCase());
  }

  function sessionEntryIsAvailable(entry) {
    return isRecord(entry)
      && entry.available !== false
      && entry.canAttach !== false
      && entry.attachable !== false;
  }

  function sessionIsSelectable(entry, activeId, canSwitch) {
    if (!isRecord(entry)) return false;
    const id = sessionEntryId(entry);
    const available = sessionEntryIsAvailable(entry);
    const current = sessionEntryIsActive(entry, activeId);
    return Boolean(id && available && !current && canSwitch && !state.sessionSwitching);
  }

  function filteredSessionEntries() {
    const source = Array.isArray(state.sessions)
      ? state.sessions
        .filter(isRecord)
        .filter((entry) => !state.attachMode || sessionEntryIsAvailable(entry))
        .map((entry, index) => ({ entry, index }))
      : [];
    source.sort((left, right) => {
      const rightTime = sessionEntryUpdatedAt(right.entry) ?? 0;
      const leftTime = sessionEntryUpdatedAt(left.entry) ?? 0;
      return rightTime - leftTime || left.index - right.index;
    });
    const ordered = source.map(({ entry }) => entry);
    const query = String(state.sessionSearch || "").trim().toLowerCase();
    return query ? ordered.filter((entry) => sessionEntrySearchText(entry).includes(query)) : ordered;
  }

  function sessionPathLabel(value) {
    const text = String(value || "").trim();
    if (!text) return t("本地会话");
    const parts = text.split(/[\\/]+/).filter(Boolean);
    return parts.length > 1 ? `${t("工作区")} · ${parts[parts.length - 1]}` : text;
  }

  function sessionTimeLabel(value) {
    const timestamp = timestampMs(value);
    if (timestamp === null) return "";
    try {
      const date = new Date(timestamp);
      const now = new Date();
      const day = Date.UTC(date.getFullYear(), date.getMonth(), date.getDate());
      const today = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate());
      const difference = Math.round((today - day) / 86_400_000);
      const locale = uiLocale();
      const time = new Intl.DateTimeFormat(locale, { hour: "2-digit", minute: "2-digit" }).format(date);
      if (difference === 0) return time;
      if (difference === 1) return `${t("昨天")} ${time}`;
      if (difference > 1 && difference < 7) return `${new Intl.DateTimeFormat(locale, { weekday: "short" }).format(date)} ${time}`;
      return `${new Intl.DateTimeFormat(locale, { month: "numeric", day: "numeric" }).format(date)} ${time}`;
    } catch {
      return "";
    }
  }

  function renderSessionPicker() {
    const list = $("sessionList");
    const status = $("sessionPickerStatus");
    const picker = $("sessionPicker");
    if (!list || !status || !picker) return;
    renderControlMode();
    const activeId = String(state.threadId || state.syncedThreadId || "");
    const canSwitch = sessionControlAllowed("sessionSelect")
      && state.appReady
      && state.ws?.readyState === WebSocket.OPEN
      && !state.turnId
      && state.requests.size === 0;
    const allSessions = Array.isArray(state.sessions)
      ? state.sessions.filter(isRecord).filter((entry) => !state.attachMode || sessionEntryIsAvailable(entry))
      : [];
    const sessions = filteredSessionEntries();
    const query = String(state.sessionSearch || "").trim();
    const selectableIds = sessions
      .filter((entry) => sessionIsSelectable(entry, activeId, canSwitch))
      .map((entry) => sessionEntryId(entry));
    if (!selectableIds.includes(state.sessionFocusedId)) {
      state.sessionFocusedId = selectableIds[0] || "";
    }
    const searchInput = $("sessionSearchInput");
    const searchClear = $("sessionSearchClear");
    if (searchInput) {
      if (searchInput.value !== state.sessionSearch) searchInput.value = state.sessionSearch;
      searchInput.setAttribute("aria-expanded", String(state.sessionPickerOpen));
      searchInput.setAttribute("aria-activedescendant", state.sessionFocusedId ? sessionOptionDomId(state.sessionFocusedId) : "");
    }
    if (searchClear) searchClear.hidden = !query;
    list.replaceChildren();
    picker.dataset.switching = String(state.sessionSwitching);
    list.setAttribute("aria-activedescendant", state.sessionFocusedId ? sessionOptionDomId(state.sessionFocusedId) : "");
    status.dataset.tone = state.sessionListError ? "warning" : "";
    if (state.sessionListLoading && !sessions.length) status.textContent = t("正在读取会话…");
    else if (state.sessionSwitching) {
      const targetTitle = state.sessionSwitchContext?.targetTitle;
      status.textContent = targetTitle
        ? uiLocale() === "en-US" ? `Switching to “${targetTitle}”...` : `正在切换到「${targetTitle}」…`
        : t("正在切换会话…");
    }
    else if (state.sessionListError) status.textContent = t(state.sessionListError);
    else if (!canSwitch && sessions.length > 1) status.textContent = t("当前任务结束或请求处理后才能切换");
    else if (query) status.textContent = uiLocale() === "en-US"
      ? `${sessions.length}/${allSessions.length} conversations`
      : `${sessions.length}/${allSessions.length} 个会话`;
    else status.textContent = sessions.length
      ? (uiLocale() === "en-US" ? `${sessions.length} conversations` : `${sessions.length} 个会话`)
      : "";

    if (!sessions.length && !state.sessionListLoading) {
      const empty = document.createElement("div");
      empty.className = "session-list-empty";
      const listError = String(state.sessionListError || "");
      empty.textContent = listError === "等待 VS Code 主机连接"
        ? t("等待 VS Code 伴随扩展连接")
        : listError === "VS Code 主机未连接"
          ? t("VS Code 伴随扩展未连接")
          : listError === "等待 relay 连接"
            ? t("等待 relay 连接")
            : listError
              ? t("无法读取会话")
              : query
                ? t("没有匹配的会话")
                : t(state.attachMode ? "没有可附加的会话" : "没有可控制的会话");
      list.append(empty);
      return;
    }
    for (const raw of sessions) {
      if (!isRecord(raw)) continue;
      const id = sessionEntryId(raw);
      if (!id) continue;
      const title = sessionEntryTitle(raw) || `${t("会话")} ${id.slice(0, 8)}`;
      const cwd = sessionEntryCwd(raw);
      const updated = sessionEntryUpdatedAt(raw);
      const current = sessionEntryIsActive(raw, activeId);
      const available = sessionEntryIsAvailable(raw);
      const statusInfo = sessionEntryStatus(raw);
      const switchingTarget = state.sessionSwitching && id === state.sessionSelectedThreadId;
      const selectable = sessionIsSelectable(raw, activeId, canSwitch);
      const option = document.createElement("button");
      option.type = "button";
      option.className = "session-option";
      option.id = sessionOptionDomId(id);
      option.dataset.available = String(available);
      option.dataset.threadId = id;
      option.dataset.status = statusInfo.kind;
      option.dataset.unread = String(statusInfo.unread);
      option.dataset.switching = String(switchingTarget);
      option.dataset.focused = String(id === state.sessionFocusedId);
      option.setAttribute("role", "option");
      option.setAttribute("aria-selected", String(current));
      option.setAttribute("aria-disabled", String(!selectable));
      option.disabled = !selectable;
      const titleNode = document.createElement("span");
      titleNode.className = "session-option-title";
      titleNode.textContent = title;
      titleNode.title = title;
      const timeNode = document.createElement("span");
      timeNode.className = "session-option-time";
      timeNode.textContent = sessionTimeLabel(updated);
      const metaNode = document.createElement("span");
      metaNode.className = "session-option-meta";
      metaNode.textContent = sessionPathLabel(cwd);
      metaNode.title = cwd || title;
      const stateNode = document.createElement("span");
      stateNode.className = "session-option-state";
      const stateLabels = [];
      if (switchingTarget) stateLabels.push(t("正在切换"));
      else if (!available) stateLabels.push(t("未打开"));
      else if (current) stateLabels.push(t("当前"));
      else if (!statusInfo.label) stateLabels.push(t("可切换"));
      if (available && statusInfo.label && (!current || statusInfo.active || statusInfo.attention)) stateLabels.push(t(statusInfo.label));
      const stateDot = document.createElement("span");
      stateDot.className = "session-status-dot";
      stateDot.setAttribute("aria-hidden", "true");
      stateNode.append(stateDot, document.createTextNode(stateLabels.join(" · ")));
      option.append(titleNode, timeNode, metaNode, stateNode);
      option.setAttribute("aria-label", `${title}, ${stateLabels.join(", ") || t("会话")}`);
      option.addEventListener("mouseenter", () => {
        if (option.disabled) return;
        state.sessionFocusedId = id;
        for (const peer of list.querySelectorAll(".session-option")) peer.dataset.focused = String(peer.dataset.threadId === id);
        list.setAttribute("aria-activedescendant", sessionOptionDomId(id));
      });
      if (!option.disabled) option.addEventListener("click", () => {
        state.sessionFocusedId = id;
        selectSession(id, title);
      });
      list.append(option);
    }
  }

  function sessionPickerOptions() {
    return [...document.querySelectorAll("#sessionList .session-option")]
      .filter((option) => !option.disabled && option.dataset.threadId);
  }

  function setSessionFocus(threadId, { scroll = true } = {}) {
    const id = String(threadId || "");
    state.sessionFocusedId = id;
    const list = $("sessionList");
    if (!list) return;
    const options = list.querySelectorAll(".session-option");
    for (const option of options) option.dataset.focused = String(option.dataset.threadId === id);
    list.setAttribute("aria-activedescendant", id ? sessionOptionDomId(id) : "");
    $("sessionSearchInput")?.setAttribute("aria-activedescendant", id ? sessionOptionDomId(id) : "");
    if (scroll) {
      const target = [...list.querySelectorAll(".session-option")]
        .find((option) => option.dataset.threadId === id);
      target?.scrollIntoView?.({ block: "nearest" });
    }
  }

  function moveSessionFocus(delta) {
    const options = sessionPickerOptions();
    if (!options.length) return false;
    let index = options.findIndex((option) => option.dataset.threadId === state.sessionFocusedId);
    if (index < 0) index = delta >= 0 ? -1 : 0;
    index = (index + delta + options.length) % options.length;
    setSessionFocus(options[index].dataset.threadId);
    return true;
  }

  function activateFocusedSession() {
    const id = state.sessionFocusedId;
    if (!id) return false;
    const entry = (Array.isArray(state.sessions) ? state.sessions : [])
      .find((candidate) => sessionEntryId(candidate) === id);
    if (!entry) return false;
    const activeId = String(state.threadId || state.syncedThreadId || "");
    const canSwitch = !state.turnId && state.requests.size === 0;
    if (!sessionIsSelectable(entry, activeId, canSwitch)) return false;
    selectSession(id, sessionEntryTitle(entry));
    return true;
  }

  function handleSessionPickerKeydown(event) {
    if (!state.sessionPickerOpen) return;
    if (event.key === "ArrowDown") {
      if (moveSessionFocus(1)) event.preventDefault();
      return;
    }
    if (event.key === "ArrowUp") {
      if (moveSessionFocus(-1)) event.preventDefault();
      return;
    }
    if (event.key === "Home") {
      const options = sessionPickerOptions();
      if (options.length) {
        setSessionFocus(options[0].dataset.threadId);
        event.preventDefault();
      }
      return;
    }
    if (event.key === "End") {
      const options = sessionPickerOptions();
      if (options.length) {
        setSessionFocus(options[options.length - 1].dataset.threadId);
        event.preventDefault();
      }
      return;
    }
    if (event.key === "Enter") {
      if (activateFocusedSession()) event.preventDefault();
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      setSessionPicker(false);
    }
  }

  function setSessionPicker(open) {
    const picker = $("sessionPicker");
    const button = $("sessionPickerButton");
    if (!picker || !button) return;
    const next = Boolean(open) && sessionControlAllowed("sessionList") && !state.modeSwitching;
    picker.hidden = !next;
    state.sessionPickerOpen = next;
    button.setAttribute("aria-expanded", String(next));
    if (next) {
      state.sessionSearch = "";
      state.sessionFocusedId = "";
      $("panelMenu").hidden = true;
      $("detailsPopover").hidden = true;
      renderSessionPicker();
      requestSessionList();
      scheduleFrame(() => {
        if (state.sessionPickerOpen) $("sessionSearchInput")?.focus();
      });
    } else {
      state.sessionSearch = "";
      state.sessionFocusedId = "";
      const input = $("sessionSearchInput");
      if (input) {
        input.value = "";
        input.setAttribute("aria-expanded", "false");
        input.setAttribute("aria-activedescendant", "");
      }
      const clear = $("sessionSearchClear");
      if (clear) clear.hidden = true;
      if (picker.contains(document.activeElement)) button.focus();
    }
  }

  function openSessionHistory() {
    if (!sessionControlAllowed("sessionList")) return;
    setSessionPicker(true);
  }

  function requestControlMode(value) {
    const mode = normalizeControlMode(value);
    if (!mode || mode === state.controlMode || controlModeChangeBlocked()) return;
    closePopovers();
    state.modeSwitching = true;
    state.requestedControlMode = mode;
    state.modeRequestEpoch = state.modeEpoch;
    setConversationStatus("正在切换控制模式", "active");
    updateIds();
    try {
      state.modeCommandId = command("control/mode/set", { mode });
    } catch (error) {
      clearControlModeRequest();
      setConversationStatus(error?.message || "控制模式切换失败", "warning");
      updateIds();
    }
  }

  function requestNewSession() {
    closePopovers();
    if (!sessionControlAllowed("sessionCreate")) {
      setConversationStatus("同步模式下会话管理由 VS Code 控制", "warning");
      return;
    }
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
      setConversationStatus("等待 relay 连接", "warning");
      return;
    }
    if (!state.appReady) {
      setConversationStatus("等待 VS Code 主机连接", "warning");
      return;
    }
    if (state.role !== "operator" && state.role !== "owner" && state.role !== "host") {
      setConversationStatus("当前角色不能创建会话", "warning");
      return;
    }
    if (state.newSessionCommandId) return;
    try {
      state.newSessionCommandId = command("session/new", {});
      setConversationStatus("正在创建新会话", "active");
      updateIds();
    } catch (error) {
      state.newSessionCommandId = "";
      setConversationStatus(sessionErrorMessage(error, "无法创建新会话"), "warning");
      updateIds();
    }
  }

  function requestSessionList() {
    if (!sessionControlAllowed("sessionList")) return;
    if (state.sessionListLoading) {
      renderSessionPicker();
      return;
    }
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) {
      state.sessionListError = "等待 relay 连接";
      renderSessionPicker();
      return;
    }
    if (!state.appReady) {
      state.sessionListError = "等待 VS Code 主机连接";
      renderSessionPicker();
      return;
    }
    state.sessionListLoading = true;
    state.sessionListError = "";
    renderSessionPicker();
    try {
      state.sessionListCommandId = command("session/list", {});
    } catch (error) {
      state.sessionListLoading = false;
      state.sessionListError = sessionErrorMessage(error, "无法读取会话");
      renderSessionPicker();
    }
  }

  function selectSession(threadId, title = "") {
    const id = String(threadId || "").trim();
    if (!sessionControlAllowed("sessionSelect")) return;
    if (!id || state.sessionSwitching || id === state.threadId) return;
    if (state.turnId || state.requests.size) {
      state.sessionListError = "当前任务仍在运行或等待授权，暂不能切换";
      renderSessionPicker();
      return;
    }
    const context = beginSessionSwitchContext(id, title);
    state.sessionSwitching = true;
    state.sessionListError = "";
    setSessionSwitchingVisual(true);
    renderSessionPicker();
    try {
      state.sessionSelectCommandId = command("session/select", { threadId: id });
      setConversationStatus(
        context.targetTitle
          ? uiLocale() === "en-US" ? `Switching to “${context.targetTitle}”` : `正在切换到「${context.targetTitle}」`
          : t("正在切换会话"),
        "active",
      );
    } catch (error) {
      restoreSessionSwitchContext();
      state.sessionListError = sessionErrorMessage(error, "会话切换失败");
      renderSessionPicker();
    }
  }

  function applySessionListResult(result) {
    const body = isRecord(result) ? result : {};
    const source = Array.isArray(result) ? result : Array.isArray(body.sessions) ? body.sessions : Array.isArray(body.threads) ? body.threads : [];
    const activeId = firstString(body.activeThreadId, body.threadId);
    state.sessions = source
      .filter((entry) => isRecord(entry))
      .filter((entry) => !state.attachMode || sessionEntryIsAvailable(entry))
      .map((entry) => ({ ...entry }));
    if (activeId) state.sessions = state.sessions.map((entry) => ({ ...entry, active: sessionEntryId(entry) === activeId || entry.active === true }));
    state.sessionListLoading = false;
    state.sessionListCommandId = "";
    state.sessionListError = "";
    renderSessionPicker();
  }

  function applySessionSelectResult(result) {
    const body = isRecord(result) ? result : {};
    const selected = firstString(body.threadId, body.activeThreadId);
    state.sessionSelectedThreadId = selected;
    if (selected && state.sessions.length) {
      state.sessions = state.sessions.map((entry) => ({ ...entry, active: sessionEntryId(entry) === selected }));
    }
    // A command result confirms only that the command completed. The switch
    // itself stays fenced until both the sequenced `session.selected` event
    // and the target's authoritative projection have independently arrived.
    // This also keeps automatic VS Code navigation and picker navigation on
    // the same completion path.
    if (state.sessionSwitchContext) {
      state.sessionSwitching = true;
      setSessionSwitchingVisual(true);
      const context = state.sessionSwitchContext;
      setConversationStatus(
        context.targetSnapshotReady ? "正在确认会话" : "正在加载会话",
        "active",
      );
    } else {
      state.sessionSwitching = false;
      state.sessionSelectCommandId = "";
      finishSessionSwitchContext();
    }
    renderSessionPicker();
    if (!state.sessionSwitching) {
      setSessionPicker(false);
      setConversationStatus("会话已切换", "ready");
    }
    requestRefresh();
  }

  function syncSessionActive(threadId) {
    const activeId = String(threadId || "");
    if (!activeId || !Array.isArray(state.sessions) || !state.sessions.length) return;
    state.sessions = state.sessions.map((entry) => ({
      ...entry,
      active: sessionEntryId(entry) === activeId,
    }));
    renderSessionPicker();
  }

  function messageText(item) {
    if (!isRecord(item)) return "";
    const direct = [item.text, item.content, item.message, item.summary, item.output]
      .find((value) => typeof value === "string" && value.length);
    if (direct) return direct;
    if (Array.isArray(item.content)) {
      return item.content.map((part) => {
        if (typeof part === "string") return part;
        if (!isRecord(part)) return "";
        return part.text || part.content || part.value || "";
      }).filter(Boolean).join("\n");
    }
    // Official collaboration records are often state-only projections: they
    // carry an agent id/name and lifecycle kind, but no user-facing `text`.
    // Keep those rows in the transcript so the sub-agent disclosure and panel
    // can be hydrated from the same snapshot.
    if (historyKind(item) === "subagent") {
      const agent = isRecord(item.agent) ? item.agent : {};
      const name = firstString(
        item.displayName,
        item.agentNickname,
        item.agentName,
        item.agentPath,
        agent.displayName,
        agent.name,
        item.agentThreadId ? `thread ${item.agentThreadId}` : "子代理",
      );
      const objective = firstString(
        item.objective,
        item.prompt,
        item.statusMessage,
        item.description,
        agent.objective,
        agent.prompt,
      );
      const action = firstString(item.action, item.tool, item.activityKind, agent.action);
      const rawStatus = firstString(item.displayStatus, item.status, item.state, agent.status);
      const normalizedStatus = normalizeSubagentStatus(rawStatus);
      const statusText = subagentStatusLabel(normalizedStatus);
      if (objective && name) return `${name}：${objective}`;
      if (action && name) return `${name} · ${action}`;
      if (name) return `${name} · ${statusText}`;
      return `子代理 · ${statusText}`;
    }
    if (isReadActivity(item)) {
      return firstString(...readPathList(item), commandText(item), "读取文件");
    }
    if (/(?:command|exec|process|tool)/i.test(String(item.type || item.kind || ""))) {
      return firstString(commandText(item), "工具输出");
    }
    const fallback = [item.reasoning, item.plan, item.steps, item.diff, item.patch, item.description]
      .map((value) => displayValue(value))
      .find((value) => value);
    if (fallback) return fallback;
    return "";
  }

  function historyKind(item) {
    const sourceKind = String(item?.kind || item?.type || "assistant").toLowerCase();
    if (item?.uiType === "subagent-activity" || item?.uiType === "multi-agent-action"
      || sourceKind.includes("subagent") || sourceKind.includes("collabagent")) return "subagent";
    if (item?.role === "user" || sourceKind.includes("user")) return "user";
    if (isReadActivity(item)) return "read";
    if (sourceKind.includes("edit") || sourceKind.includes("filechange") || sourceKind.includes("patch")) return "edit";
    if (sourceKind.includes("tool") || sourceKind.includes("command") || sourceKind.includes("exec") || sourceKind.includes("process")
      || sourceKind.includes("websearch") || sourceKind.includes("mcp") || sourceKind.includes("imageview")
      || sourceKind.includes("generatedimage") || sourceKind.includes("dynamictool")
      || sourceKind.includes("permissionrequest") || sourceKind.includes("userinput")) return "tool";
    if (sourceKind.includes("reasoning") || sourceKind.includes("contextcompaction") || sourceKind.includes("approvalreview")) return "reasoning";
    if (sourceKind.includes("plan") || sourceKind.includes("todo")) return "plan";
    return "assistant";
  }

  function normalizedAssistantPhase(item) {
    return String(item?.phase || item?.messagePhase || "")
      .replace(/[\s-]+/g, "_")
      .toLowerCase();
  }

  // Agent commentary is a work item in the official transcript. It belongs
  // inside the worked-for disclosure, while the final answer stays outside it.
  // Older snapshots omit `phase`, so use the last item in the turn as the
  // fallback final answer boundary.
  function isAssistantCommentary(item, index, turnKey, turnLastIndex) {
    if (historyKind(item) !== "assistant") return false;
    const phase = normalizedAssistantPhase(item);
    if (phase === "final_answer" || phase === "finalanswer") return false;
    if (phase === "commentary" || phase === "analysis" || phase === "reasoning" || phase === "thinking") return true;
    return turnLastIndex instanceof Map && turnLastIndex.get(turnKey) !== index;
  }

  function structuredTurnFinalAssistantIndexes(messages) {
    const result = new Map();
    const explicit = new Set();
    if (!Array.isArray(messages)) return result;
    messages.forEach((item, index) => {
      if (!isRecord(item) || historyKind(item) !== "assistant") return;
      const key = structuredMessageTurn(item, index);
      const phase = normalizedAssistantPhase(item);
      if (phase === "final_answer" || phase === "finalanswer") {
        result.set(key, index);
        explicit.add(key);
      } else if (!explicit.has(key)) {
        // A bookkeeping/sub-agent item can follow the answer in newer
        // snapshots. Choose the last assistant message, not the last item.
        result.set(key, index);
      }
    });
    return result;
  }

  function structuredDisplayKind(item, index, turnKey, turnLastIndex) {
    const kind = historyKind(item);
    return kind === "assistant" && isAssistantCommentary(item, index, turnKey, turnLastIndex)
      ? "commentary"
      : kind;
  }

  function isCollapsibleKind(kind) {
    return kind === "tool" || kind === "read" || kind === "edit" || kind === "reasoning"
      || kind === "plan" || kind === "subagent" || kind === "commentary";
  }

  function isReadActivity(item) {
    if (!isRecord(item)) return false;
    const type = normalizedItemType(item);
    const semantic = String(item.activityKind || item.uiType || item.operation || item.commandType || "")
      .replace(/[\s_./-]+/g, "")
      .toLowerCase();
    const parsed = item.parsedCmd || item.parsedCommand || item.parsedCommandType;
    const parsedType = typeof parsed === "string"
      ? parsed
      : isRecord(parsed) ? firstString(parsed.type, parsed.kind, parsed.operation) : "";
    const parsedNormalized = String(parsedType).replace(/[\s_./-]+/g, "").toLowerCase();
    return semantic.includes("fileread") || semantic.includes("readfile") || semantic === "read"
      || type.includes("fileread") || type.includes("readfile") || type === "read"
      || parsedNormalized === "read" || parsedNormalized === "fileread";
  }

  function readPathList(item) {
    if (!isRecord(item)) return [];
    const paths = [];
    const visit = (value) => {
      if (typeof value === "string" && value.trim()) paths.push(value.trim());
      else if (Array.isArray(value)) value.forEach(visit);
      else if (isRecord(value)) visit(value.path ?? value.file ?? value.filePath ?? value.name);
    };
    [item.path, item.file, item.filePath, item.filename, item.name, item.readPath, item.readPaths, item.files, item.paths].forEach(visit);
    return [...new Set(paths)];
  }

  function readSummaryLabel(item, status, duration = "") {
    const paths = [...new Set(readPathList(item).flatMap((value) => String(value).split(/\r?\n/).map((part) => part.trim()).filter(Boolean)))];
    const suffix = duration ? ` · ${duration}` : "";
    if (status === "inProgress") return paths.length === 1
      ? uiWithRaw("正在读取 ", "Reading ", paths[0])
      : t("正在读取文件");
    if (status === "failed") return paths.length === 1
      ? `${uiWithRaw("读取失败 · ", "Failed to read ", paths[0])}${suffix}`
      : `${t("读取文件失败")}${suffix}`;
    if (status === "interrupted") return paths.length === 1
      ? `${uiWithRaw("已停止读取 ", "Stopped reading ", paths[0])}${suffix}`
      : `${t("已停止读取文件")}${suffix}`;
    if (paths.length === 1) return `${uiWithRaw("已读取 ", "Read ", paths[0])}${suffix}`;
    if (paths.length > 1) {
      const count = String(paths.length);
      return `${uiText("已读取这些内容 · ", "Read these items · ")}${count}${uiText(" 个文件", " files")}${suffix}`;
    }
    return `${t("已读取文件")}${suffix}`;
  }

  function structuredMessageKey(item, index) {
    const id = item?.id ?? item?.itemId;
    if (id !== undefined && id !== null && String(id)) return `id:${String(id)}`;
    const turn = item?.turnId ? String(item.turnId) : "item";
    return `turn:${turn}:${historyKind(item)}:${index}`;
  }

  function structuredMessageRole(item, kind) {
    return item.role === "user" || kind === "user"
      ? "user"
      : item.role === "tool" || kind === "tool" || kind === "read" || kind === "edit" || kind === "subagent"
        ? "tool"
        : item.role === "reasoning" || kind === "reasoning" || kind === "plan"
          ? "system"
          : item.role === "error"
            ? "error"
            : "assistant";
  }

  function structuredMessageStatus(item) {
    if (item.status !== undefined && item.status !== null && item.status !== "") {
      return normalizeActivityStatus(item.status, String(item.status));
    }
    if (item.completed === true) return "completed";
    if (item.completed === false) return "inProgress";
    if (item.turnStatus !== undefined && item.turnStatus !== null && item.turnStatus !== "") {
      return normalizeActivityStatus(item.turnStatus, String(item.turnStatus));
    }
    return "";
  }

  function explicitStructuredTurnId(item) {
    if (!isRecord(item)) return "";
    const nestedTurn = isRecord(item.turn) ? item.turn : {};
    const value = [
      item.turnId,
      item.conversationTurnId,
      item.conversation_turn_id,
      item.turn_id,
      nestedTurn.id,
      nestedTurn.turnId,
    ].find((candidate) => candidate !== undefined && candidate !== null && String(candidate).trim());
    return value === undefined ? "" : String(value);
  }

  function deriveStructuredTurnKeys(messages) {
    if (!Array.isArray(messages)) return new Map();
    const keys = new Map();
    let anonymousNumber = 0;
    let currentKey = "";
    let currentHasUser = false;
    let currentEnded = false;
    const makeAnonymousKey = () => `anonymous:${anonymousNumber++}`;
    messages.forEach((item, index) => {
      if (!isRecord(item)) return;
      const explicit = explicitStructuredTurnId(item);
      const kind = historyKind(item);
      if (explicit) {
        currentKey = explicit;
        currentHasUser = kind === "user";
        currentEnded = false;
        keys.set(item, currentKey);
        return;
      }
      if (!currentKey || kind === "user" && (currentHasUser || currentEnded)) {
        currentKey = makeAnonymousKey();
        currentHasUser = false;
        currentEnded = false;
      }
      if (kind === "user") currentHasUser = true;
      keys.set(item, currentKey);
      // A final-answer marker is the only reliable boundary in a legacy
      // projection. Do not use turnStatus here: adapters attach the terminal
      // turn status to every item in the turn.
      const phase = normalizedAssistantPhase(item);
      if (kind === "assistant" && (phase === "final_answer" || phase === "finalanswer")) currentEnded = true;
      // Keep the index in the map as a debugging fallback for primitive array
      // entries, while object identity remains the canonical lookup.
      if (index < 0) keys.set(item, currentKey);
    });
    state.structuredTurnKeys = new WeakMap();
    for (const [item, key] of keys) state.structuredTurnKeys.set(item, key);
    return keys;
  }

  function structuredMessageTurn(item, index) {
    const explicit = explicitStructuredTurnId(item);
    if (explicit) return explicit;
    if (isRecord(item)) {
      const derived = state.structuredTurnKeys?.get(item);
      if (derived) return derived;
    }
    return `item:${index}`;
  }

  function structuredActivityKey(item, kind, turnKey, index) {
    const itemId = item?.itemId ?? item?.id;
    const threadKey = item?.threadId || state.threadId || "thread";
    if (itemId !== undefined && itemId !== null && String(itemId)) {
      return `${threadKey}:${turnKey || "turn"}:${kind}:${typeof itemId}:${String(itemId)}`;
    }
    return `structured:${structuredMessageKey(item, index)}`;
  }

  function structuredTurnWorkedDurations(messages) {
    const result = new Map();
    const explicitKeys = new Set();
    const starts = new Map();
    const ends = new Map();
    if (!Array.isArray(messages)) return result;
    deriveStructuredTurnKeys(messages);
    const finalAssistantIndexes = structuredTurnFinalAssistantIndexes(messages);
    messages.forEach((item, index) => {
      const key = structuredMessageTurn(item, index);
      const explicit = finiteNumber(
        item?.workedDurationMs,
        item?.workDurationMs,
        item?.workedForMs,
        item?.turnWorkedDurationMs,
        item?.workedFor?.durationMs,
      );
      if (explicit !== null) {
        result.set(key, Math.max(0, explicit));
        explicitKeys.add(key);
      }
      const workStart = timestampMs(
        item?.firstTurnWorkItemStartedAtMs,
        item?.workStartedAtMs,
        item?.turnWorkStartedAtMs,
        item?.turnStartedAtMs,
        item?.workedFor?.startedAtMs,
      );
      if (workStart !== null && !starts.has(key)) starts.set(key, workStart);
      const kind = historyKind(item);
      if (kind !== "user") {
        const itemStart = timestampMs(item?.startedAtMs, item?.startedAt, item?.createdAtMs);
        if (itemStart !== null && kind !== "assistant") {
          // Activity timestamps are the closest equivalent to the official
          // first-turn-work-item marker in older follower snapshots.
          if (!starts.has(key)) starts.set(key, itemStart);
        }
      }
      const explicitAssistantStart = timestampMs(
        item?.finalAssistantStartedAtMs,
        item?.workedFor?.completedAtMs,
      );
      // The official worked-for clock ends when the final assistant response
      // starts. Earlier assistant commentary is part of the work group and
      // must not extend the duration; this was the source of the recurring
      // three-to-five-second discrepancy in legacy snapshots.
      const isFinalAssistant = kind === "assistant" && finalAssistantIndexes.get(key) === index;
      const assistantStart = explicitAssistantStart !== null
        ? explicitAssistantStart
        : isFinalAssistant ? timestampMs(item?.startedAtMs, item?.startedAt, item?.createdAtMs) : null;
      if (assistantStart !== null) ends.set(key, Math.max(ends.get(key) || 0, assistantStart));
    });
    for (const [key, start] of starts) {
      if (result.has(key)) continue;
      const end = ends.get(key);
      if (end !== undefined && end >= start) result.set(key, end - start);
    }
    // Some older host snapshots keep the authoritative worked-for duration in
    // session metadata rather than repeating it on each projected item. Apply
    // it to the most recent turn only; explicit per-item values always win.
    const metadataDuration = finiteNumber(state.workedDurationMs, state.lastWorkedDurationMs);
    if (metadataDuration !== null && messages.length) {
      const keysInOrder = messages
        .filter(isRecord)
        .map((item, index) => structuredMessageTurn(item, index));
      const target = state.turnId && keysInOrder.includes(state.turnId)
        ? state.turnId
        : keysInOrder.at(-1);
      // Session metadata is authoritative over timestamp inference, but an
      // explicit per-turn/item value from the host remains the strongest
      // signal.
      if (target && !explicitKeys.has(target)) result.set(target, Math.max(0, metadataDuration));
    }
    // Older attach snapshots may expose the final-answer start separately
    // from the duration. Use it only for the corresponding most recent turn.
    const metadataFinal = timestampMs(state.finalAssistantStartedAt);
    if (metadataFinal !== null && messages.length) {
      const keyed = messages.filter(isRecord).map((item, index) => structuredMessageTurn(item, index));
      const target = state.turnId && keyed.includes(state.turnId) ? state.turnId : keyed.at(-1);
      const start = target ? starts.get(target) : null;
      if (target && start !== undefined && !explicitKeys.has(target) && !result.has(target) && metadataFinal >= start) {
        result.set(target, metadataFinal - start);
      }
    }
    return result;
  }

  function structuredActivityTiming(messages, index, turnKey, status) {
    const item = Array.isArray(messages) ? messages[index] : null;
    if (!isRecord(item)) return { durationMs: null, finishedAt: null };
    const explicitDuration = finiteNumber(item.durationMs, item.elapsedMs);
    const startedAt = timestampMs(item.startedAtMs, item.startedAt, item.createdAtMs);
    const explicitFinishedAt = timestampMs(item.completedAtMs, item.completedAt, item.finishedAtMs, item.finishedAt);
    if (explicitDuration !== null) {
      return {
        durationMs: explicitDuration,
        finishedAt: explicitFinishedAt ?? (startedAt === null ? null : startedAt + explicitDuration),
      };
    }
    if (explicitFinishedAt !== null && startedAt !== null) {
      return { durationMs: Math.max(0, explicitFinishedAt - startedAt), finishedAt: explicitFinishedAt };
    }
    // Hydrated legacy rows sometimes expose only start times. The next item in
    // the same turn is the closest end marker (and matches the official
    // projection's item interval). Never infer an end for an explicitly live
    // row, and never let a later turn's timestamp inflate this activity.
    if (status !== "inProgress" && startedAt !== null && Array.isArray(messages)) {
      for (let cursor = index + 1; cursor < messages.length; cursor += 1) {
        const next = messages[cursor];
        if (!isRecord(next)) continue;
        if (structuredMessageTurn(next, cursor) !== turnKey) break;
        const nextStartedAt = timestampMs(next.startedAtMs, next.startedAt, next.createdAtMs);
        if (nextStartedAt === null) continue;
        if (nextStartedAt >= startedAt) return {
          durationMs: nextStartedAt - startedAt,
          finishedAt: nextStartedAt,
        };
        break;
      }
    }
    return { durationMs: null, finishedAt: explicitFinishedAt };
  }

  function indexStructuredActivity(article, item, kind, turnKey, index, messages = null) {
    if (!article || !isCollapsibleKind(kind)) return null;
    const key = article.dataset.activityKey || structuredActivityKey(item, kind, turnKey, index);
    article.dataset.activityKey = key;
    article.dataset.activityKind = kind;
    const existing = state.activities.get(key);
    const turnStatus = normalizeActivityStatus(item.turnStatus, "");
    const status = structuredMessageStatus(item) || (turnStatus === "inProgress" ? "inProgress" : "completed");
    const timing = structuredActivityTiming(messages, index, turnKey, status);
    const durationMs = timing.durationMs;
    const startedAt = timestampMs(item.startedAtMs, item.startedAt, item.createdAtMs);
    const activity = existing || {
      key,
      kind,
      role: kind === "commentary"
        ? "assistant"
        : kind === "tool" || kind === "read" || kind === "edit" || kind === "subagent" ? "tool" : "system",
      messageKind: kind === "tool" || kind === "read" ? "tool" : kind,
      label: item.label || activityLabelForItem(item, kind),
      command: kind === "tool" || kind === "read" ? terminalCommandText(commandText(item)) : commandText(item),
      filePath: kind === "read" ? readPathList(item).join("\n") : "",
      cwd: (kind === "tool" || kind === "read") && typeof item.cwd === "string" ? item.cwd : "",
      shellName: (kind === "tool" || kind === "read") && typeof item.shellName === "string" && item.shellName ? item.shellName : "Shell",
      agentThreadId: kind === "subagent" ? firstString(item.agentThreadId, item.childThreadId, item.threadId) : "",
      displayName: kind === "subagent" ? firstString(item.displayName, item.agentNickname, item.agentName, item.agentPath) : "",
      objective: kind === "subagent" ? firstString(item.objective, item.prompt, item.statusMessage, item.message) : "",
      activityKind: kind === "subagent" ? firstString(item.activityKind, item.kind) : "",
      displayStatus: kind === "subagent" ? firstString(item.displayStatus, item.status) : "",
      model: kind === "subagent" ? firstString(item.model, item.modelId) : "",
      action: kind === "subagent" ? firstString(item.action, item.tool) : "",
      prompt: kind === "subagent" && item.prompt !== null ? firstString(item.prompt) : "",
      senderThreadId: kind === "subagent" ? firstString(item.senderThreadId) : "",
      receiverThreadIds: kind === "subagent"
        ? Array.isArray(item.receiverThreadIds)
          ? item.receiverThreadIds.map(String)
          : Array.isArray(item.receiverThreads) ? item.receiverThreads.map(String) : []
        : [],
      agentsStates: kind === "subagent" && isRecord(item.agentsStates) ? item.agentsStates : {},
      canInteract: item.canInteract !== false,
      exitCode: kind === "tool" || kind === "read" ? finiteNumber(item.exitCode, item.exit_code) : null,
      itemId: item.itemId === undefined || item.itemId === null ? "" : String(item.itemId),
      threadId: item.threadId || state.threadId || "",
      turnId: turnKey || "",
      startedAt: startedAt || null,
      finishedAt: null,
      durationMs: null,
      durationExplicit: finiteNumber(item.durationMs, item.elapsedMs) !== null,
      status,
      headerText: "",
      outputText: "",
      anonymous: false,
      concrete: true,
      statusOnly: false,
      article,
      body: article.querySelector(".details-body") || article.querySelector(".message-body"),
      wrapper: article.querySelector(".message-content"),
      details: article.querySelector("details"),
      summary: article.querySelector("summary"),
    };
    activity.key = key;
    activity.kind = kind;
    activity.messageKind = kind === "tool" || kind === "read" ? "tool" : kind;
    activity.itemId = item.itemId === undefined || item.itemId === null ? activity.itemId || "" : String(item.itemId);
    // Structured history is an authoritative concrete projection. If a live
    // status row occupied the same slot during reconnect, promote it here so
    // the next terminal update does not discard the hydrated item.
    activity.concrete = true;
    activity.statusOnly = false;
    activity.anonymous = false;
    activity.turnId = turnKey || activity.turnId || "";
    activity.threadId = item.threadId || activity.threadId || state.threadId || "";
    activity.label = item.label || activity.label || activityLabelForItem(item, kind);
    activity.command = kind === "tool" || kind === "read"
      ? terminalCommandText(commandText(item)) || activity.command || ""
      : commandText(item) || activity.command || "";
    if (kind === "tool" || kind === "read") {
      if (kind === "read") activity.filePath = readPathList(item).join("\n") || activity.filePath;
      activity.cwd = typeof item.cwd === "string" ? item.cwd : activity.cwd || "";
      activity.shellName = typeof item.shellName === "string" && item.shellName
        ? item.shellName
        : activity.shellName || "Shell";
      activity.exitCode = item.exitCode === undefined || item.exitCode === null
        ? activity.exitCode ?? null
        : finiteNumber(item.exitCode, item.exit_code);
    }
    if (kind === "subagent") {
      activity.agentThreadId = firstString(item.agentThreadId, item.childThreadId, item.threadId, activity.agentThreadId);
      activity.displayName = firstString(item.displayName, item.agentNickname, item.agentName, item.agentPath, activity.displayName);
      activity.objective = firstString(item.objective, item.prompt, item.statusMessage, item.message, activity.objective);
      activity.activityKind = firstString(item.activityKind, item.kind, activity.activityKind);
      activity.displayStatus = firstString(item.displayStatus, item.status, activity.displayStatus);
      activity.model = firstString(item.model, item.modelId, activity.model);
      activity.action = firstString(item.action, item.tool, activity.action);
      if (item.prompt !== null && item.prompt !== undefined) activity.prompt = firstString(item.prompt, activity.prompt);
      activity.senderThreadId = firstString(item.senderThreadId, activity.senderThreadId);
      if (Array.isArray(item.receiverThreadIds)) activity.receiverThreadIds = item.receiverThreadIds.map(String);
      else if (Array.isArray(item.receiverThreads)) activity.receiverThreadIds = item.receiverThreads.map(String);
      if (isRecord(item.agentsStates)) activity.agentsStates = item.agentsStates;
      if (item.canInteract !== undefined) activity.canInteract = item.canInteract !== false;
      if (activity.agentThreadId) article.dataset.agentThreadId = activity.agentThreadId;
      else delete article.dataset.agentThreadId;
    }
    activity.startedAt = startedAt || activity.startedAt || null;
    if (durationMs !== null || !existing) activity.durationMs = durationMs;
    activity.durationExplicit = finiteNumber(item.durationMs, item.elapsedMs) !== null;
    activity.status = status;
    activity.finishedAt = timing.finishedAt || activity.finishedAt || null;
    activity.article = article;
    activity.body = article.querySelector(".details-body") || article.querySelector(".message-body");
    activity.wrapper = article.querySelector(".message-content");
    activity.details = article.querySelector("details");
    activity.summary = article.querySelector("summary");
    activity.outputText = kind === "tool" || kind === "read" ? activityOutput(item, kind) : messageText(item);
    state.activities.set(key, activity);
    // Hydrated rows may have been created before lifecycle metadata arrived.
    // Recompute the disclosure label from the normalized status/timestamps so
    // history and live updates cannot leave stale `0ms` or generic text behind.
    renderActivityText(activity);
    refreshActivity(activity);
    return activity;
  }

  function updateStructuredMessageArticle(article, item, index, turnLastIndex, turnHasActivity = new Set(), turnWorkedDurations = new Map()) {
    const itemText = messageText(item);
    if (!itemText) return false;
    const sourceKind = historyKind(item);
    const turnKey = structuredMessageTurn(item, index);
    const kind = structuredDisplayKind(item, index, turnKey, turnLastIndex);
    const role = structuredMessageRole(item, kind);
    const status = structuredMessageStatus(item) || "completed";
    const durationMs = item.durationMs ?? item.elapsedMs;
    const duration = elapsedDuration(durationMs);
    const body = article.querySelector(".message-body");
    if (!body) return false;
    const details = article.querySelector("details");
    const wasOpen = details ? isDetailsExpanded(details) : undefined;
    article.dataset.rawText = String(itemText);
    article.dataset.turnId = turnKey;
    article.dataset.kind = kind;
    if (item.itemId !== undefined && item.itemId !== null) article.dataset.itemId = String(item.itemId);
    if (item.itemType) article.dataset.itemType = String(item.itemType);
    const agentThreadId = kind === "subagent" ? firstString(item.agentThreadId, item.childThreadId, item.threadId) : "";
    if (agentThreadId) article.dataset.agentThreadId = agentThreadId;
    else delete article.dataset.agentThreadId;
    if (item.startedAtMs || item.completedAtMs) {
      const parsed = timestampMs(item.startedAtMs ?? item.completedAtMs);
      if (parsed !== null) article.dataset.timestamp = String(parsed);
    }
    if (status) article.dataset.status = status;
    const isActivity = isCollapsibleKind(kind);
    article.classList.toggle("streaming", status === "inProgress" && !isActivity);
    renderMessageBody(body, itemText, role, isActivity ? "activity" : "history", kind);
    if (details) {
      const summary = details.querySelector("summary");
      const activitySummary = historyActivitySummary(item, kind, status, duration);
      if (summary && activitySummary) setActivitySummary(summary, activitySummary, kind);
      if (wasOpen !== undefined) setDetailsExpanded(details, wasOpen, { immediate: true, preserve: false });
    }
    const finalAssistant = sourceKind === "assistant" && role === "assistant" && kind === "assistant"
      && (item.phase === "final_answer" || item.phase === "final-answer" || !isAssistantCommentary(item, index, turnKey, turnLastIndex));
    if (finalAssistant && status !== "inProgress" && !article.classList.contains("streaming") && !article.querySelector(".message-actions")) {
      addMessageActions(article, true);
    }
    const terminalTurn = ["completed", "complete", "done", "failed", "error", "interrupted", "cancelled", "canceled"]
      .includes(String(item.turnStatus || "").replace(/[\s-]+/g, "_").toLowerCase());
    const workedDuration = turnWorkedDurations.get(turnKey)
      ?? workedDurationFor(item, null);
    if (finalAssistant && (terminalTurn || workedDuration !== null || (durationMs !== undefined && status !== "inProgress"))) {
      appendTurnDivider(turnKey, item.turnStatus || status || "completed", workedDuration ?? durationMs, article);
    }
    return true;
  }

  function reconcileStructuredOutput(text, structuredMessages) {
    const output = $("output");
    if (!output || !Array.isArray(structuredMessages) || !structuredMessages.length) return false;
    const articles = [...output.querySelectorAll(".message[data-structured-key]")];
    if (articles.length !== structuredMessages.length) return false;
    const keys = structuredMessages.map((item, index) => structuredMessageKey(item, index));
    if (articles.some((article, index) => article.dataset.structuredKey !== keys[index])) return false;
    const follow = shouldFollowOutput(output);
      const turnLastIndex = structuredTurnFinalAssistantIndexes(structuredMessages);
      const turnHasActivity = new Set();
      const turnWorkedDurations = structuredTurnWorkedDurations(structuredMessages);
    structuredMessages.forEach((item, index) => {
      const turnKey = structuredMessageTurn(item, index);
      if (isCollapsibleKind(structuredDisplayKind(item, index, turnKey, turnLastIndex))) turnHasActivity.add(turnKey);
    });
    for (const [index, item] of structuredMessages.entries()) {
      if (!updateStructuredMessageArticle(articles[index], item, index, turnLastIndex, turnHasActivity, turnWorkedDurations)) return false;
      const turnKey = structuredMessageTurn(item, index);
      const kind = structuredDisplayKind(item, index, turnKey, turnLastIndex);
      if (isCollapsibleKind(kind)) {
        indexStructuredActivity(articles[index], item, kind, turnKey, index, structuredMessages);
      }
    }
    reconcileTurnDividers();
    expandLatestTurnActivity();
    if (typeof text === "string") output.dataset.outputTail = text;
    state.structuredMessages = structuredMessages.slice();
    state.outputSynced = true;
    if (state.turnId && state.turnStartedAt !== null && ["active", "waiting"].includes(state.turnStatus)
      && [...output.querySelectorAll(".message.activity")].some((article) => article.dataset.turnId === state.turnId)) {
      ensureLiveTurnDivider(state.turnId);
    }
    if (follow) scrollOutput(output, true);
    updateScrollToBottom(output);
    return true;
  }

  function replaceOutput(text, structuredMessages) {
    const output = $("output");
    const preserveBottom = !state.outputSynced || !output || shouldFollowOutput(output);
    const preservedDistanceFromBottom = output
      ? Math.max(0, output.scrollHeight - output.scrollTop - output.clientHeight)
      : 0;
    const pendingUserText = state.pendingUserText;
    const snapshotHasPendingUser = Boolean(pendingUserText && (
      text.includes(`> ${pendingUserText}`)
      || (Array.isArray(structuredMessages) && structuredMessages.some((item) => {
        const kind = historyKind(item);
        return kind === "user" && comparableText(messageText(item)) === comparableText(pendingUserText);
      }))
    ));
    renderEmptyOutput();
    if (typeof text === "string") output.dataset.outputTail = text;
    state.structuredMessages = Array.isArray(structuredMessages) ? structuredMessages.slice() : [];
    if (Array.isArray(structuredMessages) && structuredMessages.length) {
      // Older follower snapshots may omit the top-level subagents projection.
      // Derive the small panel model from the same collaboration items used by
      // the transcript so those agents remain discoverable after reconnect.
      const suppliedSubagents = state.subagents;
      const derivedSubagents = deriveSubagentsFromMessages(structuredMessages);
      if (derivedSubagents.length) {
        state.subagents = mergeSubagentProjections(derivedSubagents, suppliedSubagents);
        renderSubagents();
      } else if (state.subagents.length) {
        // A complete snapshot with no collaboration items is authoritative for
        // the transcript. Do not carry a finished agent from the previous
        // thread into the new composer.
        state.subagents = [];
        renderSubagents();
      }
      const turnLastIndex = structuredTurnFinalAssistantIndexes(structuredMessages);
      const turnHasActivity = new Set();
      const turnWorkedDurations = structuredTurnWorkedDurations(structuredMessages);
      let previousTurnKey = "";
      structuredMessages.forEach((item, index) => {
        const key = structuredMessageTurn(item, index);
        if (isCollapsibleKind(structuredDisplayKind(item, index, key, turnLastIndex))) turnHasActivity.add(key);
      });
      structuredMessages.forEach((item, index) => {
        const itemText = messageText(item);
        if (!itemText) return;
        const sourceKind = historyKind(item);
        const turnKey = structuredMessageTurn(item, index);
        const kind = structuredDisplayKind(item, index, turnKey, turnLastIndex);
        const role = structuredMessageRole(item, kind);
        const status = structuredMessageStatus(item) || "completed";
        const durationMs = item.durationMs ?? item.elapsedMs;
        const duration = elapsedDuration(durationMs);
        const timestamp = item.startedAtMs ?? item.completedAtMs;
        if (role === "user" || role === "assistant") {
          appendDateSeparator(timestamp, {
            role,
            turnId: turnKey,
            turnStart: turnKey !== previousTurnKey,
            breaksPreviousAdjacency: item.breaksPreviousAdjacency === true,
          });
        }
        const finalAssistant = sourceKind === "assistant" && role === "assistant" && kind === "assistant"
          && (item.phase === "final_answer" || item.phase === "final-answer" || !isAssistantCommentary(item, index, turnKey, turnLastIndex));
        const terminalTurn = ["completed", "complete", "done", "failed", "error", "interrupted", "cancelled", "canceled"]
          .includes(String(item.turnStatus || "").replace(/[\s-]+/g, "_").toLowerCase());
        const workedDuration = turnWorkedDurations.get(turnKey) ?? workedDurationFor(item, null);
        const activitySummary = historyActivitySummary(item, kind, status, duration) || undefined;
        const collapsible = isCollapsibleKind(kind);
        const activityKey = collapsible ? structuredActivityKey(item, kind, turnKey, index) : "";
        const message = appendMessage(itemText, role, collapsible ? "activity" : "history", "", {
          kind,
          status,
          label: item.label || (kind === "reasoning" ? "思考" : kind === "plan" ? "计划" : kind === "edit" ? "文件变更" : kind === "read" ? "读取文件" : kind === "tool" ? "工具输出" : kind === "subagent" ? "子代理" : kind === "commentary" ? "工作说明" : ""),
          summary: activitySummary,
          command: item.command || item.commandLine,
          turnId: turnKey,
          structuredKey: structuredMessageKey(item, index),
          activityKey,
          itemId: item.itemId,
          itemType: item.itemType,
          agentThreadId: kind === "subagent" ? firstString(item.agentThreadId, item.childThreadId, item.threadId) : "",
          timestamp,
          showTimestamp: role === "user" || role === "assistant",
          showActions: role === "user" || (role === "assistant" && finalAssistant && kind !== "commentary"),
          collapsible,
          open: kind === "commentary" || (status === "inProgress" && (kind === "reasoning" || kind === "plan")),
        });
        if (message && collapsible) indexStructuredActivity(message.article, item, kind, turnKey, index, structuredMessages);
        if (finalAssistant && (terminalTurn || workedDuration !== null || (durationMs !== undefined && status !== "inProgress"))) {
          appendTurnDivider(turnKey, item.turnStatus || status || "completed", workedDuration ?? durationMs, message?.article || null);
        }
        previousTurnKey = turnKey;
      });
      reconcileTurnDividers();
      expandLatestTurnActivity();
    } else if (text) {
      // The attach adapter prefixes user items with `> ` and separates items
      // with blank lines. Use that stable marker to recreate the two sides of
      // the conversation without interpreting arbitrary Markdown as HTML.
      const chunks = text.split(/\n{2,}/).map((chunk) => chunk.trim()).filter(Boolean);
      let expectUser = true;
      for (const chunk of chunks) {
        const markedUser = chunk.startsWith("> ");
        if (markedUser && expectUser) {
          appendMessage(chunk.slice(2), "user", "history", "", { kind: "user" });
          expectUser = false;
        } else {
          appendMessage(chunk, "assistant", "history", "", { kind: "assistant" });
          expectUser = true;
        }
      }
    }
    if (pendingUserText && !snapshotHasPendingUser) {
      appendDateSeparator(Date.now(), { role: "user", force: true });
      appendMessage(pendingUserText, "user", "streaming", "", { kind: "user", timestamp: Date.now(), showTimestamp: true });
    }
    state.pendingUserText = snapshotHasPendingUser ? "" : pendingUserText;
    state.outputSynced = true;
    if (state.turnId && state.turnStartedAt !== null && ["active", "waiting"].includes(state.turnStatus)
      && [...output.querySelectorAll(".message.activity")].some((article) => article.dataset.turnId === state.turnId)) {
      ensureLiveTurnDivider(state.turnId);
    }
    if (preserveBottom) scrollOutput(output, true);
    else if (output) {
      // Rebuilding a long history changes scrollHeight. Preserve the user's
      // distance from the bottom just like the official thread scroll layout,
      // so a background sync does not yank them away from the message they are
      // reading.
      output.scrollTop = Math.max(0, output.scrollHeight - output.clientHeight - preservedDistanceFromBottom);
      updateScrollToBottom(output);
    }
  }

  function applyStructuredMessagesPatch(patch) {
    if (!isRecord(patch) || !Array.isArray(state.structuredMessages) || !Array.isArray(patch.messages)) return null;
    const start = Number(patch.start);
    const deleteCount = Number(patch.deleteCount);
    if (!Number.isInteger(start) || start < 0 || start > state.structuredMessages.length
      || !Number.isInteger(deleteCount) || deleteCount < 0
      || start + deleteCount > state.structuredMessages.length) return null;
    return [
      ...state.structuredMessages.slice(0, start),
      ...patch.messages,
      ...state.structuredMessages.slice(start + deleteCount),
    ];
  }

  function appendOutputChunk(text, stream = "codex", context = {}) {
    if (!text) return;
    const output = $("output");
    const follow = shouldFollowOutput(output);
    const normalizedStream = String(stream || "codex").toLowerCase();
    if (normalizedStream === "codex") {
      const userItem = text.match(/^(?:\n{2,})?> ([^\n]+)\n?$/);
      if (userItem) {
        const userText = userItem[1].trim();
        const eventTurn = context.turnId || state.turnId || "";
        finishAssistantStream();
        appendDateSeparator(context.timestamp || Date.now(), { role: "user", turnId: eventTurn, turnStart: true });
        if (!hasRenderedMessage(userText, eventTurn, "user")) {
          appendMessage(userText, "user", "history", "", {
            kind: "user",
            turnId: eventTurn,
            timestamp: context.timestamp || Date.now(),
            showTimestamp: true,
          });
        }
        state.pendingUserText = "";
        state.outputSynced = true;
        return;
      }
    }
    if (normalizedStream === "codex" && state.pendingUserText) {
      const marker = `> ${state.pendingUserText}`;
      if (text.includes(marker)) {
        text = text.replace(marker, "").replace(/^\n{1,2}/, "");
        state.pendingUserText = "";
        if (!text) return;
      }
    }

    const eventTurnId = context.turnId || state.turnId || "";
    if (state.outputSynced && eventTurnId && normalizedStream === "codex"
      && hasRenderedCompletedMessage(text, eventTurnId, "assistant")) {
      // A host replay often sends the same output delta immediately after an
      // authoritative snapshot. Do not append a second assistant bubble.
      state.pendingUserText = "";
      return;
    }

    const inferredActivity = normalizedStream === "reasoning"
      ? "thinking"
      : normalizedStream === "read" || normalizedStream === "reading"
        ? "reading"
        : normalizedStream === "stdout" || normalizedStream === "stderr"
          ? "running"
          : "generating";
    if (!state.currentActivity || ["idle", "completed", "failed", "interrupted"].includes(state.currentActivity)) {
      state.currentActivity = inferredActivity;
    }
    if (state.turnStartedAt === null) startTurnClock(context.turnId || state.turnId, null, null);
    updateLiveActivity(
      state.currentActivity,
      state.currentActivityStartedAt || state.turnStartedAt,
      null,
      [],
      context.turnId || state.turnId,
    );
    const activityText = statusActivityLabel(state.currentActivity) || t("正在生成");
    const outputElapsed = elapsedDuration(Math.max(0, Date.now() - (state.turnStartedAt || Date.now())));
    setConversationStatus(outputElapsed ? `${activityText} · ${outputElapsed}` : activityText, "active");

    // Lifecycle notifications create the canonical row first. Output deltas
    // from app-server versions that omit itemId are attached to the newest
    // running row of the corresponding kind.
    const streamKind = normalizedStream === "reasoning"
      ? "reasoning"
      : normalizedStream === "read" || normalizedStream === "reading"
        ? "read"
        : normalizedStream === "stdout" || normalizedStream === "stderr"
          ? "tool"
          : context.kind || "";
    const activity = streamKind
      ? latestRunningActivity(streamKind, context)
      : null;
    if (activity) {
      appendActivityChunk(activity, text);
      state.activeAssistantBody = activity.body;
      state.activeAssistantStream = normalizedStream;
      state.activeAssistantText = activity.outputText;
      state.activeAssistantActivityKey = activity.key;
      if (follow) scrollOutput(output, true);
      state.outputSynced = true;
      return;
    }
    if (state.activeAssistantStream !== normalizedStream) finishAssistantStream();
    if (!state.activeAssistantBody && normalizedStream === "codex") text = text.replace(/^\n{2,}/, "");
    if (!state.activeAssistantBody || !output.contains(state.activeAssistantBody)) {
      const role = normalizedStream === "stderr"
        ? "error"
        : normalizedStream === "stdout" || normalizedStream === "read" || normalizedStream === "reading"
          ? "tool"
          : normalizedStream === "reasoning"
            ? "system"
            : "assistant";
      const label = normalizedStream === "reasoning" ? "思考"
        : normalizedStream === "stdout" ? "命令输出"
          : normalizedStream === "read" || normalizedStream === "reading" ? "读取文件" : "";
      const kind = normalizedStream === "reasoning" ? "reasoning"
        : normalizedStream === "stdout" ? "tool"
          : normalizedStream === "read" || normalizedStream === "reading" || context.kind === "read" ? "read" : "assistant";
      let message;
      if (kind === "reasoning" || kind === "tool" || kind === "read") {
        state.activitySequence += 1;
        const key = `stream:${state.threadId || "thread"}:${state.turnId || "turn"}:${normalizedStream}:${state.activitySequence}`;
        const streamActivity = ensureActivity(key, {
          kind,
          label: label || (kind === "tool" ? "工具输出" : kind === "read" ? "读取文件" : "思考"),
          threadId: state.threadId,
          turnId: eventTurnId || state.turnId,
          status: "inProgress",
          anonymous: true,
          concrete: true,
        });
        message = streamActivity && { article: streamActivity.article, content: streamActivity.body };
        state.activeAssistantActivityKey = streamActivity?.key || null;
      } else {
        appendDateSeparator(context.timestamp || Date.now(), { role: "assistant", turnId: eventTurnId, turnStart: false });
        message = appendMessage("", role, "streaming", "", {
          kind,
          label,
          turnId: eventTurnId,
          timestamp: context.timestamp || Date.now(),
          showTimestamp: true,
          showActions: false,
          collapsible: false,
        });
      }
      state.activeAssistantBody = message?.content || null;
      state.activeAssistantStream = normalizedStream;
      state.activeAssistantText = "";
    }
    if (state.activeAssistantActivityKey) {
      const streamActivity = state.activities.get(state.activeAssistantActivityKey);
      if (streamActivity) {
        appendActivityChunk(streamActivity, text);
        state.activeAssistantText = streamActivity.outputText;
        if (follow) scrollOutput(output, true);
        state.outputSynced = true;
        return;
      }
    }
    if (state.activeAssistantBody) {
      state.activeAssistantText += text;
      const article = state.activeAssistantBody.closest(".message");
      const role = article?.classList.contains("system") ? "system" : article?.classList.contains("error") ? "error" : "assistant";
      const kind = article?.dataset.kind || "assistant";
      renderMessageBody(state.activeAssistantBody, state.activeAssistantText, role, "streaming", kind);
      if (article) article.dataset.rawText = state.activeAssistantText;
      article?.classList.add("streaming");
    }
    if (follow) scrollOutput(output, true);
    state.outputSynced = true;
  }

  function finishAssistantStream() {
    const article = state.activeAssistantBody?.closest(".message");
    article?.classList.remove("streaming");
    if (article?.classList.contains("assistant") && !article.querySelector(".message-actions")) addMessageActions(article, true);
    if (article?.dataset.kind === "reasoning") {
      const details = article.querySelector("details");
      if (details) setDetailsExpanded(details, false);
    }
    const activity = state.activeAssistantActivityKey
      ? state.activities.get(state.activeAssistantActivityKey)
      : null;
    if (activity?.anonymous && isRunningActivity(activity)) finishActivity(activity, "completed");
    state.activeAssistantBody = null;
    state.activeAssistantStream = null;
    state.activeAssistantText = "";
    state.activeAssistantActivityKey = null;
  }

  function eventBelongsToCurrentTurn(payload) {
    if (!payload || typeof payload !== "object") return true;
    const threadId = eventThreadId(payload);
    const turnId = eventTurnId(payload);
    if (threadId && state.threadId && threadId !== state.threadId) return false;
    if (turnId && state.turnId && turnId !== state.turnId) return false;
    if (turnId && state.retiredTurnIds.has(turnId)) return false;
    return true;
  }

  function eventMessage(payload) {
    if (!payload || typeof payload !== "object") return "未知错误";
    const candidates = [
      payload.message,
      payload.error?.message,
      payload.error,
      payload.params?.message,
      payload.params?.error,
      payload.text,
    ];
    const value = candidates.find((entry) => typeof entry === "string" && entry.trim());
    if (value) return value;
    try { return JSON.stringify(payload); } catch { return "未知错误"; }
  }

  function setAttachMode(value) {
    state.attachMode = Boolean(value);
    document.body.classList.toggle("attach-mode", state.attachMode);
    $("sessionMode").textContent = t(state.attachMode
      ? "已附着 VS Code 当前 Codex 会话；输入、输出和授权都回到同一个会话。"
      : "当前为独立 app-server 模式。");
    $("startThreadButton").textContent = t(state.attachMode ? "已附着现有会话" : "启动新 thread");
    const modeLabel = $("modeLabel");
    if (modeLabel) modeLabel.textContent = t(state.attachMode ? "本地模式" : "独立模式");
    const popoverMode = $("popoverMode");
    if (popoverMode) popoverMode.textContent = t(state.attachMode ? "本地模式" : "独立模式");
  }

  function firstString(...values) {
    return values.find((value) => typeof value === "string" && value.trim())?.trim() || "";
  }

  // Unlike firstString, settings projections need to preserve an explicit
  // null. Codex uses null for a model with no reasoning selector; treating it
  // as "missing" leaves the browser showing the previous turn's effort.
  function firstDefined(...values) {
    return values.find((value) => value !== undefined);
  }

  function modelDisplayName(model, effort) {
    const value = String(model || "").trim();
    if (!value) return "";
    const words = value
      .replace(/^gpt[-_]/i, "")
      .replace(/[-_]+/g, " ")
      .split(/\s+/)
      .filter(Boolean)
      .map((word) => /^\d+(?:\.\d+)*$/.test(word) ? word : `${word[0].toUpperCase()}${word.slice(1)}`);
    const effortValue = String(effort || "").trim();
    if (effortValue && !words.some((word) => word.toLowerCase() === effortValue.toLowerCase())) {
      words.push(`${effortValue[0].toUpperCase()}${effortValue.slice(1)}`);
    }
    return words.join(" ");
  }

  // Keep this tiny fallback aligned with the model power choices shipped in
  // the installed official webview. A host that exposes `availableModels`
  // always wins; these entries only make the picker useful while an older
  // private IPC build has no model/list projection.
  const FALLBACK_MODELS = [
    { model: "gpt-5.6-sol", displayName: "5.6 Sol", description: "通用 Codex 模型", efforts: ["low", "medium", "high", "xhigh"] },
    { model: "gpt-5.6-terra", displayName: "5.6 Terra", description: "平衡速度与推理", efforts: ["low", "medium", "high", "xhigh"] },
  ];
  // Labels used by the shipped Work composer. The compact trigger shows the
  // model plus its selected reasoning preset (for example `5.6 Sol 标准`).
  const EFFORT_LABELS = { none: "默认", minimal: "极低", low: "轻度", medium: "标准", high: "深度", xhigh: "极高", max: "最大", ultra: "Ultra" };

  // The compact Work picker is a power control. Keep the mapping deterministic
  // so keyboard/range input still goes through the same effort protocol used by
  // the advanced picker.
  function modelPowerOptions(model) {
    const efforts = Array.isArray(model?.efforts) ? model.efforts.filter(Boolean) : [];
    return efforts.map((effort) => ({ effort, label: t(EFFORT_LABELS[effort] || effort) }));
  }

  function normalizeModelOption(value) {
    if (typeof value === "string") {
      const fallback = FALLBACK_MODELS.find((entry) => entry.model === value);
      return fallback ? { ...fallback } : { model: value, displayName: modelDisplayName(value), description: "可用模型", efforts: [] };
    }
    if (!isRecord(value)) return null;
    const model = firstString(value.model, value.id, value.slug, value.name);
    if (!model) return null;
    const hasSupported = Array.isArray(value.supportedReasoningEfforts) || Array.isArray(value.efforts);
    const supported = Array.isArray(value.supportedReasoningEfforts)
      ? value.supportedReasoningEfforts.map((entry) => typeof entry === "string" ? entry : firstString(entry?.reasoningEffort, entry?.effort)).filter(Boolean)
      : Array.isArray(value.efforts) ? value.efforts.filter((entry) => typeof entry === "string") : [];
    const fallback = FALLBACK_MODELS.find((entry) => entry.model === model);
    return {
      model,
      displayName: firstString(value.displayName, value.label, fallback?.displayName, modelDisplayName(model)),
      description: firstString(value.description, fallback?.description),
      // Unknown catalog entries must not inherit a fabricated effort list. The
      // official advanced picker disables the power controls until the host
      // reports capabilities for that model.
      efforts: hasSupported ? supported : fallback?.efforts || [],
      defaultReasoningEffort: firstString(value.defaultReasoningEffort, value.defaultEffort, fallback?.efforts?.[1]),
      hidden: value.hidden === true,
    };
  }

  function normalizedModelOptions() {
    let source;
    if (state.availableModels.length) source = state.availableModels;
    else if (state.currentModel && FALLBACK_MODELS.some((entry) => entry.model === state.currentModel)) source = FALLBACK_MODELS;
    else if (state.currentModel) source = [state.currentModel];
    else return [];
    const seen = new Set();
    const result = [];
    for (const entry of source) {
      const option = normalizeModelOption(entry);
      if (!option || option.hidden || seen.has(option.model)) continue;
      seen.add(option.model);
      result.push(option);
    }
    if (state.currentModel && !seen.has(state.currentModel)) {
      result.unshift(normalizeModelOption(state.currentModel));
    }
    return result;
  }

  function currentModelOption() {
    return normalizedModelOptions().find((entry) => entry.model === state.currentModel) || normalizedModelOptions()[0];
  }

  function renderModelPicker() {
    const button = $("modelPickerButton");
    const label = $("modelLabel");
    const effortLabelNode = $("modelEffortLabel");
    const menu = $("modelMenu");
    const modelOptions = $("modelOptions");
    const effortOptions = $("effortOptions");
    const powerView = $("modelPowerView");
    const advancedView = $("modelAdvancedView");
    const advancedToggle = $("modelAdvancedToggle");
    const powerSlider = $("modelPowerSlider");
    const powerValue = $("modelPowerValue");
    if (!button || !label || !effortLabelNode || !menu || !modelOptions || !effortOptions) return;
    const current = currentModelOption();
    if (!current) {
      button.hidden = true;
      return;
    }
    button.hidden = false;
    const selectedEffort = state.currentEffort === null
      ? ""
      : state.currentEffort || current.defaultReasoningEffort || current.efforts?.[1] || current.efforts?.[0] || "";
    const effortLabel = selectedEffort ? t(EFFORT_LABELS[selectedEffort] || selectedEffort) : "";
    label.textContent = current.displayName || modelDisplayName(current.model);
    effortLabelNode.textContent = effortLabel;
    effortLabelNode.hidden = !effortLabel;
    const settingsModelValue = $("settingsModelValue");
    if (settingsModelValue) settingsModelValue.textContent = [label.textContent, effortLabel].filter(Boolean).join(" ") || t("默认");
    const triggerLabel = [label.textContent, effortLabel].filter(Boolean).join(" ");
    button.setAttribute("aria-label", uiLocale() === "en-US"
      ? `Current model: ${triggerLabel}; change model`
      : `当前模型 ${triggerLabel}，切换模型`);
    button.setAttribute("title", uiLocale() === "en-US"
      ? `Change model (current: ${triggerLabel})`
      : `切换模型（当前 ${triggerLabel}）`);
    modelOptions.replaceChildren();
    for (const option of normalizedModelOptions()) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "model-option";
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", String(option.model === state.currentModel));
      item.dataset.model = option.model;
      const name = document.createElement("span");
      name.className = "model-option-name";
      name.textContent = option.displayName || option.model;
      const check = document.createElement("span");
      check.className = "model-option-check";
      check.textContent = "✓";
      const description = document.createElement("span");
      description.className = "model-option-description";
      description.textContent = option.description ? t(option.description) : option.model;
      item.append(name, check, description);
      item.addEventListener("click", () => selectModel(option.model));
      modelOptions.append(item);
    }
    effortOptions.replaceChildren();
    const efforts = current.efforts?.length ? [...current.efforts] : [];
    // Ultra is capability-gated in the official picker. Preserve it when the
    // host explicitly reports the current setting, but do not advertise it to
    // every fallback session.
    if (state.currentEffort === "ultra" && current.model === "gpt-5.6-sol" && !efforts.includes("ultra")) efforts.push("ultra");
    const selectedMenuEffort = state.currentEffort === null
      ? ""
      : state.currentEffort || current.defaultReasoningEffort || efforts[1] || efforts[0] || "";
    if (!efforts.length) {
      const empty = document.createElement("div");
      empty.className = "effort-empty";
      empty.textContent = t("此模型使用默认推理强度");
      effortOptions.append(empty);
    }
    for (const effort of efforts) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "effort-option";
      item.setAttribute("role", "option");
      item.setAttribute("aria-selected", String(effort === selectedMenuEffort));
      item.dataset.effort = effort;
      const name = document.createElement("span");
      name.textContent = t(EFFORT_LABELS[effort] || effort);
      const check = document.createElement("span");
      check.className = "effort-option-check";
      check.textContent = "✓";
      item.append(name, check);
      item.title = effort;
      item.addEventListener("click", () => selectEffort(effort));
      effortOptions.append(item);
    }
    const powerOptions = modelPowerOptions(current);
    if (powerView && advancedView) {
      powerView.hidden = state.modelAdvancedOpen;
      advancedView.hidden = !state.modelAdvancedOpen;
    }
    if (advancedToggle) {
      advancedToggle.textContent = t(state.modelAdvancedOpen ? "简洁" : "高级");
      advancedToggle.setAttribute("aria-label", t(state.modelAdvancedOpen ? "返回简洁模型选择" : "显示高级模型选项"));
    }
    if (powerSlider) {
      const hasPower = powerOptions.length > 0;
      powerSlider.disabled = !hasPower;
      powerSlider.min = "0";
      powerSlider.max = String(Math.max(0, powerOptions.length - 1));
      const selectedPower = powerOptions.findIndex((entry) => entry.effort === selectedMenuEffort);
      powerSlider.value = String(selectedPower >= 0 ? selectedPower : Math.min(1, Math.max(0, powerOptions.length - 1)));
      powerSlider.setAttribute("aria-valuetext", powerOptions[Number(powerSlider.value)]?.label || t("默认"));
      powerSlider.style.setProperty("--power-position", powerOptions.length > 1
        ? `${Number(powerSlider.value) / (powerOptions.length - 1) * 100}%`
        : "0%");
      if (powerValue) powerValue.textContent = powerOptions[Number(powerSlider.value)]?.label || t("默认");
    } else if (powerValue) powerValue.textContent = "";
    $("modelPicker")?.toggleAttribute("data-pending", state.modelUpdatePending);
    renderControlMode();
  }

  function setModelMenu(open) {
    const button = $("modelPickerButton");
    const menu = $("modelMenu");
    if (!button || !menu) return;
    const next = Boolean(open) && threadSettingsAllowed() && !state.modeSwitching && !state.sessionSwitching;
    menu.hidden = !next;
    button.setAttribute("aria-expanded", String(next));
    if (next) {
      state.modelAdvancedOpen = false;
      renderModelPicker();
    }
  }

  function setPermissionMenu(open) {
    const button = $("permissionChip");
    const menu = $("permissionMenu");
    if (!button || !menu) return;
    const next = Boolean(open) && threadSettingsAllowed() && !state.modeSwitching && !state.sessionSwitching;
    menu.hidden = !next;
    button.setAttribute("aria-expanded", String(next));
    if (next) renderPermissionMenu();
  }

  function permissionSandboxLabel(value) {
    const label = {
      "read-only": "只读",
      "workspace-write": "工作区写入",
      "danger-full-access": "完全访问",
    }[normalizeSandboxName(value)] || String(value || "工作区写入");
    return t(label);
  }

  function renderPermissionMenu() {
    const menu = $("permissionMenu");
    const chip = $("permissionChip");
    const label = $("permissionLabel");
    if (!menu || !chip || !label) return;
    const mode = state.sandboxPolicy === "danger-full-access" && state.approvalPolicy === "never"
      ? "full"
      : state.sandboxPolicy === "read-only"
        ? "readonly"
        : state.approvalPolicy === "untrusted" || state.approvalPolicy === "never" ? "auto" : "ask";
    label.textContent = t({
      ask: "需要时询问",
      auto: "由 Codex 审批",
      full: "完全访问",
      readonly: "只读",
    }[mode]);
    const settingsPermissionValue = $("settingsPermissionValue");
    if (settingsPermissionValue) settingsPermissionValue.textContent = label.textContent;
    chip.setAttribute("aria-label", uiLocale() === "en-US"
      ? `Change permissions; current: ${label.textContent}`
      : `修改权限，当前为${label.textContent}`);
    chip.setAttribute("title", uiLocale() === "en-US"
      ? `Change permissions (current: ${label.textContent})`
      : `修改权限（当前：${label.textContent}）`);
    menu.querySelectorAll("[data-permission-mode]").forEach((item) => {
      item.setAttribute("aria-checked", String(item.dataset.permissionMode === mode));
    });
  }

  function selectPermissionMode(mode) {
    if (!threadSettingsAllowed()) {
      setConversationStatus("当前模式不支持修改会话设置", "warning");
      return;
    }
    if (mode === "custom") {
      setPermissionMenu(false);
      setConversationStatus("自定义权限由 config.toml 管理", "ready");
      return;
    }
    if (mode === "full" && !(state.sandboxPolicy === "danger-full-access" && state.approvalPolicy === "never")) {
      const confirm = $("permissionConfirm");
      if (confirm) {
        confirm.hidden = false;
        confirm.dataset.pendingMode = mode;
        setPermissionMenu(false);
        $("permissionConfirmAccept")?.focus();
      }
      return;
    }
    applyPermissionMode(mode);
  }

  function applyPermissionMode(mode) {
    if (!threadSettingsAllowed()) return;
    const presets = {
      ask: { sandboxPolicy: "workspace-write", approvalPolicy: "on-request" },
      auto: { sandboxPolicy: "workspace-write", approvalPolicy: "untrusted" },
      full: { sandboxPolicy: "danger-full-access", approvalPolicy: "never" },
      readonly: { sandboxPolicy: "read-only", approvalPolicy: "on-request" },
    };
    const preset = presets[mode];
    if (!preset) return;
    state.sandboxPolicy = preset.sandboxPolicy;
    state.approvalPolicy = preset.approvalPolicy;
    const sandboxInput = $("sandboxInput");
    const approvalInput = $("approvalInput");
    if (sandboxInput && [...sandboxInput.options].some((option) => option.value === state.sandboxPolicy)) sandboxInput.value = state.sandboxPolicy;
    if (approvalInput && [...approvalInput.options].some((option) => option.value === state.approvalPolicy)) approvalInput.value = state.approvalPolicy;
    renderPermissionMenu();
    updateThreadSettings(undefined, undefined, {
      sandboxPolicy: state.sandboxPolicy,
      approvalPolicy: state.approvalPolicy,
      permissions: state.sandboxPolicy === "read-only"
        ? ":read-only"
        : state.sandboxPolicy === "danger-full-access" ? ":danger-full-access" : ":workspace",
      approvalsReviewer: "user",
    });
    setPermissionMenu(false);
  }

  function selectPermissionSetting(kind, value) {
    if (!threadSettingsAllowed()) return;
    if (!value) return;
    if (kind === "sandbox") {
      state.sandboxPolicy = normalizeSandboxName(value);
      const input = $("sandboxInput");
      if (input && [...input.options].some((option) => option.value === state.sandboxPolicy)) input.value = state.sandboxPolicy;
    } else {
      state.approvalPolicy = String(value);
      const input = $("approvalInput");
      if (input && [...input.options].some((option) => option.value === state.approvalPolicy)) input.value = state.approvalPolicy;
    }
    renderPermissionMenu();
    updateThreadSettings(undefined, undefined, {
      sandboxPolicy: state.sandboxPolicy,
      approvalPolicy: state.approvalPolicy,
      permissions: state.sandboxPolicy === "read-only"
        ? ":read-only"
        : state.sandboxPolicy === "danger-full-access" ? ":danger-full-access" : ":workspace",
      approvalsReviewer: "user",
    });
    setPermissionMenu(false);
  }

  function normalizeUsage(value) {
    if (!isRecord(value)) return null;
    const source = isRecord(value.contextWindow) ? value.contextWindow : isRecord(value.context) ? value.context : value;
    const total = isRecord(value.total) ? value.total : isRecord(source.total) ? source.total : {};
    const last = isRecord(value.last) ? value.last : isRecord(source.last) ? source.last : {};
    const breakdownTotal = (entry) => {
      if (!isRecord(entry)) return null;
      const explicit = finiteNumber(entry.totalTokens, entry.total_tokens, entry.tokens, entry.used, entry.inputTokens);
      if (explicit !== null) return explicit;
      const parts = [
        finiteNumber(entry.inputTokens, entry.input_tokens),
        finiteNumber(entry.cachedInputTokens, entry.cached_input_tokens),
        finiteNumber(entry.cacheWriteInputTokens, entry.cache_write_input_tokens),
        finiteNumber(entry.outputTokens, entry.output_tokens),
        finiteNumber(entry.reasoningOutputTokens, entry.reasoning_output_tokens),
      ].filter((entry) => entry !== null);
      return parts.length ? parts.reduce((sum, entry) => sum + entry, 0) : null;
    };
    const used = finiteNumber(
      source.used,
      source.usedTokens,
      source.inputTokens,
      source.input_tokens,
      source.tokensUsed,
      source.totalTokens,
      value.usedTokens,
      value.inputTokens,
      breakdownTotal(last),
      breakdownTotal(total),
    );
    const limit = finiteNumber(
      value.modelContextWindow,
      value.model_context_window,
      source.limit,
      source.max,
      source.maxTokens,
      typeof source.contextWindow === "number" ? source.contextWindow : null,
      value.limit,
      value.maxTokens,
    );
    const remaining = finiteNumber(source.remaining, source.remainingTokens, value.remainingTokens);
    const percent = finiteNumber(source.percent, source.percentage, value.percent, value.percentage);
    if (used === null && limit === null && remaining === null && percent === null) return null;
    const computedPercent = percent !== null
      ? Math.max(0, Math.min(100, percent))
      : used !== null && limit !== null && limit > 0 ? Math.max(0, Math.min(100, used / limit * 100))
        : used !== null && remaining !== null && used + remaining > 0 ? used / (used + remaining) * 100 : 0;
    const clampedUsed = used !== null && limit !== null && limit > 0 ? Math.min(used, limit) : used;
    const computedRemaining = remaining !== null
      ? remaining
      : clampedUsed !== null && limit !== null ? Math.max(limit - clampedUsed, 0) : null;
    return {
      used: clampedUsed,
      limit,
      remaining: computedRemaining,
      percent: computedPercent,
      totalTokens: breakdownTotal(total),
      lastTokens: breakdownTotal(last),
    };
  }

  function renderUsage() {
    const picker = $("usagePicker");
    const button = $("usageButton");
    const label = $("usageLabel");
    const ring = $("usageRing");
    const summary = $("usageSummary");
    const details = $("usageDetails");
    const bar = $("usageMeterBar");
    if (!picker || !button || !label || !ring || !summary || !details || !bar) return;
    const usage = normalizeUsage(state.tokenUsage);
    picker.hidden = !usage;
    if (!usage) return;
    const percent = Math.round(usage.percent);
    label.textContent = `${percent}%`;
    ring.style.setProperty("--usage-percent", `${percent}%`);
    ring.dataset.level = percent >= 90 ? "critical" : percent >= 70 ? "warning" : "normal";
    const remainingPercent = Math.max(0, 100 - percent);
    button.title = uiLocale() === "en-US"
      ? `Context used ${percent}% (${remainingPercent}% remaining)`
      : `上下文已使用 ${percent}%（剩余 ${remainingPercent}%）`;
    button.setAttribute("aria-label", button.title);
    summary.textContent = usage.limit !== null
      ? `${usage.used ?? 0} / ${usage.limit} tokens (${percent}%)`
      : uiLocale() === "en-US" ? `${percent}% used` : `${percent}% 已使用`;
    bar.style.width = `${percent}%`;
    details.textContent = [
      usage.remaining !== null ? (uiLocale() === "en-US" ? `${usage.remaining} tokens remaining` : `剩余 ${usage.remaining} tokens`) : "",
      usage.used !== null ? (uiLocale() === "en-US" ? `Current context: ${usage.used} tokens` : `当前上下文 ${usage.used} tokens`) : "",
      usage.lastTokens !== null && usage.lastTokens !== usage.used ? (uiLocale() === "en-US" ? `Latest request: ${usage.lastTokens} tokens` : `最近请求 ${usage.lastTokens} tokens`) : "",
      usage.totalTokens !== null && usage.totalTokens !== usage.used ? (uiLocale() === "en-US" ? `Total: ${usage.totalTokens} tokens` : `累计 ${usage.totalTokens} tokens`) : "",
    ].filter(Boolean).join("\n");
  }

  function setUsageMenu(open) {
    const button = $("usageButton");
    const menu = $("usageMenu");
    if (!button || !menu) return;
    const next = Boolean(open);
    menu.hidden = !next;
    button.setAttribute("aria-expanded", String(next));
  }

  function updateThreadSettings(model, effort, extra = {}) {
    if (!threadSettingsAllowed() || !state.threadId || state.role !== "operator") return;
    const threadSettings = {};
    if (model) threadSettings.model = model;
    if (effort !== undefined) threadSettings.effort = effort;
    if (isRecord(extra)) {
      for (const [key, value] of Object.entries(extra)) {
        if (value !== undefined) threadSettings[key] = value;
      }
    }
    if (!Object.keys(threadSettings).length) return;
    state.modelUpdatePending = true;
    if (model) state.currentModel = model;
    if (effort !== undefined) state.currentEffort = effort === null ? null : effort || "";
    renderModelPicker();
    try {
      command("thread/settings/update", { threadId: state.threadId, threadSettings });
    } catch (error) {
      state.modelUpdatePending = false;
      appendOutput(error.message || "无法更新模型设置", "error");
      renderModelPicker();
    }
  }

  function currentEffortParams() {
    if (state.currentEffort === null) return { effort: null };
    return state.currentEffort ? { effort: state.currentEffort } : {};
  }

  function selectModel(model) {
    const option = normalizedModelOptions().find((entry) => entry.model === model);
    const effort = option?.efforts?.length
      ? option.efforts.includes(state.currentEffort)
        ? state.currentEffort
        : option.defaultReasoningEffort || option.efforts[1] || option.efforts[0]
      : null;
    updateThreadSettings(model, effort);
    setModelMenu(false);
  }

  function selectEffort(effort) {
    updateThreadSettings(undefined, effort);
    setModelMenu(false);
  }

  function selectPowerIndex(value) {
    const current = currentModelOption();
    const options = modelPowerOptions(current);
    if (!options.length) return;
    const index = Math.max(0, Math.min(options.length - 1, Number(value) || 0));
    const option = options[index];
    if (!option) return;
    updateThreadSettings(undefined, option.effort);
    // The official power control remains open while keyboard arrows adjust it.
    state.modelAdvancedOpen = false;
    renderModelPicker();
  }

  function normalizeSubagentStatus(value) {
    const raw = isRecord(value) ? firstString(value.status, value.type, value.state) : value;
    const normalized = String(raw || "").replace(/[\s_-]+/g, "").toLowerCase();
    if (["running", "working", "interacted", "updated", "inprogress", "active", "started"].includes(normalized)) return "working";
    if (["pending", "pendinginit", "waiting", "queued", "waitingforinput", "awaitinginstruction", "waitingforinstruction", "needsinput"].includes(normalized)) return "waiting";
    if (["failed", "errored", "error", "notfound"].includes(normalized)) return "failed";
    if (["completed", "complete", "done", "interrupted", "shutdown", "cancelled", "canceled"].includes(normalized)) return "done";
    return normalized || "waiting";
  }

  function subagentStatusLabel(status) {
    // These are the localized equivalents of the official background-agent
    // rows ("is working", "is awaiting instruction", "is done").
    return t({ waiting: "正在等待指示", working: "正在工作", done: "已完成", failed: "失败" }[status] || status);
  }

  function subagentThreadId(entry) {
    return firstString(entry.threadId, entry.agentThreadId, entry.childThreadId);
  }

  function subagentElapsed(entry) {
    const startedAt = timestampMs(entry.startedAtMs, entry.startedAt, entry.createdAtMs, entry.createdAt);
    if (startedAt === null) return "";
    const status = normalizeSubagentStatus(entry.status);
    const completedAt = timestampMs(entry.completedAtMs, entry.completedAt, entry.finishedAtMs, entry.finishedAt, entry.lastAssistantMessageAtMs);
    const end = status === "working" || status === "waiting" ? Date.now() : completedAt;
    if (end === null || end === undefined) return "";
    return elapsedDuration(Math.max(0, end - startedAt));
  }

  function focusSubagentActivity(threadId) {
    if (!threadId) return;
    const output = $("output");
    const target = output
      ? [...output.querySelectorAll(".message[data-agent-thread-id]")]
        .reverse()
        .find((article) => article.dataset.agentThreadId === threadId)
      : null;
    if (!target) return;
    const turnId = target.dataset.turnId;
    const turnActivity = turnId ? state.turnDividers.get(turnId) : null;
    const turnToggle = turnActivity?.querySelector(".turn-divider-toggle");
    if (turnToggle?.getAttribute("aria-expanded") === "false") {
      turnToggle.setAttribute("aria-expanded", "true");
      setTurnActivityVisibility(turnId, true);
    }
    target.scrollIntoView({ block: "center", behavior: "smooth" });
    target.classList.remove("subagent-focus");
    void target.offsetWidth;
    target.classList.add("subagent-focus");
    setTimeout(() => target.classList.remove("subagent-focus"), 1_200);
  }

  function createSubagentPanelRow(entry) {
    const status = normalizeSubagentStatus(entry.status);
    const threadId = subagentThreadId(entry);
    const row = document.createElement(threadId ? "button" : "div");
    if (threadId) row.type = "button";
    row.className = "subagent-row";
    row.dataset.status = status;
    if (threadId) row.dataset.agentThreadId = threadId;
    const startedAt = timestampMs(entry.startedAtMs, entry.startedAt, entry.createdAtMs, entry.createdAt);
    const completedAt = timestampMs(
      entry.completedAtMs,
      entry.completedAt,
      entry.finishedAtMs,
      entry.finishedAt,
      entry.lastAssistantMessageAtMs,
      entry.recencyAtMs,
      entry.recencyAt,
    );
    if (startedAt !== null) row.dataset.startedAt = String(startedAt);
    if (completedAt !== null) row.dataset.completedAt = String(completedAt);

    const icon = document.createElement("span");
    icon.className = "subagent-icon";
    icon.setAttribute("aria-hidden", "true");
    icon.append(createSubagentIcon());
    const copy = document.createElement("span");
    copy.className = "subagent-copy";
    const name = document.createElement("span");
    name.className = "subagent-name";
    name.textContent = firstString(entry.displayName, entry.agentNickname, entry.name, entry.agentPath) || t("子代理");
    copy.append(name);
    const stateLabel = document.createElement("span");
    stateLabel.className = "subagent-status";
    const statusText = document.createElement("span");
    statusText.className = "subagent-status-text";
    statusText.textContent = subagentStatusLabel(status);
    stateLabel.append(statusText);
    const diff = isRecord(entry.diffStats) ? entry.diffStats : isRecord(entry.diff_stats) ? entry.diff_stats : null;
    const added = finiteNumber(diff?.linesAdded, diff?.added);
    const removed = finiteNumber(diff?.linesRemoved, diff?.removed);
    if (added !== null || removed !== null) {
      const diffLabel = document.createElement("span");
      diffLabel.className = "subagent-diff-stats";
      diffLabel.textContent = `+${Math.max(0, added || 0)} -${Math.max(0, removed || 0)}`;
      stateLabel.append(diffLabel);
    }
    const elapsed = document.createElement("span");
    elapsed.className = "subagent-elapsed";
    elapsed.textContent = subagentElapsed(entry);
    // The composer keeps the row compact, but exposes timing in the tooltip
    // and makes the elapsed node available for live updates.
    if (elapsed.textContent) stateLabel.append(elapsed);
    row.append(icon, copy, stateLabel);
    const objective = firstString(entry.statusMessage, entry.objective, entry.prompt, entry.role, entry.agentRole);
    const model = firstString(entry.spawnModel, entry.model, entry.modelId);
    const tooltipParts = [objective, model ? (uiLocale() === "en-US" ? `Using ${model}` : `使用 ${model}`) : "", elapsed.textContent ? (uiLocale() === "en-US" ? `Elapsed: ${elapsed.textContent}` : `已用时 ${elapsed.textContent}`) : ""].filter(Boolean);
    if (tooltipParts.length) row.title = `${name.textContent}: ${tooltipParts.join(" · ")}`;
    if (threadId) {
      row.title = tooltipParts.length
        ? `${name.textContent}: ${tooltipParts.join(" · ")}`
        : uiLocale() === "en-US" ? `View ${name.textContent}'s activity` : `查看 ${name.textContent} 的活动`;
      row.setAttribute("aria-label", `${name.textContent}, ${subagentStatusLabel(status)}`);
      row.addEventListener("click", () => focusSubagentActivity(threadId));
    }
    return row;
  }

  function deriveSubagentsFromMessages(messages) {
    if (!Array.isArray(messages)) return [];
    deriveStructuredTurnKeys(messages);
    const entries = new Map();
    let anonymousIndex = 0;
    const upsert = (item, threadId = "", itemIndex = -1) => {
      const agent = isRecord(item.agent) ? item.agent : {};
      const displayName = firstString(item.displayName, item.agentNickname, item.agentName, item.agentPath, agent.displayName, agent.name);
      const key = threadId || displayName || firstString(item.agentPath, item.action, item.activityKind) || `anonymous-${anonymousIndex++}`;
      const existing = entries.get(key) || { threadId: threadId || "", displayName: null, prompt: null, objective: null, status: "working", statusMessage: null, canInteract: false };
      const activityKind = firstString(item.activityKind, item.kind, agent.activityKind);
      const rawStatus = firstString(item.displayStatus, item.status, item.state, agent.status)
        || (activityKind === "completed" ? "completed" : activityKind === "interrupted" ? "interrupted" : "working");
      const normalizedStatus = normalizeSubagentStatus(rawStatus);
      existing.threadId = existing.threadId || threadId;
      existing.displayName = displayName || existing.displayName;
      existing.agentPath = firstString(item.agentPath, agent.agentPath, existing.agentPath);
      existing.prompt = firstString(item.prompt, agent.prompt, existing.prompt) || null;
      existing.objective = firstString(item.objective, item.statusMessage, item.prompt, agent.objective, existing.objective) || null;
      existing.statusMessage = firstString(item.statusMessage, agent.statusMessage, existing.statusMessage) || null;
      existing.status = normalizedStatus;
      existing.canInteract = item.canInteract !== undefined ? item.canInteract !== false : existing.canInteract;
      const startedAt = timestampMs(item.startedAtMs, item.startedAt, item.createdAtMs, agent.startedAtMs);
      const completedAt = timestampMs(
        item.completedAtMs,
        item.completedAt,
        item.finishedAtMs,
        item.finishedAt,
        item.lastAssistantMessageAtMs,
        item.recencyAtMs,
        item.recencyAt,
        agent.completedAtMs,
        agent.lastAssistantMessageAtMs,
        agent.recencyAtMs,
      );
      if (startedAt !== null) existing.startedAtMs = existing.startedAtMs ?? startedAt;
      if (completedAt !== null) existing.completedAtMs = completedAt;
      if (existing.completedAtMs === undefined && normalizedStatus === "done" && itemIndex >= 0) {
        const turnKey = structuredMessageTurn(item, itemIndex);
        for (let cursor = itemIndex + 1; cursor < messages.length; cursor += 1) {
          const next = messages[cursor];
          if (!isRecord(next)) continue;
          if (structuredMessageTurn(next, cursor) !== turnKey) break;
          const nextStartedAt = timestampMs(next.startedAtMs, next.startedAt, next.createdAtMs);
          if (nextStartedAt !== null) {
            existing.completedAtMs = nextStartedAt;
            break;
          }
        }
      }
      if (item.model !== undefined || agent.model !== undefined) existing.model = firstString(item.model, agent.model, existing.model) || null;
      entries.set(key, existing);
    };
    messages.forEach((item, index) => {
      if (!isRecord(item) || historyKind(item) !== "subagent") return;
      const receivers = Array.isArray(item.receiverThreadIds)
        ? item.receiverThreadIds.map(String)
        : Array.isArray(item.receiverThreads) ? item.receiverThreads.map(String) : [];
      const states = isRecord(item.agentsStates) ? Object.keys(item.agentsStates) : [];
      const ids = [...new Set([
        firstString(item.agentThreadId, item.childThreadId),
        ...receivers,
        ...states,
      ].filter(Boolean))];
      if (!ids.length) upsert(item, "", index);
      else ids.forEach((id) => upsert(item, id, index));
    });
    return [...entries.values()];
  }

  function subagentIdentity(entry) {
    if (!isRecord(entry)) return "";
    return firstString(
      entry.threadId,
      entry.agentThreadId,
      entry.childThreadId,
      entry.displayName,
      entry.agentNickname,
      entry.name,
      entry.agentPath,
    );
  }

  // A snapshot can carry a rich top-level subagent projection while the
  // message list only contains the compact activity item. Merge matching
  // identities so timestamps, model names, and interaction flags survive the
  // transcript re-render without carrying stale agents across threads.
  function mergeSubagentProjections(derived, supplied) {
    const rich = Array.isArray(supplied) ? supplied.filter(isRecord) : [];
    if (!rich.length) return derived;
    const byIdentity = new Map(rich
      .map((entry) => [subagentIdentity(entry), entry])
      .filter(([identity]) => Boolean(identity)));
    const matched = new Set();
    const merged = derived.map((entry) => {
      const identity = subagentIdentity(entry);
      const source = identity ? byIdentity.get(identity) : undefined;
      if (!source) return entry;
      matched.add(source);
      return { ...entry, ...source };
    });
    // Keep a rich top-level entry when the transcript projection is absent
    // (for example while a newly spawned agent has not emitted its first
    // activity item yet), but discard unrelated entries from an older thread.
    if (!merged.length) return rich;
    for (const source of rich) {
      if (matched.has(source)) continue;
      const identity = subagentIdentity(source);
      if (identity && !merged.some((entry) => subagentIdentity(entry) === identity)
        && normalizeSubagentStatus(source.status) !== "done") merged.push(source);
    }
    return merged;
  }

  function refreshSubagentElapsed() {
    for (const row of document.querySelectorAll(".subagent-row[data-started-at]")) {
      const startedAt = timestampMs(row.dataset.startedAt);
      if (startedAt === null) continue;
      const completedAt = timestampMs(row.dataset.completedAt);
      const status = normalizeSubagentStatus(row.dataset.status);
      const end = status === "working" || status === "waiting" ? Date.now() : completedAt;
      const label = row.querySelector(".subagent-elapsed");
      if (label) label.textContent = end === null || end === undefined ? "" : elapsedDuration(Math.max(0, end - startedAt));
    }
  }

  function renderSubagents() {
    const panel = $("subagentsPanel");
    const list = $("subagentsList");
    const count = $("subagentsCount");
    if (!panel || !list || !count) return;
    const entries = Array.isArray(state.subagents) ? state.subagents.filter(isRecord) : [];
    const visibleEntries = entries.filter((entry) => firstString(
      entry.displayName,
      entry.agentNickname,
      entry.name,
      entry.agentPath,
      entry.threadId,
      entry.agentThreadId,
      entry.childThreadId,
      entry.objective,
    ));
    panel.hidden = visibleEntries.length === 0;
    if (!visibleEntries.length) {
      count.textContent = "";
      if (isRecord(state.subagentsExpanded)) {
        state.subagentsExpanded.active = false;
        state.subagentsExpanded.done = false;
      }
      list.replaceChildren();
      return;
    }
    const normalizedEntries = visibleEntries.map((entry) => ({ entry, status: normalizeSubagentStatus(entry.status) }));
    // The official composer uses one compact disclosure row. It does not
    // split the list into active/done sections or show per-agent wall-clock
    // durations; status is rendered inline beside each display name.
    const title = $("subagentsToggle")?.querySelector(".subagents-title");
    if (title) title.textContent = uiLocale() === "en-US"
      ? `${normalizedEntries.length} background agents${state.subagentsCollapsed ? "" : " · @ to mention agents"}`
      : `${normalizedEntries.length} 个后台代理${state.subagentsCollapsed ? "" : " · @ 可标记代理"}`;
    count.textContent = "";
    panel.dataset.collapsed = String(state.subagentsCollapsed);
    const toggle = $("subagentsToggle");
    if (toggle) toggle.setAttribute("aria-expanded", String(!state.subagentsCollapsed));
    list.setAttribute("aria-hidden", String(state.subagentsCollapsed));
    list.inert = state.subagentsCollapsed;
    list.replaceChildren();
    const rows = document.createElement("div");
    rows.className = "subagent-section-rows";
    for (const { entry } of normalizedEntries) rows.append(createSubagentPanelRow(entry));
    list.append(rows);
  }

  function normalizeSandboxName(value) {
    const normalized = String(value || "")
      .replace(/^:+/, "")
      .replace(/([a-z0-9])([A-Z])/g, "$1-$2")
      .replace(/[\s_]+/g, "-")
      .toLowerCase();
    if (normalized === "dangerfullaccess" || normalized === "full-access" || normalized === "fullaccess") return "danger-full-access";
    if (normalized === "workspacewrite") return "workspace-write";
    if (normalized === "workspace" || normalized === "write") return "workspace-write";
    if (normalized === "readonly" || normalized === "read") return "read-only";
    return normalized;
  }

  function sandboxFromPermissions(value) {
    if (typeof value === "string") {
      const normalized = normalizeSandboxName(value);
      if (["read-only", "workspace-write", "danger-full-access"].includes(normalized)) return normalized;
      return "";
    }
    if (!isRecord(value)) return "";
    const explicit = firstString(value.sandboxPolicy, value.sandbox, value.mode, value.profile, value.type);
    if (explicit) {
      const normalized = normalizeSandboxName(explicit);
      if (["read-only", "workspace-write", "danger-full-access"].includes(normalized)) return normalized;
    }
    if (value.dangerFullAccess === true || value.fullAccess === true || value.full_access === true) return "danger-full-access";
    const fileSystem = isRecord(value.fileSystem) ? value.fileSystem : isRecord(value.file_system) ? value.file_system : value;
    if (fileSystem.write === true || fileSystem.workspaceWrite === true || fileSystem.workspace_write === true) return "workspace-write";
    if (fileSystem.readOnly === true || fileSystem.read_only === true || fileSystem.read === true) return "read-only";
    return "";
  }

  function normalizeApprovalPolicy(value) {
    const candidate = isRecord(value)
      ? firstString(value.policy, value.mode, value.type, value.approvalPolicy, value.approval_policy)
      : firstString(value);
    const normalized = String(candidate || "").replace(/[\s_]+/g, "-").toLowerCase();
    return ["on-request", "never", "untrusted"].includes(normalized) ? normalized : "";
  }

  // Model metadata belongs to the attached thread. Clear it before applying
  // a snapshot for a different thread so an older catalog cannot leak into a
  // newly selected conversation that does not expose model data.
  function resetSessionModelMetadata() {
    state.currentModel = "";
    state.currentEffort = "";
    state.availableModels = [];
    state.modelUpdatePending = false;
    state.tokenUsage = null;
    state.workedDurationMs = null;
    state.lastWorkedDurationMs = null;
    state.turnWorkStartedAt = null;
    state.finalAssistantStartedAt = null;
    state.sandboxPolicy = "workspace-write";
    state.approvalPolicy = "on-request";
    const modelInput = $("modelInput");
    if (modelInput) modelInput.value = "";
    const label = $("modelLabel");
    if (label) {
      label.textContent = "";
      label.hidden = true;
    }
    setModelMenu(false);
    renderModelPicker();
    renderPermissionMenu();
    renderUsage();
  }

  function prepareForSessionSnapshot(threadId) {
    const incoming = typeof threadId === "string" ? threadId : "";
    const previous = state.syncedThreadId !== null ? state.syncedThreadId : state.threadId || "";
    if (previous !== incoming && (previous || incoming)) {
      resetSessionModelMetadata();
      state.turnExpansion.clear();
    }
    if (incoming) syncSessionActive(incoming);
    return previous !== incoming;
  }

  function snapshotHistoryComplete(...sources) {
    const seen = new Set();
    const visit = (value, depth = 0) => {
      if (!isRecord(value) || seen.has(value) || depth > 3) return undefined;
      seen.add(value);
      if (typeof value.historyComplete === "boolean") return value.historyComplete;
      for (const key of ["metadata", "sessionMetadata", "state", "snapshot"]) {
        const nested = visit(value[key], depth + 1);
        if (typeof nested === "boolean") return nested;
      }
      return undefined;
    };
    for (const source of sources) {
      const result = visit(source);
      if (typeof result === "boolean") return result;
    }
    return undefined;
  }

  function projectionTargetThreadId() {
    const switchingTarget = firstString(state.sessionSwitchContext?.targetThreadId);
    if (switchingTarget) return switchingTarget;
    if (state.sessionSelectedThreadId && state.sessionSelectedThreadId !== state.syncedThreadId) {
      return state.sessionSelectedThreadId;
    }
    return firstString(state.threadId, state.syncedThreadId);
  }

  function outputProjectionAllowed(threadId) {
    const incoming = String(threadId || "");
    const expected = projectionTargetThreadId();
    return !expected || !incoming || incoming === expected;
  }

  function hasVisibleOutputProjection() {
    const output = $("output");
    return Boolean(
      state.structuredMessages.length
      || output?.dataset.outputTail
      || output?.querySelector(".message")
    );
  }

  function finishSessionSnapshotCommit(threadId, authoritativeSnapshot = false) {
    const incoming = String(threadId || "");
    if (!incoming) return;
    const context = state.sessionSwitchContext;
    if (authoritativeSnapshot && context?.targetThreadId === incoming) {
      context.targetSnapshotReady = true;
      finishSessionSwitchIfReady();
    }
    syncSessionActive(incoming);
  }

  // Transcript, active-thread identity, and switch completion are one commit.
  // This prevents a metadata-only/placeholder reconnect snapshot from clearing
  // a fully rendered thread or completing a switch before its history arrives.
  function commitOutputProjection(threadId, text, structuredMessages, options = {}) {
    const incoming = String(threadId || "");
    if (!outputProjectionAllowed(incoming)) return false;
    const hasContent = Boolean(
      (typeof text === "string" && text.length)
      || (Array.isArray(structuredMessages) && structuredMessages.length)
    );
    const historyComplete = options.historyComplete;
    if (!hasContent && historyComplete !== true) {
      // All relay control snapshots have projection-shaped placeholder fields.
      // Until the host explicitly says history loading is complete, an empty
      // projection is not authoritative—even when the current DOM is empty.
      return false;
    }
    const changedThread = state.syncedThreadId !== null && state.syncedThreadId !== incoming;
    prepareForSessionSnapshot(incoming);
    if (changedThread) {
      state.outputSynced = false;
      state.snapshotNoticeShown = false;
    }
    replaceOutput(typeof text === "string" ? text : "", structuredMessages);
    state.syncedThreadId = incoming;
    if (incoming) state.threadId = incoming;
    finishSessionSnapshotCommit(incoming, options.authoritativeSnapshot === true);
    return true;
  }

  function applySessionMetadata(metadata, snapshotState = {}) {
    const meta = isRecord(metadata) ? metadata : {};
    const stateSnapshot = isRecord(snapshotState) ? snapshotState : {};
    const thread = isRecord(meta.thread) ? meta.thread : isRecord(stateSnapshot.thread) ? stateSnapshot.thread : {};
    const settings = isRecord(meta.threadSettings)
      ? meta.threadSettings
      : isRecord(meta.latestThreadSettings)
        ? meta.latestThreadSettings
      : isRecord(meta.settings)
        ? meta.settings
        : isRecord(stateSnapshot.threadSettings) ? stateSnapshot.threadSettings : {};
    const title = firstString(meta.title, meta.threadTitle, meta.name, thread.title, thread.name, thread.preview, stateSnapshot.title, stateSnapshot.threadTitle);
    if (title) $("threadTitle").textContent = title;

    const cwd = firstString(meta.cwd, settings.cwd, stateSnapshot.cwd);
    if (cwd) $("cwdInput").value = cwd;
    const modelValue = meta.latestModel ?? meta.model ?? meta.modelName ?? meta.modelId
      ?? settings.model ?? settings.modelName ?? stateSnapshot.latestModel ?? stateSnapshot.model ?? stateSnapshot.modelName;
    const model = typeof modelValue === "object" && modelValue !== null
      ? firstString(modelValue.name, modelValue.id, modelValue.slug)
      : firstString(modelValue);
    if (model) {
      $("modelInput").value = model;
      const modelLabel = $("modelLabel");
      const effortValue = firstDefined(
        meta.latestReasoningEffort,
        meta.effort,
        settings.effort,
        stateSnapshot.latestReasoningEffort,
        stateSnapshot.effort,
      );
      state.currentModel = model;
      if (effortValue !== undefined) {
        state.currentEffort = effortValue === null ? null : firstString(effortValue);
      }
      if (modelLabel) { modelLabel.textContent = modelDisplayName(model); modelLabel.hidden = false; }
    } else if (modelValue !== undefined) {
      const modelLabel = $("modelLabel");
      if (modelLabel) modelLabel.hidden = true;
    }
    const modelsValue = meta.availableModels ?? meta.models ?? stateSnapshot.availableModels ?? stateSnapshot.models;
    if (Array.isArray(modelsValue)) state.availableModels = modelsValue.filter((entry) => typeof entry === "string" || isRecord(entry));
    const subagentsValue = meta.subagents ?? stateSnapshot.subagents;
    if (Array.isArray(subagentsValue)) state.subagents = subagentsValue;
    const usageValue = meta.tokenUsage ?? meta.latestTokenUsageInfo ?? meta.contextUsage ?? meta.usage
      ?? stateSnapshot.tokenUsage ?? stateSnapshot.latestTokenUsageInfo ?? stateSnapshot.contextUsage ?? stateSnapshot.usage;
    if (usageValue !== undefined) state.tokenUsage = usageValue;
    const metadataWorkedDuration = finiteNumber(
      meta.workedDurationMs,
      meta.workDurationMs,
      meta.workedForMs,
      meta.workedFor?.durationMs,
      settings.workedDurationMs,
      stateSnapshot.workedDurationMs,
      stateSnapshot.workDurationMs,
    );
    if (metadataWorkedDuration !== null) {
      state.workedDurationMs = Math.max(0, metadataWorkedDuration);
      state.lastWorkedDurationMs = Math.max(0, metadataWorkedDuration);
    }
    const metadataWorkStart = timestampMs(
      meta.firstTurnWorkItemStartedAtMs,
      meta.firstWorkItemStartedAtMs,
      meta.workStartedAtMs,
      meta.workedFor?.startedAtMs,
      settings.firstTurnWorkItemStartedAtMs,
      stateSnapshot.firstTurnWorkItemStartedAtMs,
      stateSnapshot.workStartedAtMs,
    );
    if (metadataWorkStart !== null) state.turnWorkStartedAt = metadataWorkStart;
    const metadataFinalStart = timestampMs(
      meta.finalAssistantStartedAtMs,
      meta.workedFor?.completedAtMs,
      settings.finalAssistantStartedAtMs,
      stateSnapshot.finalAssistantStartedAtMs,
    );
    if (metadataFinalStart !== null) state.finalAssistantStartedAt = metadataFinalStart;
    renderModelPicker();
    renderSubagents();
    renderUsage();

    const permissionsValue = meta.permissions ?? meta.currentPermissions ?? settings.permissions
      ?? stateSnapshot.permissions ?? stateSnapshot.currentPermissions;
    const sandboxValue = meta.sandboxPolicy ?? meta.sandbox ?? settings.sandboxPolicy ?? settings.sandbox
      ?? stateSnapshot.sandboxPolicy ?? stateSnapshot.sandbox ?? sandboxFromPermissions(permissionsValue);
    const sandbox = typeof sandboxValue === "object" && sandboxValue !== null
      ? firstString(sandboxValue.type, sandboxValue.mode, sandboxValue.policy)
      : firstString(sandboxValue);
    if (sandbox) {
      const normalizedSandbox = normalizeSandboxName(sandbox);
      state.sandboxPolicy = normalizedSandbox;
      const sandboxInput = $("sandboxInput");
      if (sandboxInput && [...sandboxInput.options].some((option) => option.value === normalizedSandbox)) sandboxInput.value = normalizedSandbox;
      const permission = $("permissionChip");
      const permissionLabel = $("permissionLabel");
      if (permission && permissionLabel) {
        permissionLabel.textContent = permissionSandboxLabel(normalizedSandbox);
        permission.hidden = false;
      }
    } else {
      const permission = $("permissionChip");
      if (permission) permission.hidden = false;
    }
    const approval = normalizeApprovalPolicy(
      meta.approvalPolicy !== undefined ? meta.approvalPolicy
        : settings.approvalPolicy !== undefined ? settings.approvalPolicy
          : stateSnapshot.approvalPolicy,
    );
    if (approval) {
      state.approvalPolicy = approval;
      const approvalInput = $("approvalInput");
      if (approvalInput && [...approvalInput.options].some((option) => option.value === approval)) approvalInput.value = approval;
    }
    renderPermissionMenu();
    const mode = firstString(meta.mode, stateSnapshot.mode);
    const modeLabel = $("modeLabel");
    if (modeLabel && mode) modeLabel.textContent = t(/cloud|remote/i.test(mode) ? "云端模式" : "本地模式");
    const popoverCwd = $("popoverCwd");
    const popoverMode = $("popoverMode");
    if (popoverCwd && cwd) popoverCwd.textContent = cwd;
    if (popoverMode && mode) popoverMode.textContent = t(/cloud|remote/i.test(mode) ? "云端模式" : "本地模式");
  }

  const setConnection = (kind, text) => {
    $("connectionDot").className = `dot ${kind}`;
    $("connectionText").textContent = t(text);
    if (embeddedInAether) embedBridge.reportState(kind, { message: String(text || "") });
  };

  function setAuthRequired(value) {
    if (typeof value !== "boolean") return;
    state.authRequired = value;
    document.body.classList.toggle("local-no-auth", !value);
    const label = $("tokenLabel");
    const input = $("tokenInput");
    if (!label || !input) return;
    label.textContent = t(value ? "访问 token（认证模式）" : "本机连接（无需 token）");
    input.placeholder = t(value ? "粘贴 relay 启动时打印的 token" : "本机模式无需填写；认证模式再填写");
    input.setAttribute("aria-label", t(value ? "relay access token" : "本地连接无需 token"));
  }

  const authHeaders = () => ({ Authorization: `Bearer ${state.token}`, "Content-Type": "application/json" });

  function attachPendingUserToTurn(turnId) {
    const key = String(turnId || "");
    if (!key) return;
    const article = state.pendingUserArticle;
    if (article && article.isConnected) article.dataset.turnId = key;
    state.pendingUserArticle = null;
  }

  function sendFrame(frame) {
    if (!state.ws || state.ws.readyState !== WebSocket.OPEN) throw new Error(t("WebSocket 未连接"));
    state.ws.send(JSON.stringify(frame));
  }

  function command(method, params) {
    const commandId = `web-${crypto.randomUUID()}`;
    sendFrame({ type: "command", commandId, method, params });
    if (method === "turn/start" || method === "turn/steer") {
      const text = (params?.input || [])
        .map((item) => typeof item === "string" ? item : item?.text)
        .filter(Boolean)
        .join("\n");
      finishAssistantStream();
      state.pendingUserText = text;
      appendDateSeparator(Date.now(), { role: "user", turnId: state.turnId || params?.expectedTurnId || "", turnStart: true });
      const userMessage = appendMessage(text || "(空消息)", "user", "text", "", {
        kind: "user",
        turnId: state.turnId || params?.expectedTurnId || "",
        timestamp: Date.now(),
        showTimestamp: true,
      });
      if (method === "turn/start") state.pendingUserArticle = userMessage?.article || null;
    } else if (method === "thread/settings/update") {
      // Keep settings changes quiet in the transcript. The model picker has
      // its own pending state and the authoritative snapshot will update the
      // label once the official follower accepts the request.
      setConversationStatus("正在更新模型设置", "active");
    } else if (["session/list", "session/select", "control/mode/set"].includes(sessionCommandMethod(method))) {
      // Session navigation is shell UI state. It must not appear as a Codex
      // message or alter the current turn's activity timeline.
    } else {
      appendOutput(`${method}  (${commandId})`, "meta");
    }
    return commandId;
  }

  function composerText() {
    const editor = $("messageInput");
    if (!editor) return "";
    return String(editor.innerText || editor.textContent || "")
      .replace(/\u00a0/g, " ")
      .replace(/\n{3,}/g, "\n\n")
      .trim();
  }

  function clearComposer() {
    const editor = $("messageInput");
    if (!editor) return;
    editor.replaceChildren();
    editor.style.height = "";
    resizeComposer();
    updateIds();
  }

  function resizeComposer() {
    const editor = $("messageInput");
    if (!editor) return;
    editor.style.height = "auto";
    const maxHeight = Math.max(40, Math.round(window.innerHeight * 0.25));
    const nextHeight = Math.min(Math.max(editor.scrollHeight, 40), maxHeight);
    editor.style.height = `${nextHeight}px`;
    updateScrollPadding();
  }

  function defaultResponse(request) {
    const method = request.method;
    // Rendering a request must never imply consent. The operator still has
    // to press an explicit action button.
    if (method === "item/commandExecution/requestApproval") return { decision: "decline" };
    if (method === "item/fileChange/requestApproval") return { decision: "decline" };
    if (method === "item/permissions/requestApproval") {
      return normalizePermissionResponse({});
    }
    if (method === "applyPatchApproval" || method === "execCommandApproval") return { decision: { denied: { rejection: "默认拒绝，请明确允许" } } };
    if (method === "item/tool/requestUserInput") {
      const answers = {};
      for (const question of request.params?.questions || []) answers[question.id] = { answers: [""] };
      return { answers };
    }
    if (method === "mcpServer/elicitation/request") return { action: "decline", content: null, _meta: null };
    return {};
  }

  function normalizePermissionResponse(rawPermissions, scope, strictAutoReview) {
    const permissions = {};
    if (rawPermissions && typeof rawPermissions === "object" && !Array.isArray(rawPermissions)) {
      for (const [key, value] of Object.entries(rawPermissions)) {
        if (value && typeof value === "object" && !Array.isArray(value)) permissions[key] = value;
      }
    }
    const response = { permissions, scope: scope === "session" ? "session" : "turn" };
    if (typeof strictAutoReview === "boolean") response.strictAutoReview = strictAutoReview;
    return response;
  }

  function requestSummary(request) {
    const params = request.params || {};
    if (Array.isArray(params.commandActions)) {
      const commands = params.commandActions
        .map((action) => action && typeof action === "object" ? action.command || action.description : "")
        .filter(Boolean);
      if (commands.length) return commands.join("\n");
    }
    if (request.summary) return request.summary;
    if (Array.isArray(params.command)) return params.command.join(" ");
    if (params.command) return params.command;
    if (params.reason) return params.reason;
    if (Array.isArray(params.questions)) return params.questions.map((q) => q.question).join(" / ");
    if (params.message) return params.message;
    return t("需要远程确认或输入");
  }

  function requestTitle(request) {
    const method = String(request.method || "");
    if (method === "item/commandExecution/requestApproval" || method === "execCommandApproval") return t("允许运行命令？");
    if (method === "item/fileChange/requestApproval" || method === "applyPatchApproval") return t("允许修改文件？");
    if (method === "item/permissions/requestApproval") return t("需要扩大权限");
    if (method === "item/tool/requestUserInput") return t("Codex 需要你的回答");
    if (method === "mcpServer/elicitation/request") return t("需要外部服务确认");
    return t("Codex 请求确认");
  }

  function requestRisk(request) {
    const risk = String(request.risk || "medium").toLowerCase();
    return t(risk === "high" ? "高风险" : risk === "low" ? "低风险" : "需确认");
  }

  function requestCommand(request) {
    const params = request.params || {};
    if (Array.isArray(params.commandActions)) {
      return params.commandActions
        .map((action) => action && typeof action === "object" ? action.command || action.description : "")
        .filter(Boolean)
        .join("\n");
    }
    if (Array.isArray(params.command)) return params.command.join(" ");
    return typeof params.command === "string" ? params.command : "";
  }

  function questionOptions(question) {
    const options = question?.options || question?.choices || question?.enum;
    if (!Array.isArray(options)) return [];
    return options.map((option) => {
      if (typeof option === "string") return { label: option, value: option };
      if (option && typeof option === "object") {
        const value = option.value ?? option.id ?? option.label ?? option.name;
        const label = option.label ?? option.name ?? value;
        return { label: String(label ?? ""), value: String(value ?? "") };
      }
      return null;
    }).filter((option) => option && option.value);
  }

  function renderQuestionFields(container, request) {
    const questions = Array.isArray(request.params?.questions) ? request.params.questions : [];
    container.replaceChildren();
    if (!questions.length) {
      container.hidden = true;
      return;
    }
    container.hidden = false;
    questions.forEach((question, index) => {
      if (!question || typeof question !== "object") return;
      const field = document.createElement("label");
      field.className = "request-question";
      field.dataset.questionId = String(question.id ?? question.key ?? index);
      const prompt = document.createElement("span");
      const questionPrompt = question.question ?? question.prompt ?? question.label;
      prompt.textContent = questionPrompt === undefined || questionPrompt === null ? t("请输入") : String(questionPrompt);
      field.append(prompt);
      const options = questionOptions(question);
      let control;
      if (options.length) {
        control = document.createElement("select");
        options.forEach((option) => {
          const item = document.createElement("option");
          item.value = option.value;
          item.textContent = option.label;
          control.append(item);
        });
      } else {
        control = document.createElement("input");
        control.type = question.secret ? "password" : "text";
        control.placeholder = String(question.placeholder ?? "");
      }
      control.className = "request-answer";
      control.dataset.answerId = field.dataset.questionId;
      field.append(control);
      container.append(field);
    });
  }

  function inputResponseFromCard(article) {
    const answers = {};
    for (const field of article.querySelectorAll(".request-question")) {
      const id = field.dataset.questionId;
      const value = field.querySelector(".request-answer")?.value ?? "";
      answers[id] = { answers: [value] };
    }
    return { answers };
  }

  function requestResponseFromCard(request, article, responseBox, action) {
    if (request.method === "item/tool/requestUserInput" && action === "allow") {
      // Prefer explicit edits in the JSON editor; otherwise collect the
      // first-class answer controls rendered for each question.
      if (responseBox.value !== article.dataset.defaultResponse) {
        try {
          const parsed = JSON.parse(responseBox.value);
          if (parsed && typeof parsed === "object") return parsed;
        } catch {
          return null;
        }
      }
      return inputResponseFromCard(article);
    }
    if (action === "allow") {
      const response = allowResponse(request, responseBox.value);
      const scope = article.querySelector(".request-scope")?.value;
      if (request.method === "item/permissions/requestApproval" && response && typeof response === "object" && scope) response.scope = scope;
      return response;
    }
    if (action === "deny") return denyResponse(request);
    try {
      const parsed = JSON.parse(responseBox.value);
      return parsed && typeof parsed === "object" ? parsed : {};
    } catch {
      return null;
    }
  }

  function buildRequestCard(request) {
    const fragment = $("requestTemplate").content.cloneNode(true);
    const article = fragment.querySelector(".request");
    const method = String(request.method || "unknown");
    const risk = requestRisk(request);
    article.dataset.requestKey = requestKey(request.requestId);
    article.dataset.risk = String(request.risk || "medium").toLowerCase();
    fragment.querySelector(".request-method").textContent = requestTitle(request);
    fragment.querySelector(".request-id").textContent = `#${request.requestId}`;
    const riskNode = fragment.querySelector(".request-risk");
    riskNode.textContent = risk;
    riskNode.dataset.risk = article.dataset.risk;
    fragment.querySelector(".request-summary").textContent = requestSummary(request);
    const commandNode = fragment.querySelector(".request-command");
    const commandText = requestCommand(request);
    commandNode.textContent = commandText;
    commandNode.hidden = !commandText;
    fragment.querySelector(".request-json").textContent = JSON.stringify(request.params || {}, null, 2);
    const responseBox = fragment.querySelector(".request-response");
    responseBox.value = JSON.stringify(defaultResponse(request), null, 2);
    article.dataset.defaultResponse = responseBox.value;
    const scopeWrap = fragment.querySelector(".request-scope-wrap");
    if (request.method === "item/permissions/requestApproval") scopeWrap.hidden = false;
    renderQuestionFields(fragment.querySelector(".request-questions"), request);
    const allow = fragment.querySelector(".request-allow");
    const deny = fragment.querySelector(".request-deny");
    const send = fragment.querySelector(".request-send");
    allow.textContent = t(method === "item/tool/requestUserInput" ? "提交回答" : method === "item/permissions/requestApproval" ? "允许" : "允许一次");
    send.textContent = t("发送自定义响应");
    allow.addEventListener("click", () => {
      const result = requestResponseFromCard(request, article, responseBox, "allow");
      if (result) respond(request.requestId, JSON.stringify(result));
      else appendOutput("自定义响应不是有效 JSON", "error");
    });
    deny.addEventListener("click", () => respond(request.requestId, JSON.stringify(requestResponseFromCard(request, article, responseBox, "deny"))));
    send.addEventListener("click", () => {
      const result = requestResponseFromCard(request, article, responseBox, "custom");
      if (result) respond(request.requestId, JSON.stringify(result));
      else appendOutput("响应不是有效 JSON", "error");
    });
    if (state.sessionSwitching || state.modeSwitching || state.role !== "operator" || !RESPONDABLE_METHODS.has(method) || state.responding.has(requestKey(request.requestId))) {
      for (const button of fragment.querySelectorAll("button")) button.disabled = true;
      responseBox.disabled = true;
      for (const control of fragment.querySelectorAll("input, select")) control.disabled = true;
    }
    return fragment;
  }

  function renderRequests() {
    const container = $("inlineRequests");
    const legacyContainer = $("requests");
    container.replaceChildren();
    if (legacyContainer) legacyContainer.replaceChildren();
    const requests = [...state.requests.values()];
    $("requestCount").textContent = String(requests.length);
    $("factRequests").textContent = String(requests.length);
    renderControlMode();
    const panel = $("requestsPanel");
    if (!requests.length) {
      container.className = "inline-requests empty";
      if (legacyContainer) {
        legacyContainer.className = "requests empty";
        legacyContainer.textContent = t("暂无待处理请求");
      }
      if (panel) panel.open = false;
      return;
    }
    container.className = "inline-requests";
    for (const request of requests) {
      const fragment = buildRequestCard(request);
      container.append(fragment);
    }
    if (panel) panel.open = false;
  }

  function denyResponse(request) {
    if (request.method === "item/commandExecution/requestApproval" || request.method === "item/fileChange/requestApproval") return { decision: "decline" };
    if (request.method === "item/permissions/requestApproval") return normalizePermissionResponse({});
    if (request.method === "applyPatchApproval" || request.method === "execCommandApproval") return { decision: { denied: { rejection: "远程参与者拒绝" } } };
    if (request.method === "mcpServer/elicitation/request") return { action: "decline", content: null, _meta: null };
    if (request.method === "item/tool/requestUserInput") return { answers: {} };
    return { decision: "decline" };
  }

  function allowResponse(request, raw) {
    const method = request.method;
    if (method === "item/commandExecution/requestApproval" || method === "item/fileChange/requestApproval") return { decision: "accept" };
    if (method === "item/permissions/requestApproval") {
      if (raw === undefined) return normalizePermissionResponse(request.params?.permissions);
      try {
        const parsed = JSON.parse(raw);
        return parsed && typeof parsed === "object"
          ? normalizePermissionResponse(parsed.permissions, parsed.scope, parsed.strictAutoReview)
          : normalizePermissionResponse({});
      } catch {
        return normalizePermissionResponse(request.params?.permissions);
      }
    }
    if (method === "applyPatchApproval" || method === "execCommandApproval") return { decision: "approved" };
    if (method === "mcpServer/elicitation/request") return { action: "accept", content: null, _meta: null };
    // User-input requests need the operator's edited answers. Keep the JSON
    // editor as the source of truth and fail closed if it is malformed.
    try {
      const parsed = JSON.parse(raw);
      return parsed && typeof parsed === "object" ? parsed : { answers: {} };
    } catch {
      return { answers: {} };
    }
  }

  function respond(requestId, raw) {
    if (state.sessionSwitching) return;
    const key = requestKey(requestId);
    if (state.responding.has(key)) return;
    let result;
    try { result = JSON.parse(raw); } catch { appendOutput("响应不是有效 JSON", "error"); return; }
    try { sendFrame({ type: "respond", requestId, result }); } catch (error) { appendOutput(error.message, "error"); return; }
    state.responding.add(key);
    renderRequests();
  }

  function updateIds() {
    $("threadId").textContent = state.threadId || "-";
    $("turnId").textContent = state.turnId || "-";
    const popoverThread = $("popoverThread");
    if (popoverThread) popoverThread.textContent = state.threadId || "-";
    const hasThread = Boolean(state.threadId);
    const hasTurn = Boolean(state.turnId);
    const hasComposerText = Boolean(composerText());
    const switching = Boolean(state.sessionSwitching || state.modeSwitching);
    document.body.classList.toggle("turn-active", hasTurn);
    $("startThreadButton").disabled = state.attachMode || !state.appReady || state.role !== "operator";
    const newSessionButton = $("newSessionButton");
    if (newSessionButton) {
      newSessionButton.disabled = !sessionControlAllowed("sessionCreate") || switching || !state.appReady
        || Boolean(state.newSessionCommandId)
        || !["operator", "owner", "host"].includes(String(state.role || ""));
      newSessionButton.setAttribute("aria-busy", String(Boolean(state.newSessionCommandId)));
    }
    $("startTurnButton").disabled = switching || !state.appReady || !hasThread || hasTurn || !hasComposerText || state.role !== "operator";
    $("steerButton").disabled = switching || !state.appReady || !hasTurn || !hasComposerText || state.role !== "operator";
    $("interruptButton").disabled = switching || !state.appReady || !hasTurn || state.role !== "operator";
    const messageInput = $("messageInput");
    if (messageInput) {
      messageInput.contentEditable = switching ? "false" : "true";
      messageInput.setAttribute("aria-disabled", String(switching));
    }
    for (const id of ["modelPickerButton", "permissionChip", "composerPlusButton"]) {
      const control = $(id);
      if (control) control.disabled = switching;
    }
    const send = $("startTurnButton");
    if (send) {
      send.title = t(hasTurn ? "发送 Steer" : "发送消息");
      send.setAttribute("aria-label", t(hasTurn ? "发送 Steer" : "发送消息"));
    }
    const settingsModelValue = $("settingsModelValue");
    if (settingsModelValue) settingsModelValue.textContent = state.currentModel ? modelDisplayName(state.currentModel, state.currentEffort) : t("默认");
    const settingsPermissionValue = $("settingsPermissionValue");
    if (settingsPermissionValue) settingsPermissionValue.textContent = permissionSandboxLabel(state.sandboxPolicy);
    renderControlMode();
  }

  function protocolMethodForEvent(event, payload) {
    if (typeof payload?.method === "string") return payload.method;
    if (typeof payload?.params?.method === "string") return payload.params.method;
    const type = String(event?.type || "");
    if (type === "item.started") return "item/started";
    if (type === "item.completed") return "item/completed";
    if (type === "thread.started") return "thread/started";
    if (type === "turn.started") return "turn/started";
    if (type === "turn.completed") return "turn/completed";
    if (type === "turn.plan.updated") return "turn/plan/updated";
    if (type === "turn.diff.updated") return "turn/diff/updated";
    return "";
  }

  function handleProtocolNotification(method, payload, eventType = "") {
    const params = eventParams(payload);
    if (method === "item/started" || eventType === "item.started") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      handleItemLifecycle("started", params);
      return true;
    }
    if (method === "item/completed" || eventType === "item.completed") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      handleItemLifecycle("completed", params);
      return true;
    }
    if (method === "turn/plan/updated" || method === "turn/plan/update" || eventType === "turn.plan.updated") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      handlePlanUpdate(params);
      return true;
    }
    if (method === "turn/diff/updated" || method === "turn/diff/update" || eventType === "turn.diff.updated") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      handleDiffUpdate(params);
      return true;
    }
    if (/^(?:item\/)?fileChange\/outputDelta$/.test(method)
      || method === "item/fileChange/delta") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      appendFileChangeChunk(payload, textFromValue(params.delta ?? params.text ?? params.output ?? params.chunk));
      return true;
    }
    if (/^(?:item\/)?(?:fileRead|readFile|fileReadOutput)\/(?:outputDelta|delta|textDelta)$/.test(method)
      || method === "item/fileRead/outputDelta"
      || method === "item/fileRead/delta") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      appendOutputChunk(textFromValue(params.delta ?? params.text ?? params.output ?? params.chunk ?? params.content), "read", {
        kind: "read",
        itemId: params.itemId,
        turnId: eventTurnId(payload),
      });
      return true;
    }
    if (method === "item/commandExecution/outputDelta"
      || method === "command/exec/outputDelta"
      || method === "process/outputDelta") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      const stream = params.stream || params.channel || (params.stderr ? "stderr" : "stdout");
      appendOutputChunk(textFromValue(params.delta ?? params.text ?? params.output ?? params.chunk), stream, {
        kind: "tool",
        itemId: params.itemId,
        turnId: eventTurnId(payload),
      });
      return true;
    }
    if (method === "item/reasoning/summaryTextDelta"
      || method === "item/reasoning/textDelta"
      || method === "item/plan/delta") {
      if (!eventBelongsToCurrentTurn(payload)) return true;
      appendOutputChunk(textFromValue(params.delta ?? params.text), "reasoning", {
        kind: method === "item/plan/delta" ? "plan" : "reasoning",
        itemId: params.itemId,
        turnId: eventTurnId(payload),
      });
      return true;
    }
    if (method === "turn/started") {
      const turn = isRecord(params.turn) ? params.turn : params;
      const startedTurnId = turn.id || params.turnId;
      const workStart = timestampMs(
        turn.firstTurnWorkItemStartedAtMs,
        turn.workStartedAtMs,
        params.firstTurnWorkItemStartedAtMs,
        params.workStartedAtMs,
      );
      if (workStart !== null) state.turnWorkStartedAt = workStart;
      startTurnClock(startedTurnId, turn.startedAtMs || params.startedAtMs, turn.elapsedMs || params.elapsedMs);
      attachPendingUserToTurn(startedTurnId);
      return true;
    }
    if (method === "turn/completed") {
      const turn = isRecord(params.turn) ? params.turn : params;
      const turnId = turn.id || params.turnId || "";
      if (!eventBelongsToCurrentTurn(payload)) return true;
      const status = normalizeActivityStatus(turn.status || params.status, "completed");
      const duration = turn.durationMs || params.durationMs;
      const worked = workedDurationFor(turn, workedDurationFor(params, null));
      const finalAssistantStart = timestampMs(turn.finalAssistantStartedAtMs, params.finalAssistantStartedAtMs);
      if (finalAssistantStart !== null) state.finalAssistantStartedAt = finalAssistantStart;
      finishActivitiesForTurn(turnId, status);
      finishAssistantStream();
      const completedTurnId = turnId || state.turnId || "";
      stopTurnClock(status, duration, turn.completedAtMs || params.completedAtMs, worked);
      appendCompletedTurnDivider(completedTurnId, status, worked ?? state.lastWorkedDurationMs ?? duration ?? state.lastTurnDurationMs);
      reconcileTurnDividers();
      if (completedTurnId) {
        state.retiredTurnIds.add(completedTurnId);
        if (state.retiredTurnIds.size > 100) state.retiredTurnIds.delete(state.retiredTurnIds.values().next().value);
      }
      state.pendingUserText = "";
      state.turnId = "";
      updateIds();
      return true;
    }
    return false;
  }

  function handleEvent(event) {
    const eventSeq = finiteNumber(event.seq);
    // The relay sends a buffered event stream followed by one control
    // `session.snapshot`. Treat that control frame as the only baseline during
    // the handshake; an older host `session.snapshot` event in the buffer is
    // just historical data and may describe a turn that already ended.
    if (state.awaitingSnapshot) return;
    // `subscribe` may replay events that are already represented by the
    // following authoritative snapshot. Ignore those frames entirely so a
    // stale task.started/task.status cannot reopen the composer state.
    if (eventSeq !== null && state.lastSnapshotSeq > 0 && eventSeq <= state.lastSnapshotSeq) return;
    if (eventSeq !== null && eventSeq > state.lastSeq) state.lastSeq = eventSeq;
    $("latestSeq").textContent = `seq ${state.lastSeq}`;
    $("lastEvent").textContent = `${event.seq || "-"} / ${event.type || "event"}`;
    const payload = isRecord(event.payload) ? { ...event.payload } : {};
    // Older bridge versions put identity/status fields on the event envelope
    // instead of inside payload. Normalize both shapes before routing so a
    // terminal event cannot be attributed to the wrong turn.
    for (const key of [
      "threadId", "turnId", "requestId", "method", "params", "status", "executionStatus", "activity", "turnStatus", "activeFlags",
      "controlMode", "mode", "targetMode", "modeEpoch", "capabilities",
      "startedAtMs", "durationMs", "completedAtMs", "workedDurationMs", "workDurationMs", "workedForMs",
      "firstTurnWorkItemStartedAtMs", "firstWorkItemStartedAtMs", "workStartedAtMs", "finalAssistantStartedAtMs",
    ]) {
      if (payload[key] === undefined && event[key] !== undefined) payload[key] = event[key];
    }
    const usagePayload = payload.tokenUsage ?? payload.latestTokenUsageInfo ?? payload.contextUsage ?? payload.usage
      ?? payload.params?.tokenUsage ?? payload.params?.latestTokenUsageInfo ?? payload.params?.contextUsage;
    if (usagePayload !== undefined) {
      state.tokenUsage = usagePayload;
      renderUsage();
    }
    const protocolMethod = protocolMethodForEvent(event, payload);
    // Do not let a replayed terminal event from an older turn overwrite the
    // timer/status of a newer turn. The protocol handler performs the same
    // check, but status is normally applied before routing the event.
    const staleTerminalEvent = (event.type === "task.finished"
      || event.type === "task.cancelled"
      || protocolMethod === "turn/completed")
      && !eventBelongsToCurrentTurn(payload);
    // Status is carried both as a typed relay field and inside the payload for
    // older clients. Apply it before routing the event so a normal output
    // delta cannot hide an active thinking/editing/approval state.
    const hasExecutionStatus = isRecord(payload.executionStatus)
      || isRecord(event.status)
      || isRecord(payload.status)
      || typeof payload.activity === "string"
      || typeof payload.turnStatus === "string"
      || Array.isArray(payload.activeFlags)
      || payload.startedAtMs !== undefined
      || payload.durationMs !== undefined;
    if (hasExecutionStatus && !staleTerminalEvent) {
      applyStatusSnapshot({
        ...payload,
        status: payload.executionStatus || event.status || payload.status,
      }, { allowTerminal: true, showIdle: false });
    }
    if (protocolMethod && handleProtocolNotification(protocolMethod, payload, event.type)) {
      if (event.type === "item.started"
        || event.type === "item.completed"
        || event.type === "task.status"
        || protocolMethod === "turn/completed") updateIds();
      return;
    }
    if (event.type === "control.mode.switching" || event.type === "control.mode.changed") {
      const requestedMode = normalizeControlMode(firstString(payload.controlMode, payload.mode, payload.targetMode));
      if (requestedMode && (requestedMode !== state.controlMode || state.modeSwitching)) {
        state.modeSwitching = true;
        state.requestedControlMode = requestedMode;
        if (state.modeRequestEpoch < 0) state.modeRequestEpoch = state.modeEpoch;
        setConversationStatus("正在切换控制模式", "active");
        updateIds();
      }
      return;
    }
    if (event.type === "session.switching") {
      state.sessionSwitching = true;
      const targetThreadId = firstString(payload.targetThreadId, payload.threadId);
      let switchContext = state.sessionSwitchContext;
      if (targetThreadId) {
        state.sessionSelectedThreadId = targetThreadId;
        switchContext = beginSessionSwitchContext(targetThreadId);
        // Route subsequent target events against the requested owner while
        // the previous transcript remains mounted as the visual fallback.
        state.threadId = targetThreadId;
        if (switchContext?.targetTitle) {
          setConversationStatus(
            uiLocale() === "en-US"
              ? `Switching to “${switchContext.targetTitle}”`
              : `正在切换到「${switchContext.targetTitle}」`,
            "active",
          );
        }
      }
      finishAssistantStream();
      // Keep the previous transcript mounted until the target's authoritative
      // snapshot arrives. The bridge may emit `session.switching` before
      // owner discovery/follow completes; clearing here made a failed switch
      // look like an empty conversation and left the user with no way to tell
      // whether the target had actually loaded.
      setSessionSwitchingVisual(true);
      if (!switchContext?.targetTitle) setConversationStatus("正在切换会话", "active");
      renderSessionPicker();
      return;
    }
    if (event.type === "session.selected") {
      const selectedThreadId = firstString(payload.threadId, payload.activeThreadId);
      const switchContext = state.sessionSwitchContext;
      if (payload.failed === true) {
        // VS Code-driven attachment changes have no browser command result.
        // The adapter therefore publishes an explicit failed selection for
        // the previous thread. Roll routing/title back even if a target
        // snapshot was already rendered while the adapter validated its owner.
        const previousThreadId = firstString(switchContext?.previousThreadId, selectedThreadId);
        restoreSessionSwitchContext();
        if (previousThreadId) {
          state.threadId = previousThreadId;
          state.sessionSelectedThreadId = previousThreadId;
          syncSessionActive(previousThreadId);
        }
        state.sessionListError = sessionErrorMessage(payload, "会话切换失败，已恢复原会话");
        setConversationStatus(state.sessionListError, "warning");
        renderSessionPicker();
        return;
      }

      const matchesTarget = Boolean(switchContext
        && selectedThreadId
        && switchContext.targetThreadId === selectedThreadId);
      if (selectedThreadId && (!switchContext || matchesTarget)) {
        state.sessionSelectedThreadId = selectedThreadId;
        syncSessionActive(selectedThreadId);
      }
      // This is only one half of the switch commit. An acknowledgement for a
      // superseded target is ignored, and a matching acknowledgement keeps all
      // input disabled until the target's authoritative snapshot is committed.
      if (matchesTarget) {
        switchContext.selectedAckReady = true;
        if (finishSessionSwitchIfReady()) {
          setConversationStatus("会话已切换", "ready");
        } else {
          state.sessionSwitching = true;
          setSessionSwitchingVisual(true);
          setConversationStatus("正在加载会话", "active");
        }
      } else if (!switchContext) {
        state.sessionSwitching = false;
        finishSessionSwitchContext();
      }
      renderSessionPicker();
      return;
    }
    if (/error|warning/i.test(String(event.type || ""))
      || ["error", "warning"].includes(String(payload.method || "").toLowerCase())) {
      finishAssistantStream();
      appendOutput(eventMessage(payload), /warning/i.test(String(event.type || "")) ? "meta" : "error");
      return;
    }
    if (event.type === "connection.opened" || event.type === "app.ready") {
      state.appReady = true;
      $("appState").textContent = appStatusLabel("ready");
      $("factApp").textContent = appStatusLabel("ready");
    }
    if (event.type === "connection.closed" || event.type === "host.disconnected" || event.type === "app.exited") {
      finishAssistantStream();
      const disconnectedTurnId = state.turnId;
      if (disconnectedTurnId) finishActivitiesForTurn(disconnectedTurnId, "interrupted");
      state.pendingUserText = "";
      state.appReady = false;
      state.snapshotNoticeShown = false;
      state.turnId = "";
      state.turnStartedAt = null;
      state.turnWorkStartedAt = null;
      state.finalAssistantStartedAt = null;
      state.workedDurationMs = null;
      state.lastWorkedDurationMs = null;
      state.currentActivity = "idle";
      state.currentActivityStartedAt = null;
      state.currentActivityDurationMs = null;
      state.currentActivityTurnId = "";
      state.subagents = [];
      renderSubagents();
      updateLiveActivity("idle");
      state.sessionListLoading = false;
      state.sessionListCommandId = "";
      state.newSessionCommandId = "";
      state.sessions = [];
      state.sessionFocusedId = "";
      if (state.sessionSwitching) {
        // An explicit host disconnect aborts an in-flight hand-off. Restore
        // the previous thread identity while keeping its transcript mounted.
        restoreSessionSwitchContext();
      }
      state.sessionSelectedThreadId = "";
      state.sessionListError = "VS Code 主机未连接";
      renderSessionPicker();
      $("appState").textContent = appStatusLabel("offline");
      $("factApp").textContent = appStatusLabel("offline");
      updateIds();
      updateScrollToBottom($("output"));
    }
    if (event.type === "output.snapshot") {
      const snapshotThreadId = firstString(payload.threadId, state.threadId, state.syncedThreadId);
      if (!outputProjectionAllowed(snapshotThreadId)) return;
      const committed = commitOutputProjection(snapshotThreadId, typeof payload.text === "string" ? payload.text : "", payload.messages, {
        historyComplete: snapshotHistoryComplete(payload),
        authoritativeSnapshot: true,
      });
      if (!committed) return;
      if (Array.isArray(payload.subagents)) {
        state.subagents = payload.subagents;
        renderSubagents();
      }
      return;
    }
    if ((event.type === "output.delta" || event.type === "output.chunk")
      && (payload.text || Array.isArray(payload.messages) || isRecord(payload.messagesPatch))) {
      const projectionThreadId = firstString(payload.threadId, state.threadId, state.syncedThreadId);
      if (!outputProjectionAllowed(projectionThreadId)) return;
      if (!eventBelongsToCurrentTurn(payload)) return;
      const projectionMatches = !projectionThreadId || projectionThreadId === state.syncedThreadId;
      // Attach-mode adapters include the complete role-aware projection on a
      // delta. Re-rendering that projection keeps reasoning, tools, edits and
      // assistant text in their canonical item boundaries while retaining the
      // legacy append-only text field for older bridges.
      if (Array.isArray(payload.messages)) {
        if (!projectionMatches) {
          const committed = commitOutputProjection(
            projectionThreadId,
            typeof payload.outputTail === "string" ? payload.outputTail : typeof payload.text === "string" ? payload.text : "",
            payload.messages,
            { historyComplete: snapshotHistoryComplete(payload) },
          );
          if (committed && Array.isArray(payload.subagents)) {
            state.subagents = payload.subagents;
            renderSubagents();
          }
          return;
        }
        if (Array.isArray(payload.subagents)) {
          state.subagents = payload.subagents;
          renderSubagents();
        }
        if (payload.structureChanged === false && reconcileStructuredOutput(
          typeof payload.outputTail === "string" ? payload.outputTail : payload.text,
          payload.messages,
        )) return;
        replaceOutput(
          typeof payload.outputTail === "string" ? payload.outputTail : typeof payload.text === "string" ? payload.text : "",
          payload.messages,
        );
        return;
      }
      if (isRecord(payload.messagesPatch)) {
        // A suffix patch has meaning only relative to the same thread's
        // authoritative baseline. Never apply it to the transcript retained
        // while another session is still loading.
        if (!projectionMatches) return;
        if (Array.isArray(payload.subagents)) {
          state.subagents = payload.subagents;
          renderSubagents();
        }
        const patchedMessages = applyStructuredMessagesPatch(payload.messagesPatch);
        if (patchedMessages) {
          const outputTail = typeof payload.outputTail === "string"
            ? payload.outputTail
            : typeof payload.text === "string"
              ? `${$("output")?.dataset.outputTail || ""}${payload.text}`.slice(-32_000)
              : $("output")?.dataset.outputTail || "";
          if (reconcileStructuredOutput(outputTail, patchedMessages)) return;
          replaceOutput(outputTail, patchedMessages);
          return;
        }
      }
      if (!projectionMatches) return;
      if (Array.isArray(payload.subagents)) {
        state.subagents = payload.subagents;
        renderSubagents();
      }
      appendOutputChunk(payload.text, payload.stream, {
        kind: payload.kind,
        itemId: payload.itemId,
        turnId: eventTurnId(payload),
        timestamp: payload.timestamp || payload.startedAtMs,
      });
      return;
    }
    if (event.type === "app.stderr") {
      finishAssistantStream();
      appendOutput(payload.text, "meta");
      return;
    }
    if (event.type === "approval.requested" || event.type === "input.requested" || event.type === "server.requested" || event.type === "server.request") {
      state.requests.set(requestKey(payload.requestId), {
        requestId: payload.requestId,
        method: payload.method,
        params: payload.params || payload,
        ...(payload.risk ? { risk: payload.risk } : {}),
        ...(payload.summary ? { summary: payload.summary } : {}),
        ...(payload.commandHash ? { commandHash: payload.commandHash } : {}),
        ...(payload.createdAt ? { createdAt: payload.createdAt } : {}),
        ...(payload.expiresAt ? { expiresAt: payload.expiresAt } : {}),
      });
      renderRequests();
      const inlineRequests = $("inlineRequests");
      if (inlineRequests) inlineRequests.scrollTop = inlineRequests.scrollHeight;
      finishAssistantStream();
      // The inline request card is the canonical representation. A separate
      // plain-text transcript line would duplicate the approval/input prompt
      // and could be mistaken for Codex output.
      const waitingActivity = payload.method === "item/tool/requestUserInput"
        || payload.method === "mcpServer/elicitation/request"
        ? "waiting_input"
        : "waiting_approval";
      state.currentActivity = waitingActivity;
      setConversationStatus(statusActivityLabel(waitingActivity), "warning");
      return;
    }
    if (event.type === "server.responded" || event.type === "approval.resolved" || event.type === "input.resolved" || event.type === "approval.expired" || event.type === "input.expired") {
      const key = resolveRequestKey(payload.requestId);
      state.responding.delete(key);
      state.requests.delete(key);
      renderRequests();
      return;
    }
    if (event.type === "task.status") {
      applyStatusSnapshot(payload, { allowTerminal: true, showIdle: false });
      updateIds();
      return;
    }
    if (event.type === "task.started") {
      finishAssistantStream();
      if (payload.turnId) state.turnId = payload.turnId;
      if (payload.threadId) state.threadId = payload.threadId;
      attachPendingUserToTurn(payload.turnId || state.turnId);
      applyStatusSnapshot(payload, { allowTerminal: false });
      if (!state.currentActivity || state.currentActivity === "idle") state.currentActivity = "running";
      updateLiveActivity(state.currentActivity, state.turnStartedAt, null, payload.activeFlags || [], payload.turnId || state.turnId);
      updateIds();
      return;
    }
    if (event.type === "task.finished" || event.type === "task.cancelled") {
      if (!eventBelongsToCurrentTurn(payload)) return;
      finishAssistantStream();
      const reportedStatus = payload.turnStatus
        ?? payload.status?.turnStatus
        ?? payload.status?.status
        ?? payload.status
        ?? payload.executionStatus?.turnStatus
        ?? payload.executionStatus?.status
        ?? payload.executionStatus;
      const finalStatus = event.type === "task.cancelled"
        ? "interrupted"
        : normalizeActivityStatus(reportedStatus, "completed");
      const finishedTurnId = eventTurnId(payload) || state.turnId || "";
      applyStatusSnapshot({ ...payload, turnStatus: finalStatus }, { allowTerminal: false });
      state.pendingUserText = "";
      const worked = workedDurationFor(payload, null);
      stopTurnClock(finalStatus, payload.durationMs, payload.completedAtMs, worked);
      appendCompletedTurnDivider(finishedTurnId, finalStatus, worked ?? state.lastWorkedDurationMs ?? payload.durationMs ?? state.lastTurnDurationMs);
      reconcileTurnDividers();
      if (finishedTurnId) state.retiredTurnIds.add(finishedTurnId);
      if (state.retiredTurnIds.size > 100) state.retiredTurnIds.delete(state.retiredTurnIds.values().next().value);
      if (payload.threadId) state.threadId = payload.threadId;
      state.pendingUserText = "";
      state.turnId = "";
      updateIds();
      return;
    }
    if (event.type === "session.created" && payload.thread?.id) {
      state.threadId = payload.thread.id;
      if (state.syncedThreadId === null || state.syncedThreadId === payload.thread.id) {
        prepareForSessionSnapshot(payload.thread.id);
        applySessionMetadata({ ...(isRecord(payload.metadata) ? payload.metadata : {}), ...payload }, payload);
      } else {
        setConversationStatus("正在加载会话", "active");
      }
      updateIds();
      return;
    }
    if (event.type === "session.snapshot") {
      state.awaitingSnapshot = false;
      state.appReady = true;
      const snapshotThreadId = payload.threadId || "";
      applyControlModeSnapshot(payload.metadata);
      const waitingForSession = payload.state === "waiting_for_host"
        || (isRecord(payload.metadata) && payload.metadata.waitingForSession === true);
      const eventSnapshotSeq = finiteNumber(payload.latestSeq, event.seq);
      if (eventSnapshotSeq !== null) state.lastSnapshotSeq = eventSnapshotSeq;
      const adapterName = payload.metadata && payload.metadata.adapter;
      if (adapterName === "codex-ipc-follower") setAttachMode(true);
      else if (adapterName) setAttachMode(false);
      const hasProjection = typeof payload.outputTail === "string" || Array.isArray(payload.messages);
      const projectionCommitted = hasProjection && commitOutputProjection(
        snapshotThreadId,
        typeof payload.outputTail === "string" ? payload.outputTail : "",
        payload.messages,
        {
          historyComplete: snapshotHistoryComplete(payload, payload.state),
          authoritativeSnapshot: true,
        },
      );
      const appliesToView = projectionCommitted || snapshotThreadId === state.syncedThreadId;
      if (!appliesToView) {
        // Keep the retained transcript and loading state until a matching,
        // non-placeholder snapshot is available for the selected thread.
        updateIds();
        return;
      }
      prepareForSessionSnapshot(snapshotThreadId);
      applySessionMetadata({ ...(isRecord(payload.metadata) ? payload.metadata : {}), ...payload }, payload.state);
      if (Array.isArray(payload.subagents)) {
        state.subagents = payload.subagents;
        renderSubagents();
      } else if (isRecord(payload.state) && Array.isArray(payload.state.subagents)) {
        state.subagents = payload.state.subagents;
        renderSubagents();
      }
      if (payload.threadId !== undefined) state.threadId = payload.threadId || "";
      if (payload.turnId !== undefined) state.turnId = payload.turnId || "";
      applyStatusSnapshot({
        ...payload,
        ...(payload.executionStatus ? { status: payload.executionStatus } : {}),
        turnId: payload.turnId !== undefined ? payload.turnId : state.turnId,
      }, { allowTerminal: true, showIdle: false });
      if (payload.metadata && typeof payload.metadata.cwd === "string") $("cwdInput").value = payload.metadata.cwd;
      reconcileSnapshotTerminalState(payload, payload.state);
      if (Array.isArray(payload.pendingRequests)) {
        state.requests = new Map(payload.pendingRequests
          .filter((request) => request && request.requestId !== undefined)
          .map((request) => [requestKey(request.requestId), request]));
        state.responding.clear();
        renderRequests();
      }
      if (waitingForSession) {
        setConversationStatus("等待在 VS Code 中打开 Codex 会话", "active");
        state.snapshotNoticeShown = false;
      } else if (!state.snapshotNoticeShown && state.turnStartedAt === null && (!state.currentActivity || state.currentActivity === "idle")) {
        setConversationStatus("ready");
        state.snapshotNoticeShown = true;
      }
      updateIds();
      return;
    }
    if (event.type === "session.closed") {
      finishAssistantStream();
      state.pendingUserText = "";
      state.appReady = false;
      state.sessions = [];
      state.sessionFocusedId = "";
      state.sessionSelectedThreadId = "";
      state.threadId = "";
      state.turnId = "";
      const closedTitle = $("threadTitle");
      if (closedTitle) closedTitle.textContent = "Codex";
      state.turnStartedAt = null;
      state.turnWorkStartedAt = null;
      state.finalAssistantStartedAt = null;
      state.workedDurationMs = null;
      state.lastWorkedDurationMs = null;
      state.currentActivity = "idle";
      state.currentActivityStartedAt = null;
      state.subagents = [];
      resetSessionModelMetadata();
      renderSubagents();
      updateLiveActivity("idle");
      state.sessionSwitching = false;
      state.sessionSelectCommandId = "";
      finishSessionSwitchContext();
      setConversationStatus("会话已关闭", "warning");
      state.sessionListError = "VS Code 会话已关闭";
      renderSessionPicker();
      updateIds();
      return;
    }
    if (event.type === "app.notification") {
      if (payload.method === "thread/started" && payload.params?.thread?.id) state.threadId = payload.params.thread.id;
      if (payload.method === "turn/started" && payload.params?.turn?.id) {
        state.turnId = payload.params.turn.id;
        attachPendingUserToTurn(state.turnId);
      }
      if (payload.method === "turn/completed") {
        state.turnId = "";
        finishAssistantStream();
      }
      if (payload.text) {
        finishAssistantStream();
        appendOutput(payload.text);
      }
      else if (["thread/started", "turn/started", "turn/completed", "thread/status/changed"].includes(payload.method)) appendOutput(`${payload.method}`, "meta");
      updateIds();
      return;
    }
    if (event.type === "thread.started" && payload.params?.thread?.id) {
      state.threadId = payload.params.thread.id;
      updateIds();
      return;
    }
    if (event.type === "turn.started" && payload.params?.turn?.id) {
      state.turnId = payload.params.turn.id;
      attachPendingUserToTurn(state.turnId);
      updateIds();
      return;
    }
    if (event.type === "turn.completed") {
      finishAssistantStream();
      const completedTurnId = eventTurnId(payload) || state.turnId || "";
      const status = normalizeActivityStatus(payload.status, "completed");
      const worked = workedDurationFor(payload, null);
      stopTurnClock(status, payload.durationMs, payload.completedAtMs, worked);
      appendCompletedTurnDivider(completedTurnId, status, worked ?? state.lastWorkedDurationMs ?? payload.durationMs ?? state.lastTurnDurationMs);
      reconcileTurnDividers();
      state.turnId = "";
      updateIds();
      return;
    }
    if (event.type === "command.result") {
      const commandId = payload.commandId;
      // The relay sends a sequenced event to every subscriber and a direct
      // acknowledgement to the originating browser. Render a result once.
      if (commandId) {
        const key = String(commandId);
        if (state.commandResults.has(key)) return;
        state.commandResults.add(key);
        if (state.commandResults.size > 2_000) state.commandResults.delete(state.commandResults.values().next().value);
      }
      const commandMethodName = sessionCommandMethod(payload.method);
      if (commandMethodName === "control/mode/set") {
        state.modeCommandId = "";
        if (!payload.ok) {
          clearControlModeRequest();
          setConversationStatus(sessionErrorMessage(payload, "控制模式切换失败"), "warning");
        } else if (state.modeSwitching) {
          // The command result is only an acknowledgement. Keep the requested
          // segment pending until a newer authoritative snapshot supplies the
          // resulting modeEpoch and capabilities.
          setConversationStatus("正在切换控制模式", "active");
        }
        updateIds();
        return;
      }
      if (commandMethodName === "session/list" || commandMethodName === "thread/list") {
        if (!payload.ok) {
          state.sessionListLoading = false;
          state.sessionListError = eventMessage(payload);
          state.sessionListError = sessionErrorMessage(payload, state.sessionListError);
          renderSessionPicker();
        } else {
          applySessionListResult(payload.result ?? payload);
        }
        return;
      }
      if (commandMethodName === "session/select" || commandMethodName === "thread/select") {
        if (!payload.ok) {
          state.sessionListError = failSessionSwitch(payload, eventMessage(payload));
          renderSessionPicker();
          setConversationStatus(state.sessionListError, "warning");
          requestRefresh();
        } else {
          applySessionSelectResult(payload.result ?? payload);
        }
        return;
      }
      if (commandMethodName === "session/new" || commandMethodName === "thread/new") {
        state.newSessionCommandId = "";
        if (!payload.ok) {
          const detail = eventMessage(payload);
          setConversationStatus(
            uiLocale() === "en-US"
              ? `Unable to create a new conversation: ${detail}`
              : `新会话创建失败：${detail}`,
            "warning",
          );
        } else {
          setConversationStatus("新会话已在 VS Code 中打开", "ready");
          // The official command opens the new panel asynchronously. Give the
          // host a short window to publish its rollout/owner, then refresh the
          // same history menu so it can be selected without leaving the web UI.
          openSessionHistory();
          let refreshAttempts = 0;
          const refreshNewSession = () => {
            if (!state.sessionPickerOpen || refreshAttempts >= 4) return;
            refreshAttempts += 1;
            if (!state.sessionListLoading) requestSessionList();
            setTimeout(refreshNewSession, 700);
          };
          setTimeout(refreshNewSession, 500);
        }
        updateIds();
        return;
      }
      if (payload.method === "thread/settings/update") {
        state.modelUpdatePending = false;
        if (!payload.ok) {
          const detail = eventMessage(payload);
          appendOutput(
            uiLocale() === "en-US"
              ? `Unable to update model settings: ${detail}`
              : `模型设置更新失败：${detail}`,
            "error",
          );
        } else setConversationStatus(t("模型设置已更新"), "ready");
        renderModelPicker();
        return;
      }
      if (!payload.ok) {
        const methodLabel = payload.method || t("命令");
        const uncertainty = payload.uncertain
          ? uiText("（执行状态未知，请等待主机恢复）", " (execution status unknown; wait for the host to recover)")
          : "";
        appendOutput(`${methodLabel}: ${JSON.stringify(payload.error)}${uncertainty}`, "error");
      }
      else {
        const result = payload.result || {};
        if (payload.method === "thread/start" && result.thread?.id) state.threadId = result.thread.id;
        if (payload.method === "turn/start" && result.turn?.id) state.turnId = result.turn.id;
        if (payload.method === "turn/start" && result.turn?.id) attachPendingUserToTurn(result.turn.id);
        if (payload.method === "thread/start") {
          applySessionMetadata({ ...result, ...(isRecord(result.thread) ? result.thread : {}) }, result);
        }
        finishAssistantStream();
        appendOutput(`${payload.method || "命令"} 完成`, "meta");
        updateIds();
      }
    }
  }

  function handleMessage(message) {
    if (message.type === "auth.ok") {
      state.role = message.role;
      setAuthRequired(message.authRequired);
      $("roleBadge").textContent = message.role;
      $("roleBadge").className = `badge ${message.role === "operator" ? "" : "warning"}`;
      setConnection("pending", "同步中");
      state.awaitingSnapshot = true;
      sendFrame({ type: "subscribe", fromSeq: state.lastSeq });
      return;
    }
    // A host event can legitimately have the same type as a relay control
    // frame (notably `session.snapshot`). Route the envelope by `kind` first
    // so its payload is not mistaken for the compact control shape below.
    if (message.kind === "event") {
      handleEvent(message);
      return;
    }
    if (message.type === "session.snapshot") {
      state.awaitingSnapshot = false;
      const snapshot = message.snapshot || {};
      const appState = snapshot.state || {};
      const snapshotThreadId = appState.activeThreadId || "";
      const controlMetadata = {
        ...(isRecord(snapshot.metadata) ? snapshot.metadata : {}),
        ...(isRecord(appState.sessionMetadata) ? appState.sessionMetadata : {}),
      };
      applyControlModeSnapshot(controlMetadata);
      const waitingForSession = controlMetadata.waitingForSession === true
        || (!snapshotThreadId && controlMetadata.attachReady === false);
      $("appState").textContent = appStatusLabel(appState.app);
      $("factApp").textContent = appState.app ? appStatusLabel(appState.app) : "-";
      $("factClients").textContent = String((snapshot.clients || []).length);
      state.appReady = appState.app === "ready" || appState.initialized === true;
      if (appState.mode === "host") setAttachMode(true);
      if (appState.mode === "embedded") setAttachMode(false);
      const snapshotSeq = finiteNumber(snapshot.latestSeq);
      if (snapshotSeq !== null) {
        state.lastSeq = snapshotSeq;
        state.lastSnapshotSeq = snapshotSeq;
      }
      // The control snapshot is authoritative for routing, but its transcript
      // fields can still be placeholders while VS Code is loading history.
      // Route target events immediately without replacing the retained view.
      if (snapshotThreadId || state.syncedThreadId === null) state.threadId = snapshotThreadId;
      state.turnId = appState.activeTurnId || "";
      state.requests = new Map((snapshot.pendingRequests || []).map((request) => [requestKey(request.requestId), request]));
      state.responding.clear();
      // `subscribe` replays buffered events before sending this control
      // snapshot. The snapshot is authoritative, so reconcile once at the
      // end of the replay instead of leaving transient duplicate bubbles.
      const hasProjection = typeof snapshot.outputTail === "string" || Array.isArray(snapshot.messages);
      const projectionCommitted = hasProjection && commitOutputProjection(
        snapshotThreadId,
        typeof snapshot.outputTail === "string" ? snapshot.outputTail : "",
        snapshot.messages,
        {
          historyComplete: snapshotHistoryComplete(snapshot, snapshot.metadata, appState),
          authoritativeSnapshot: true,
        },
      );
      const appliesToView = projectionCommitted || snapshotThreadId === state.syncedThreadId;
      if (appliesToView) {
        prepareForSessionSnapshot(snapshotThreadId);
        applySessionMetadata({
          ...controlMetadata,
          ...appState,
        }, appState);
        if (Array.isArray(snapshot.subagents)) {
          state.subagents = snapshot.subagents;
          renderSubagents();
        } else if (Array.isArray(appState.subagents)) {
          state.subagents = appState.subagents;
          renderSubagents();
        }
        applyStatusSnapshot({
          ...(snapshot.status ? { status: snapshot.status } : {}),
          ...(snapshot.executionStatus ? { status: snapshot.executionStatus } : {}),
          ...(snapshot.state && typeof snapshot.state === "object" ? snapshot.state : {}),
          turnId: state.turnId,
        }, { allowTerminal: true, showIdle: false });
        reconcileSnapshotTerminalState(snapshot, appState);
      } else if (state.sessionSwitching || hasVisibleOutputProjection()) {
        setConversationStatus("正在加载会话", "active");
      }
      renderRequests();
      updateIds();
      setConnection("online", "已连接");
      if (waitingForSession) {
        setConversationStatus("等待在 VS Code 中打开 Codex 会话", "active");
        state.snapshotNoticeShown = false;
      } else {
        $("outputHint").textContent = `${appStatusLabel(appState.app || "app-server")} / ${state.role}`;
      }
      if (state.sessionPickerOpen && state.appReady) requestSessionList();
      return;
    }
    if (message.type === "resync.required") {
      appendOutput("事件窗口已过期，请以当前快照为准", "error");
      return;
    }
    if (message.type === "response.accepted") {
      const key = resolveRequestKey(message.requestId);
      state.responding.delete(key);
      state.requests.delete(key);
      renderRequests();
      appendOutput(`请求 #${message.requestId} 已提交`, "meta");
      return;
    }
    if (message.type === "response.pending") {
      appendOutput(`请求 #${message.requestId} 已发送，等待 VS Code 主机确认`, "meta");
      return;
    }
    if (message.type === "command.accepted") return;
    if (message.type === "command.rejected" || message.type === "response.rejected") {
      if (message.requestId !== undefined) state.responding.delete(resolveRequestKey(message.requestId));
      const rejectedId = String(message.commandId || "");
      if (rejectedId && rejectedId === String(state.modeCommandId || "")) {
        clearControlModeRequest();
        setConversationStatus(sessionErrorMessage(message, "控制模式切换失败"), "warning");
        updateIds();
        return;
      }
      if (rejectedId && rejectedId === String(state.newSessionCommandId || "")) {
        state.newSessionCommandId = "";
        const detail = message.message || message.code || t("未知错误");
        setConversationStatus(
          uiLocale() === "en-US"
            ? `Unable to create a new conversation: ${detail}`
            : `新会话创建失败：${detail}`,
          "warning",
        );
        updateIds();
        return;
      }
      if (rejectedId && rejectedId === String(state.sessionListCommandId || "")) {
        state.sessionListLoading = false;
        state.sessionListCommandId = "";
        state.sessionListError = sessionErrorMessage(message, "无法读取会话");
        renderSessionPicker();
        return;
      }
      if (rejectedId && rejectedId === String(state.sessionSelectCommandId || "")) {
        state.sessionListError = failSessionSwitch(message, "会话切换失败");
        renderSessionPicker();
        setConversationStatus(state.sessionListError, "warning");
        requestRefresh();
        return;
      }
      appendOutput(`${message.code}: ${message.message}`, "error");
      renderRequests();
      return;
    }
    if (message.type === "command.result") {
      handleEvent({ type: "command.result", seq: message.seq, payload: message });
      return;
    }
    if (message.type === "error") appendOutput(message.message || "relay error", "error");
  }

  function embeddedSocketUrl(value) {
    if (!embeddedInAether) return "";
    const raw = String(value || "").trim();
    if (!raw) return "";
    try {
      const url = new URL(raw, location.href);
      const expectedProtocol = location.protocol === "https:" ? "wss:" : "ws:";
      if (url.origin !== `${location.protocol}//${location.host}` && url.origin !== `${expectedProtocol}//${location.host}`) {
        throw new Error(t("云端连接地址必须与当前页面同源"));
      }
      if (url.protocol === "http:") url.protocol = "ws:";
      if (url.protocol === "https:") url.protocol = "wss:";
      if (url.protocol !== "ws:" && url.protocol !== "wss:") throw new Error(t("云端连接配置无效"));
      return url.href;
    } catch (error) {
      appendOutput(error?.message || t("云端连接配置无效"), "error");
      embedBridge.reportState("error", { code: "invalid_ws_url", message: error?.message || "invalid ws url" });
      return "";
    }
  }

  function requestEmbedTicket(reason = "missing") {
    if (!embeddedInAether || state.embedStopped || state.embedTicketRequested) return;
    state.embedTicketRequested = true;
    setConversationStatus(t("正在获取新的连接凭证"), "active");
    embedBridge.requestTicket({ reason, deviceId: state.embedDeviceId || undefined });
  }

  function disconnectEmbedded(reason = "parent") {
    if (!embeddedInAether) return;
    state.embedStopped = true;
    state.embedTicket = "";
    state.embedTicketRequested = false;
    clearTimeout(state.reconnectTimer);
    state.reconnectTimer = null;
    const socket = state.ws;
    state.ws = null;
    if (socket && socket.readyState <= WebSocket.OPEN) socket.close(1000, reason);
    setConnection("offline", "云端连接已断开");
    setConversationStatus(t("父页面已断开连接"), "warning");
  }

  function applyEmbedConnection(message) {
    if (!embeddedInAether) return;
    if (message.locale) i18n?.setLocale?.(message.locale, { persist: false });
    const ticket = typeof message.ticket === "string" ? message.ticket.trim() : "";
    const wsUrl = embeddedSocketUrl(message.wsUrl || "/api/vscodex/ws");
    if (!ticket || !wsUrl) {
      appendOutput(t("云端连接配置无效"), "error");
      embedBridge.reportState("error", { code: "invalid_connection_config", message: "ticket and wsUrl are required" });
      requestEmbedTicket("invalid");
      return;
    }
    state.embedStopped = false;
    state.embedTicketRequested = false;
    state.embedTicket = ticket;
    state.embedWsUrl = wsUrl;
    state.embedDeviceId = typeof message.deviceId === "string" ? message.deviceId : state.embedDeviceId;
    setAuthRequired(true);
    document.body.classList.add("embed-aether");
    setConversationStatus(t("正在连接云端会话"), "active");
    if (state.ws && state.ws.readyState <= WebSocket.OPEN) {
      const socket = state.ws;
      state.ws = null;
      socket.close(1000, "connection replaced");
    }
    connect();
  }

  function connect() {
    if (state.ws && state.ws.readyState <= WebSocket.OPEN) return;
    if (embeddedInAether) {
      if (state.embedStopped) return;
      if (!state.embedTicket || !state.embedWsUrl) { requestEmbedTicket("missing"); return; }
      state.token = state.embedTicket;
    } else state.token = $("tokenInput").value.trim();
    if (!state.token && state.authRequired === true) { appendOutput("当前 relay 需要 token", "error"); return; }
    setConnection("pending", "连接中");
    const protocol = location.protocol === "https:" ? "wss:" : "ws:";
    let socket;
    try {
      socket = new WebSocket(embeddedInAether ? state.embedWsUrl : `${protocol}//${location.host}/ws`);
    } catch (error) {
      if (embeddedInAether) {
        state.embedTicket = "";
        requestEmbedTicket("socket-error");
      }
      appendOutput(error?.message || "WebSocket 未连接", "error");
      return;
    }
    const connectionTicket = embeddedInAether ? state.embedTicket : "";
    if (embeddedInAether) {
      state.embedTicket = "";
      state.token = "";
    }
    state.ws = socket;
    socket.addEventListener("open", () => {
      if (state.ws !== socket) return;
      // Send a hello even when local auth is disabled so the relay can assign
      // the browser role without requiring a dummy password.
      socket.send(JSON.stringify({ v: 1, kind: "hello", clientType: "web", protocol: 1 }));
      // A loopback relay authenticates on hello. Do not send a stale token as
      // a second frame after that handshake, because it is already complete.
      const token = embeddedInAether ? connectionTicket : state.token;
      if (token && state.authRequired !== false) socket.send(JSON.stringify({ type: "auth", token }));
    });
    socket.addEventListener("message", (event) => {
      if (state.ws !== socket) return;
      try { handleMessage(JSON.parse(event.data)); } catch { appendOutput("收到无法解析的 relay 消息", "error"); }
    });
    socket.addEventListener("close", (event) => {
      if (state.ws !== socket) return;
      setConnection("offline", event.code === 1008 ? "认证失败，准备重连" : "准备重连");
      state.appReady = false;
      state.turnId = "";
      state.sessionListLoading = false;
      state.sessionListCommandId = "";
      state.newSessionCommandId = "";
      clearControlModeRequest();
      state.sessions = [];
      state.sessionFocusedId = "";
      // A transport reconnect may resume the same owner hand-off. Preserve
      // its previous/target identities so the next control placeholder cannot
      // be mistaken for an authoritative empty target transcript.
      if (!state.sessionSwitching) state.sessionSelectedThreadId = "";
      state.sessionListError = "等待 relay 连接";
      updateIds();
      state.ws = null;
      renderSessionPicker();
      clearTimeout(state.reconnectTimer);
      state.reconnectTimer = null;
      if (embeddedInAether) {
        if (!state.embedStopped) requestEmbedTicket(event.code === 1008 ? "ticket-rejected" : "disconnected");
      } else state.reconnectTimer = setTimeout(connect, 3000);
    });
    socket.addEventListener("error", () => {
      // The close handler owns retrying. Keep transient socket errors in the
      // connection indicator instead of adding noisy messages to the turn.
      if (state.ws === socket) setConnection("offline", "重连中");
    });
  }

  function closePopovers() {
    for (const id of ["panelMenu", "detailsPopover", "sessionPicker", "composerPlusMenu"]) {
      const element = $(id);
      if (element) element.hidden = true;
    }
    state.sessionPickerOpen = false;
    state.sessionSearch = "";
    state.sessionFocusedId = "";
    $("sessionPickerButton")?.setAttribute("aria-expanded", "false");
    const sessionSearchInput = $("sessionSearchInput");
    if (sessionSearchInput) {
      sessionSearchInput.value = "";
      sessionSearchInput.setAttribute("aria-expanded", "false");
      sessionSearchInput.setAttribute("aria-activedescendant", "");
    }
    $("sessionSearchClear")?.setAttribute("hidden", "");
    setModelMenu(false);
    setPermissionMenu(false);
    setUsageMenu(false);
    $("composerPlusButton")?.setAttribute("aria-expanded", "false");
    const confirm = $("permissionConfirm");
    if (confirm) { confirm.hidden = true; delete confirm.dataset.pendingMode; }
  }

  function requestRefresh() {
    if (state.ws && state.ws.readyState === WebSocket.OPEN) {
      try { sendFrame({ type: "subscribe", fromSeq: state.lastSeq }); } catch { connect(); }
    } else connect();
  }

  document.querySelectorAll("[data-panel-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const action = button.dataset.panelAction;
      if (action === "back" || action === "history") {
        setSessionPicker(!state.sessionPickerOpen);
        return;
      }
      if (action === "new-session") {
        requestNewSession();
        return;
      }
      if (action === "expand") {
        document.body.classList.toggle("panel-expanded");
        return;
      }
      if (action === "close") {
        document.body.classList.add("panel-hidden");
        const restore = $("restorePanel");
        if (restore) restore.hidden = false;
        closePopovers();
        return;
      }
      if (action === "refresh") { closePopovers(); requestRefresh(); return; }
      if (action === "menu") {
        const menu = $("panelMenu");
        const details = $("detailsPopover");
        if (details) details.hidden = true;
        if (menu) menu.hidden = !menu.hidden;
        return;
      }
      if (action === "settings") {
        const details = $("detailsPopover");
        const menu = $("panelMenu");
        if (menu) menu.hidden = true;
        if (details) details.hidden = !details.hidden;
        updateIds();
      }
    });
  });
  $("sessionPickerButton")?.addEventListener("click", () => {
    setSessionPicker(!state.sessionPickerOpen);
  });
  $("controlModeSwitch")?.querySelectorAll("[data-control-mode]").forEach((button) => {
    button.addEventListener("click", () => requestControlMode(button.dataset.controlMode));
  });
  $("sessionPickerRefresh")?.addEventListener("click", (event) => {
    event.stopPropagation();
    requestSessionList();
  });
  $("sessionSearchInput")?.addEventListener("input", (event) => {
    const input = event.currentTarget;
    if (!(input instanceof HTMLInputElement)) return;
    state.sessionSearch = input.value;
    state.sessionFocusedId = "";
    renderSessionPicker();
  });
  $("sessionSearchInput")?.addEventListener("keydown", handleSessionPickerKeydown);
  $("sessionList")?.addEventListener("keydown", handleSessionPickerKeydown);
  $("sessionSearchClear")?.addEventListener("click", (event) => {
    event.preventDefault();
    event.stopPropagation();
    state.sessionSearch = "";
    state.sessionFocusedId = "";
    renderSessionPicker();
    $("sessionSearchInput")?.focus();
  });
  $("restorePanel")?.addEventListener("click", () => {
    document.body.classList.remove("panel-hidden");
    $("restorePanel").hidden = true;
  });
  $("panelMenu")?.querySelectorAll("[data-menu-action]").forEach((button) => {
    button.addEventListener("click", (event) => {
      if (button.dataset.menuAction === "sessions") {
        // The document-level outside-click handler runs in the same bubble
        // phase. Keep the picker open when it is launched from this menu.
        event.stopPropagation();
        closePopovers();
        setSessionPicker(true);
        return;
      } else if (button.dataset.menuAction === "clear") {
        renderEmptyOutput();
        state.outputSynced = false;
      } else if (button.dataset.menuAction === "refresh") requestRefresh();
      else if (button.dataset.menuAction === "expand") document.body.classList.toggle("panel-expanded");
      else if (button.dataset.menuAction === "close") {
        document.body.classList.add("panel-hidden");
        const restore = $("restorePanel");
        if (restore) restore.hidden = false;
      }
      closePopovers();
    });
  });
  $("detailsPopover")?.querySelectorAll("[data-settings-action]").forEach((button) => {
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      const action = button.dataset.settingsAction;
      closePopovers();
      if (action === "model") {
        setModelMenu(true);
        $("modelPickerButton")?.focus();
      } else if (action === "permission") {
        setPermissionMenu(true);
        $("permissionChip")?.focus();
      }
    });
  });
  document.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    if (!target.closest(".model-picker")) setModelMenu(false);
    if (!target.closest(".permission-menu, #permissionChip")) setPermissionMenu(false);
    if (!target.closest(".usage-menu, #usageButton")) setUsageMenu(false);
    if (!target.closest(".composer-plus-menu, #composerPlusButton")) {
      const menu = $("composerPlusMenu");
      if (menu) menu.hidden = true;
      $("composerPlusButton")?.setAttribute("aria-expanded", "false");
    }
    if (!target.closest("[data-panel-action], .panel-popover, .composer-popover, .composer-icon-button, .permission-chip, .usage-button, #sessionPickerButton")) closePopovers();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape" && state.sessionPickerOpen) {
      event.preventDefault();
      closePopovers();
      $("sessionPickerButton")?.focus();
    }
  });

  $("tokenInput").addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      connect();
    }
  });
  $("tokenInput").addEventListener("change", connect);
  $("localeSelect")?.addEventListener("change", (event) => {
    if (!embeddedInAether) i18n?.setLocale?.(event.target.value, { persist: true });
  });
  window.addEventListener("aether-vscodex:locale", () => {
    for (const activity of state.activities.values()) {
      renderActivityText(activity);
      refreshActivity(activity);
    }
    for (const [turnId, divider] of state.turnDividers) {
      const label = divider.querySelector(".turn-divider-label");
      if (!label) continue;
      const duration = finiteNumber(divider.dataset.durationMs);
      label.textContent = turnDividerLabel(divider.dataset.status || "completed", duration);
    }
    setAttachMode(state.attachMode);
    setAuthRequired(state.authRequired);
    renderSessionPicker();
    renderModelPicker();
    renderPermissionMenu();
    renderUsage();
    renderSubagents();
    updateIds();
    renderRequests();
    for (const checkbox of document.querySelectorAll(".task-list-item input[type=checkbox]")) {
      checkbox.setAttribute("aria-label", t(checkbox.checked ? "已完成" : "未完成"));
    }
  });
  $("clearOutputButton").addEventListener("click", () => {
    renderEmptyOutput();
    state.outputSynced = false;
  });
  $("startThreadButton").addEventListener("click", () => {
    if (state.attachMode) return;
    const params = { cwd: $("cwdInput").value.trim() || undefined, sandbox: $("sandboxInput").value, approvalPolicy: $("approvalInput").value };
    if ($("modelInput").value.trim()) params.model = $("modelInput").value.trim();
    command("thread/start", params);
  });
  $("startTurnButton").addEventListener("click", () => {
    const text = composerText();
    if (!text || !state.threadId) return;
    command("turn/start", {
      threadId: state.threadId,
      input: [{ type: "text", text, text_elements: [] }],
      ...(state.currentModel ? { model: state.currentModel } : {}),
      ...currentEffortParams(),
    });
    clearComposer();
  });
  $("steerButton").addEventListener("click", () => {
    const text = composerText();
    if (!text || !state.threadId || !state.turnId) return;
    command("turn/steer", {
      threadId: state.threadId,
      expectedTurnId: state.turnId,
      input: [{ type: "text", text, text_elements: [] }],
      ...(state.currentModel ? { model: state.currentModel } : {}),
      ...currentEffortParams(),
    });
    clearComposer();
  });
  $("interruptButton").addEventListener("click", () => {
    if (state.threadId && state.turnId) command("turn/interrupt", { threadId: state.threadId, turnId: state.turnId });
  });
  $("messageInput").addEventListener("keydown", (event) => {
    if (event.key !== "Enter" || event.shiftKey || event.isComposing) return;
    event.preventDefault();
    const button = state.turnId ? $("steerButton") : $("startTurnButton");
    if (button && !button.disabled) button.click();
  });
  $("messageInput").addEventListener("input", () => {
    resizeComposer();
    updateIds();
  });
  $("messageInput").addEventListener("paste", (event) => {
    event.preventDefault();
    const text = event.clipboardData?.getData("text/plain") || "";
    if (!text) return;
    const editor = $("messageInput");
    const selection = window.getSelection();
    if (editor && selection && selection.rangeCount) {
      const range = selection.getRangeAt(0);
      if (editor.contains(range.commonAncestorContainer)) {
        range.deleteContents();
        const node = document.createTextNode(text);
        range.insertNode(node);
        range.setStartAfter(node);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      } else editor.append(document.createTextNode(text));
    } else editor?.append(document.createTextNode(text));
    resizeComposer();
    updateIds();
  });
  $("modelPickerButton")?.addEventListener("click", (event) => {
    event.stopPropagation();
    const menu = $("modelMenu");
    setPermissionMenu(false);
    setUsageMenu(false);
    const plus = $("composerPlusMenu");
    if (plus) plus.hidden = true;
    $("composerPlusButton")?.setAttribute("aria-expanded", "false");
    setModelMenu(Boolean(menu?.hidden));
  });
  $("modelAdvancedToggle")?.addEventListener("click", (event) => {
    event.stopPropagation();
    state.modelAdvancedOpen = !state.modelAdvancedOpen;
    renderModelPicker();
    if (state.modelAdvancedOpen) $("modelAdvancedBack")?.focus();
    else $("modelPowerSlider")?.focus();
  });
  $("modelAdvancedBack")?.addEventListener("click", (event) => {
    event.stopPropagation();
    state.modelAdvancedOpen = false;
    renderModelPicker();
    $("modelPowerSlider")?.focus();
  });
  $("modelPowerSlider")?.addEventListener("input", (event) => {
    selectPowerIndex(event.currentTarget.value);
  });
  $("composerPlusButton")?.addEventListener("click", (event) => {
    event.stopPropagation();
    const menu = $("composerPlusMenu");
    if (!menu) return;
    const next = menu.hidden;
    menu.hidden = !next;
    $("composerPlusButton").setAttribute("aria-expanded", String(next));
    if (next) {
      setPermissionMenu(false);
      setUsageMenu(false);
      setModelMenu(false);
    }
  });
  $("permissionChip")?.addEventListener("click", (event) => {
    event.stopPropagation();
    const menu = $("permissionMenu");
    setPermissionMenu(Boolean(menu?.hidden));
    if (!menu?.hidden) {
      const plus = $("composerPlusMenu");
      if (plus) plus.hidden = true;
      $("composerPlusButton")?.setAttribute("aria-expanded", "false");
      setUsageMenu(false);
      setModelMenu(false);
    }
  });
  $("usageButton")?.addEventListener("click", (event) => {
    event.stopPropagation();
    const menu = $("usageMenu");
    setPermissionMenu(false);
    setModelMenu(false);
    const plus = $("composerPlusMenu");
    if (plus) plus.hidden = true;
    $("composerPlusButton")?.setAttribute("aria-expanded", "false");
    setUsageMenu(Boolean(menu?.hidden));
    if (!menu?.hidden) {
      setPermissionMenu(false);
      setModelMenu(false);
    }
  });
  const insertComposerText = (text) => {
    const editor = $("messageInput");
    if (!editor || !text) return;
    editor.focus();
    const selection = window.getSelection();
    if (selection && selection.rangeCount) {
      const range = selection.getRangeAt(0);
      if (editor.contains(range.commonAncestorContainer)) {
        range.deleteContents();
        const node = document.createTextNode(text);
        range.insertNode(node);
        range.setStartAfter(node);
        range.collapse(true);
        selection.removeAllRanges();
        selection.addRange(range);
      } else editor.append(document.createTextNode(text));
    } else editor.append(document.createTextNode(text));
    resizeComposer();
    updateIds();
  };
  $("composerPlusMenu")?.querySelectorAll("[data-composer-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const action = button.dataset.composerAction;
      if (action === "attach") {
        const input = $("attachmentInput");
        if (input) { input.accept = ".txt,.md,.json,.js,.ts,.tsx,.jsx,.css,.html,.yml,.yaml,.xml,.py,.go,.rs,.java,.c,.cpp,.h"; input.click(); }
      } else if (action === "photo") {
        const input = $("attachmentInput");
        if (input) { input.accept = "image/*"; input.click(); }
      } else if (action === "workspace") {
        insertComposerText("\n\n@workspace ");
        setConversationStatus("已添加工作区上下文", "ready");
      } else if (action === "web-search") {
        insertComposerText("\n\n/web-search ");
        setConversationStatus("已添加网页搜索", "ready");
      }
      const menu = $("composerPlusMenu");
      if (menu) menu.hidden = true;
      $("composerPlusButton")?.setAttribute("aria-expanded", "false");
    });
  });
  $("attachmentInput")?.addEventListener("change", async (event) => {
    const files = [...(event.target?.files || [])];
    for (const file of files) {
      try {
        if (file.type.startsWith("image/")) {
          insertComposerText(`\n\n[图片附件：${file.name}]\n`);
          continue;
        }
        const text = await file.text();
        const clipped = text.length > 80_000 ? `${text.slice(0, 80_000)}\n${t("…（文件已截断）")}` : text;
        const fence = "```";
        insertComposerText(`\n\n### ${file.name}\n\n${fence}\n${clipped}\n${fence}\n`);
      } catch {
        setConversationStatus(`无法读取 ${file.name}`, "warning");
      }
    }
    event.target.value = "";
  });
  $("permissionMenu")?.querySelectorAll("[data-permission-mode], [data-sandbox], [data-approval]").forEach((button) => {
    button.addEventListener("click", () => {
      if (button.dataset.permissionMode) selectPermissionMode(button.dataset.permissionMode);
      else if (button.dataset.sandbox) selectPermissionSetting("sandbox", button.dataset.sandbox);
      else if (button.dataset.approval) selectPermissionSetting("approval", button.dataset.approval);
    });
  });
  $("permissionConfirmCancel")?.addEventListener("click", () => {
    const confirm = $("permissionConfirm");
    if (confirm) { confirm.hidden = true; delete confirm.dataset.pendingMode; }
  });
  $("permissionConfirmAccept")?.addEventListener("click", () => {
    const confirm = $("permissionConfirm");
    const mode = confirm?.dataset.pendingMode || "full";
    if (confirm) { confirm.hidden = true; delete confirm.dataset.pendingMode; }
    applyPermissionMode(mode);
  });
  $("subagentsToggle")?.addEventListener("click", () => {
    state.subagentsCollapsed = !state.subagentsCollapsed;
    renderSubagents();
  });
  $("output").addEventListener("scroll", () => updateScrollToBottom($("output")), { passive: true });
  $("scrollToBottom")?.addEventListener("click", () => {
    const output = $("output");
    if (output) output.scrollTo({ top: output.scrollHeight, behavior: "smooth" });
  });
  if (typeof ResizeObserver === "function") {
    const output = $("output");
    const observedContent = new Set();
    const observeTranscriptContent = () => {
      const children = new Set(output ? [...output.children] : []);
      for (const child of observedContent) {
        if (children.has(child)) continue;
        layoutObserver.unobserve(child);
        observedContent.delete(child);
      }
      for (const child of children) {
        if (observedContent.has(child)) continue;
        observedContent.add(child);
        layoutObserver.observe(child);
      }
    };
    const layoutObserver = new ResizeObserver(() => {
      const distance = state.outputDistanceFromBottom;
      const following = distance <= 24;
      const anchorLocked = motionClock() < (state.timelineAnchorLockUntil || 0);
      updateScrollPadding();
      // Preserve the reader's distance from the bottom when a streamed item or
      // the composer changes height. The official thread layout uses the same
      // bottom-relative anchor instead of allowing content to jump.
      if (output && !anchorLocked) {
        if (following) scrollOutput(output, true);
        else output.scrollTop = Math.max(0, output.scrollHeight - output.clientHeight - distance);
        updateScrollToBottom(output);
      } else if (output) updateScrollToBottom(output);
      observeTranscriptContent();
    });
    layoutObserver.observe($("messageForm"));
    layoutObserver.observe($("inlineRequests"));
    if (output) layoutObserver.observe(output);
    observeTranscriptContent();
    if (typeof MutationObserver === "function" && output) {
      const childObserver = new MutationObserver(observeTranscriptContent);
      childObserver.observe(output, { childList: true });
    }
  }
  window.addEventListener("resize", updateScrollPadding, { passive: true });
  $("cwdInput").value = location.pathname === "/" ? "" : "";
  renderPermissionMenu();
  renderUsage();
  renderSessionPicker();
  resizeComposer();
  updateScrollPadding();
  updateScrollToBottom($("output"));
  updateIds();

  if (embeddedInAether) {
    document.body.classList.add("embed-aether");
    setAuthRequired(true);
    setConversationStatus(t("正在等待云端连接"), "active");
    embedBridge.on("connect", applyEmbedConnection);
    embedBridge.on("context", (message) => {
      if (message.locale) i18n?.setLocale?.(message.locale, { persist: false });
    });
    embedBridge.on("disconnect", () => disconnectEmbedded("parent disconnect"));
    embedBridge.on("error", (message) => {
      const detail = typeof message.message === "string" && message.message ? message.message : t("云端连接已断开");
      appendOutput(detail, "error");
      setConversationStatus(detail, "warning");
    });
  } else {
    // /api/health is intentionally public and only reports capability metadata.
    // Probe it first so a local relay can connect automatically without a token;
    // Authenticated deployments wait until a token is entered in the field.
    fetch("./api/health", { cache: "no-store" }).then(async (response) => {
      let health;
      try { health = await response.json(); } catch { return; }
      setAuthRequired(health.authRequired);
      if (health.authRequired === false || (health.authRequired === true && $("tokenInput").value.trim())) connect();
    }).catch(() => undefined);
  }
})();
