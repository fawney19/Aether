import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createApp, nextTick, type App } from 'vue'
import OAuthAccountDialog from '@/features/providers/components/OAuthAccountDialog.vue'

const endpointMocks = vi.hoisted(() => ({
  startProviderLevelOAuth: vi.fn(),
  completeProviderLevelOAuth: vi.fn(),
  authorizeProviderWithCookie: vi.fn(),
  startProviderCookieAuthorizeTask: vi.fn(),
  getProviderCookieAuthorizeTaskStatus: vi.fn(),
  importProviderRefreshToken: vi.fn(),
  startBatchImportOAuthTask: vi.fn(),
  getBatchImportOAuthTaskStatus: vi.fn(),
  startDeviceAuthorize: vi.fn(),
  pollDeviceAuthorize: vi.fn(),
  getAwsRegions: vi.fn(),
}))

const toastMocks = vi.hoisted(() => ({
  success: vi.fn(),
  warning: vi.fn(),
  error: vi.fn(),
}))

vi.mock('@/api/endpoints', async () => {
  const actual = await vi.importActual<typeof import('@/api/endpoints/provider_oauth')>(
    '@/api/endpoints/provider_oauth',
  )

  return {
    ...endpointMocks,
    normalizeBatchImportCredentials: actual.normalizeBatchImportCredentials,
  }
})

vi.mock('@/components/ui', async () => {
  const { defineComponent, h } = await import('vue')

  const passthrough = (name: string, tag = 'div') => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h(tag, slots.default?.())
    },
  })

  const Dialog = defineComponent({
    name: 'DialogStub',
    props: {
      modelValue: Boolean,
    },
    setup(props, { slots }) {
      return () => {
        if (!props.modelValue) return null
        const headerActions = slots['header-actions'] ?? slots.headerActions
        return h('section', [headerActions?.(), slots.default?.(), slots.footer?.()])
      }
    },
  })

  const Button = defineComponent({
    name: 'ButtonStub',
    inheritAttrs: false,
    props: {
      disabled: Boolean,
      variant: String,
      size: String,
    },
    setup(props, { attrs, slots }) {
      return () => h('button', {
        ...attrs,
        disabled: props.disabled,
        type: attrs.type ?? 'button',
      }, slots.default?.())
    },
  })

  const Textarea = defineComponent({
    name: 'TextareaStub',
    inheritAttrs: false,
    props: {
      modelValue: {
        type: String,
        default: '',
      },
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('textarea', {
        ...attrs,
        value: props.modelValue,
        onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLTextAreaElement).value),
      })
    },
  })

  const Switch = defineComponent({
    name: 'SwitchStub',
    props: {
      modelValue: Boolean,
      disabled: Boolean,
    },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit }) {
      return () => h('button', {
        ...attrs,
        type: 'button',
        role: 'switch',
        'aria-checked': String(props.modelValue),
        disabled: props.disabled,
        onClick: () => emit('update:modelValue', !props.modelValue),
      })
    },
  })

  return {
    Dialog,
    Button,
    Textarea,
    Switch,
    Popover: passthrough('PopoverStub'),
    PopoverTrigger: passthrough('PopoverTriggerStub'),
    PopoverContent: passthrough('PopoverContentStub'),
  }
})

vi.mock('radix-vue', async () => {
  const { defineComponent, h } = await import('vue')
  const passthrough = (name: string) => defineComponent({
    name,
    setup(_, { slots }) {
      return () => h('div', slots.default?.())
    },
  })

  return {
    ComboboxAnchor: passthrough('ComboboxAnchorStub'),
    ComboboxContent: passthrough('ComboboxContentStub'),
    ComboboxEmpty: passthrough('ComboboxEmptyStub'),
    ComboboxInput: passthrough('ComboboxInputStub'),
    ComboboxItem: passthrough('ComboboxItemStub'),
    ComboboxRoot: passthrough('ComboboxRootStub'),
    ComboboxTrigger: passthrough('ComboboxTriggerStub'),
    ComboboxViewport: passthrough('ComboboxViewportStub'),
  }
})

vi.mock('@/components/common/JsonImportInput.vue', async () => {
  const { defineComponent, h } = await import('vue')

  return {
    default: defineComponent({
      name: 'JsonImportInputStub',
      props: {
        modelValue: {
          type: String,
          default: '',
        },
        dropTitle: {
          type: String,
          default: '',
        },
        dropHint: {
          type: String,
          default: '',
        },
        manualPlaceholder: {
          type: String,
          default: '',
        },
        manualDescription: {
          type: String,
          default: '',
        },
        pasteToggleText: {
          type: String,
          default: '',
        },
        fileToggleText: {
          type: String,
          default: '',
        },
      },
      emits: ['update:modelValue'],
      setup(props, { emit }) {
        return () => h('div', [
          h('p', { 'data-testid': 'drop-title' }, props.dropTitle),
          h('p', { 'data-testid': 'drop-hint' }, props.dropHint),
          h('p', { 'data-testid': 'manual-description' }, props.manualDescription),
          h('p', props.pasteToggleText),
          h('p', props.fileToggleText),
          h('textarea', {
            'data-testid': 'import-textarea',
            placeholder: props.manualPlaceholder,
            value: props.modelValue,
            onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLTextAreaElement).value),
          }),
        ])
      },
    }),
  }
})

vi.mock('@/components/ui/Label.vue', () => ({}))
vi.mock('../ProxyNodeSelect.vue', async () => {
  const { defineComponent, h } = await import('vue')
  return {
    default: defineComponent({
      name: 'ProxyNodeSelectStub',
      props: {
        modelValue: {
          type: String,
          default: '',
        },
      },
      emits: ['update:modelValue'],
      setup(_, { emit }) {
        return () => h('button', {
          type: 'button',
          'data-testid': 'proxy-node-select',
          onClick: () => emit('update:modelValue', 'proxy-node-1'),
        })
      },
    }),
  }
})

vi.mock('@/stores/proxy-nodes', () => ({
  useProxyNodesStore: () => ({
    nodes: [],
    onlineNodes: [],
    loading: false,
    ensureLoaded: vi.fn(),
  }),
}))

vi.mock('@/composables/useToast', () => ({
  useToast: () => toastMocks,
}))

vi.mock('@/composables/useClipboard', () => ({
  useClipboard: () => ({
    copyToClipboard: vi.fn(),
  }),
}))

vi.mock('@/composables/useTotp', () => ({
  useTotp: () => ({
    code: { value: '' },
    remaining: { value: 0 },
    start: vi.fn(),
    stop: vi.fn(),
  }),
}))

vi.mock('lucide-vue-next', async () => {
  const { defineComponent, h } = await import('vue')
  const Icon = defineComponent({
    name: 'IconStub',
    setup() {
      return () => h('span')
    },
  })

  return {
    UserPlus: Icon,
    Copy: Icon,
    ExternalLink: Icon,
    Globe: Icon,
    AlertCircle: Icon,
    ShieldCheck: Icon,
    ChevronsUpDown: Icon,
    Check: Icon,
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

function mountDialog(providerType = 'grok') {
  const root = document.createElement('div')
  document.body.appendChild(root)
  const app = createApp(OAuthAccountDialog, {
    open: true,
    providerId: 'provider-1',
    providerType,
  })
  app.mount(root)
  mountedApps.push({ app, root })
  return root
}

async function settle() {
  await nextTick()
  await Promise.resolve()
}

function getButton(root: HTMLElement, text: string) {
  return Array.from(root.querySelectorAll('button'))
    .find(button => button.textContent?.includes(text))
}

function getExactButton(root: HTMLElement, text: string) {
  return Array.from(root.querySelectorAll('button'))
    .find(button => button.textContent?.trim() === text)
}

function getImportTextarea(root: HTMLElement) {
  const textarea = root.querySelector('[data-testid="import-textarea"]')
  if (!(textarea instanceof HTMLTextAreaElement)) {
    throw new Error('Expected import textarea to exist')
  }
  return textarea
}

describe('OAuthAccountDialog authorization and import', () => {
  beforeEach(() => {
    endpointMocks.startProviderLevelOAuth.mockReset()
    endpointMocks.completeProviderLevelOAuth.mockReset()
    endpointMocks.authorizeProviderWithCookie.mockReset()
    endpointMocks.startProviderCookieAuthorizeTask.mockReset()
    endpointMocks.getProviderCookieAuthorizeTaskStatus.mockReset()
    endpointMocks.importProviderRefreshToken.mockReset()
    endpointMocks.startBatchImportOAuthTask.mockReset()
    endpointMocks.getBatchImportOAuthTaskStatus.mockReset()
    endpointMocks.startDeviceAuthorize.mockReset()
    endpointMocks.pollDeviceAuthorize.mockReset()
    endpointMocks.getAwsRegions.mockReset()
    toastMocks.success.mockReset()
    toastMocks.warning.mockReset()
    toastMocks.error.mockReset()

    endpointMocks.startProviderLevelOAuth.mockResolvedValue({
      authorization_url: 'https://claude.ai/oauth/authorize',
      redirect_uri: 'https://platform.claude.com/oauth/code/callback',
      provider_type: 'claude_code',
      instructions: '',
    })
    endpointMocks.authorizeProviderWithCookie.mockResolvedValue({
      key_id: 'key-claude-cookie',
      provider_type: 'claude_code',
      has_refresh_token: true,
      email: 'claude@example.com',
      replaced: false,
    })
    endpointMocks.startProviderCookieAuthorizeTask.mockResolvedValue({
      task_id: 'claude-cookie-task-1',
      status: 'submitted',
      total: 2,
      processed: 0,
      success: 0,
      failed: 0,
      progress_percent: 0,
    })
    endpointMocks.importProviderRefreshToken.mockResolvedValue({
      provider_type: 'grok',
      has_refresh_token: false,
      email: 'grok@example.com',
      replaced: false,
    })
    endpointMocks.startBatchImportOAuthTask.mockResolvedValue({
      task_id: 'task-1',
      status: 'submitted',
      total: 2,
      processed: 0,
      success: 0,
      failed: 0,
      progress_percent: 0,
    })
  })

  afterEach(() => {
    for (const { app, root } of mountedApps.splice(0)) {
      app.unmount()
      root.remove()
    }
    vi.useRealTimers()
  })

  it('opens Grok in import mode without starting unsupported OAuth', async () => {
    const root = mountDialog('grok')
    await settle()

    expect(endpointMocks.startProviderLevelOAuth).not.toHaveBeenCalled()
    expect(root.textContent).not.toContain('获取授权')
    expect(root.querySelector('textarea')?.getAttribute('placeholder')).toContain('Grok sso/session token')
    expect(root.textContent).toContain('plan_type / pool_tier')
    expect(getButton(root, '导入账号')).toBeTruthy()
  })

  it('shows Claude authorization modes in the required order', async () => {
    const root = mountDialog('claude_code')
    await settle()
    await settle()

    const modeLabels = Array.from(root.querySelectorAll('button'))
      .map(button => button.textContent?.trim())
      .filter(label => ['获取授权', 'Cookie授权', '导入授权'].includes(label || ''))

    expect(modeLabels).toEqual(['获取授权', 'Cookie授权', '导入授权'])
    expect(Array.from(root.querySelectorAll<HTMLTextAreaElement>('textarea')).map(
      textarea => textarea.placeholder,
    )).toContain('粘贴完整回调 URL 或授权码（code#state）')

    const callbackTextarea = root.querySelector<HTMLTextAreaElement>(
      '[data-testid="oauth-callback-textarea"]',
    )
    expect(callbackTextarea?.classList.contains('h-full')).toBe(true)
    expect(callbackTextarea?.classList.contains('min-h-[120px]')).toBe(true)
    expect(callbackTextarea?.parentElement?.classList.contains('flex-1')).toBe(true)

    const cookieInput = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="每行粘贴一个 sessionKey Cookie 值或完整 Cookie 请求头，最多 20 个"]',
    )
    const cookiePanel = cookieInput?.closest('[inert]')
    expect(cookiePanel?.getAttribute('aria-hidden')).toBe('true')
  })

  it('keeps Cookie authorization unavailable for non-Claude providers', async () => {
    const root = mountDialog('codex')
    await settle()

    expect(getExactButton(root, 'Cookie授权')).toBeFalsy()
  })

  it('authorizes a Claude account with a cookie and selected proxy node', async () => {
    const root = mountDialog('claude_code')
    await settle()

    getExactButton(root, 'Cookie授权')?.click()
    await settle()

    const cookieInput = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="每行粘贴一个 sessionKey Cookie 值或完整 Cookie 请求头，最多 20 个"]',
    )
    if (!cookieInput) throw new Error('Expected Claude cookie input to exist')
    expect(cookieInput.classList.contains('min-h-[200px]')).toBe(true)
    expect(cookieInput.classList.contains('h-[200px]')).toBe(true)
    expect(cookieInput.parentElement?.classList.contains('relative')).toBe(true)
    expect(root.querySelector('#claude-session-cookie-status')?.classList.contains('absolute')).toBe(true)
    expect(cookieInput.style.getPropertyValue('-webkit-text-security')).toBe('')
    expect(cookieInput.closest('[aria-hidden="true"]')).toBeNull()
    expect(cookieInput.closest('[inert]')).toBeNull()
    expect(root.querySelector('[data-testid="cookie-visibility-toggle"]')).toBeNull()

    const authorizeButton = getExactButton(root, '授权')
    expect(authorizeButton?.disabled).toBe(true)

    const proxyNodeSelect = root.querySelector<HTMLButtonElement>('[data-testid="proxy-node-select"]')
    expect(proxyNodeSelect).toBeTruthy()
    proxyNodeSelect?.click()
    await settle()
    cookieInput.value = 'Cookie: sessionKey=claude-session-key'
    cookieInput.dispatchEvent(new Event('input'))
    await settle()

    expect(authorizeButton?.disabled).toBe(false)
    authorizeButton?.click()
    await settle()

    expect(endpointMocks.authorizeProviderWithCookie).toHaveBeenCalledWith('provider-1', {
      cookie: 'Cookie: sessionKey=claude-session-key',
      proxy_node_id: 'proxy-node-1',
    })
    expect(toastMocks.success).toHaveBeenCalled()
  })

  it('authorizes multiple Claude cookies through a task and keeps only failed lines', async () => {
    vi.useFakeTimers()
    endpointMocks.getProviderCookieAuthorizeTaskStatus.mockResolvedValueOnce({
      task_id: 'claude-cookie-task-1',
      provider_id: 'provider-1',
      provider_type: 'claude_code',
      status: 'completed',
      total: 3,
      processed: 3,
      success: 2,
      failed: 1,
      created_count: 1,
      replaced_count: 1,
      progress_percent: 100,
      message: null,
      error: null,
      error_samples: [{ index: 1, status: 'error', error: 'expired cookie' }],
      created_at: 1,
      finished_at: 2,
      updated_at: 2,
    })
    const root = mountDialog('claude_code')
    await settle()

    getExactButton(root, 'Cookie授权')?.click()
    await settle()
    const cookieInput = root.querySelector<HTMLTextAreaElement>('[data-testid="claude-cookie-input"]')
    if (!cookieInput) throw new Error('Expected Claude cookie input to exist')
    cookieInput.value = [
      'sessionKey=claude-session-1',
      '',
      'Cookie: sessionKey=expired-session',
      'sessionKey=claude-session-3',
    ].join('\n')
    cookieInput.dispatchEvent(new Event('input'))
    await settle()

    const batchButton = getExactButton(root, '批量授权')
    expect(batchButton?.disabled).toBe(false)
    batchButton?.click()
    await settle()

    expect(endpointMocks.startProviderCookieAuthorizeTask).toHaveBeenCalledWith('provider-1', {
      cookies: [
        'sessionKey=claude-session-1',
        'Cookie: sessionKey=expired-session',
        'sessionKey=claude-session-3',
      ],
      proxy_node_id: undefined,
    })
    expect(getExactButton(root, '授权中...')).toBeTruthy()

    await vi.runOnlyPendingTimersAsync()
    await settle()

    expect(endpointMocks.getProviderCookieAuthorizeTaskStatus).toHaveBeenCalledWith(
      'provider-1',
      'claude-cookie-task-1',
    )
    expect(cookieInput.value).toBe('Cookie: sessionKey=expired-session')
    expect(toastMocks.warning).toHaveBeenCalledWith(
      '批量授权完成：成功 2 个（新增 1 个，替换 1 个），失败 1 个；#2 expired cookie',
      '批量授权',
    )
    expect(toastMocks.error).not.toHaveBeenCalled()
  })

  it('keeps all Claude cookie lines when a batch task has no successes', async () => {
    vi.useFakeTimers()
    endpointMocks.getProviderCookieAuthorizeTaskStatus.mockResolvedValueOnce({
      task_id: 'claude-cookie-task-1',
      provider_id: 'provider-1',
      provider_type: 'claude_code',
      status: 'completed',
      total: 4,
      processed: 4,
      success: 0,
      failed: 4,
      created_count: 0,
      replaced_count: 0,
      progress_percent: 100,
      message: null,
      error: null,
      error_samples: [
        { index: 0, status: 'error', error: 'sessionKey=must-not-leak' },
        { index: 1, status: 'error', error: 'invalid cookie' },
        { index: 2, status: 'error', error: 'expired cookie' },
        { index: 3, status: 'error', error: 'third safe reason' },
      ],
      created_at: 1,
      finished_at: 2,
      updated_at: 2,
    })
    const root = mountDialog('claude_code')
    await settle()

    getExactButton(root, 'Cookie授权')?.click()
    await settle()
    const cookieInput = root.querySelector<HTMLTextAreaElement>('[data-testid="claude-cookie-input"]')
    if (!cookieInput) throw new Error('Expected Claude cookie input to exist')
    const originalInput = [
      'sessionKey=secret',
      'sessionKey=invalid',
      'sessionKey=expired',
      'sessionKey=other',
    ].join('\n')
    cookieInput.value = originalInput
    cookieInput.dispatchEvent(new Event('input'))
    await settle()
    getExactButton(root, '批量授权')?.click()
    await settle()
    await vi.runOnlyPendingTimersAsync()
    await settle()

    expect(cookieInput.value).toBe(originalInput)
    expect(toastMocks.error).toHaveBeenCalledWith(
      '批量授权完成：成功 0 个（新增 0 个，替换 0 个），失败 4 个；#2 invalid cookie；#3 expired cookie',
      '错误',
    )
    expect(toastMocks.error.mock.calls[toastMocks.error.mock.calls.length - 1]?.[0]).not.toContain('must-not-leak')
    expect(toastMocks.error.mock.calls[toastMocks.error.mock.calls.length - 1]?.[0]).not.toContain('third safe reason')
    expect(toastMocks.warning).not.toHaveBeenCalled()
  })

  it('blocks Claude cookie batches over the 20-account limit', async () => {
    const root = mountDialog('claude_code')
    await settle()

    getExactButton(root, 'Cookie授权')?.click()
    await settle()
    const cookieInput = root.querySelector<HTMLTextAreaElement>('[data-testid="claude-cookie-input"]')
    if (!cookieInput) throw new Error('Expected Claude cookie input to exist')
    cookieInput.value = Array.from({ length: 21 }, (_, index) => `sessionKey=claude-${index}`).join('\n')
    cookieInput.dispatchEvent(new Event('input'))
    await settle()

    expect(getExactButton(root, '批量授权')?.disabled).toBe(true)
    expect(root.querySelector('#claude-session-cookie-status')?.textContent?.trim())
      .toBe('已输入 21 个，最多 20 个')
    expect(endpointMocks.startProviderCookieAuthorizeTask).not.toHaveBeenCalled()
  })

  it('uses a Claude-specific import credential placeholder', async () => {
    const root = mountDialog('claude_code')
    await settle()

    getExactButton(root, '导入授权')?.click()
    await settle()

    const textarea = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="粘贴 Claude Refresh Token 或 Claude Code .credentials.json 内容"]',
    )
    expect(textarea).toBeTruthy()
  })

  it('imports only Claude OAuth credentials from a Claude Code credentials file', async () => {
    const root = mountDialog('claude_code')
    await settle()

    getExactButton(root, '导入授权')?.click()
    await settle()

    const textarea = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="粘贴 Claude Refresh Token 或 Claude Code .credentials.json 内容"]',
    )
    if (!textarea) throw new Error('Expected Claude credentials import textarea to exist')
    textarea.value = JSON.stringify({
      claudeAiOauth: {
        accessToken: 'claude-access-token',
        refreshToken: 'claude-refresh-token',
        expiresAt: 4_102_444_800_000,
        scopes: ['user:inference'],
      },
      mcpOAuth: {
        accessToken: 'mcp-access-token-must-not-be-imported',
        refreshToken: 'mcp-refresh-token-must-not-be-imported',
      },
    })
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '导入')?.click()
    await settle()

    expect(endpointMocks.importProviderRefreshToken).toHaveBeenCalledWith('provider-1', {
      access_token: 'claude-access-token',
      refresh_token: 'claude-refresh-token',
      expires_at: 4_102_444_800,
      proxy_node_id: undefined,
    })
  })

  it('maps a single Grok JSON token into account metadata import payload', async () => {
    const root = mountDialog('grok')
    await settle()

    const textarea = getImportTextarea(root)
    textarea.value = JSON.stringify({
      token: 'sso-1',
      planType: 'super',
      tier: 'heavy',
      email: 'grok@example.com',
      accountName: 'Grok Heavy',
    })
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getButton(root, '导入账号')?.click()
    await settle()

    expect(endpointMocks.importProviderRefreshToken).toHaveBeenCalledWith('provider-1', {
      access_token: 'sso-1',
      account_name: 'Grok Heavy',
      email: 'grok@example.com',
      plan_type: 'super',
      pool_tier: 'heavy',
      sso_rw_token: undefined,
      cf_cookies: undefined,
      cf_clearance: undefined,
      user_agent: undefined,
      browser_profile: undefined,
      proxy_node_id: undefined,
      refresh_token: undefined,
      expires_at: undefined,
      name: undefined,
      account_id: undefined,
      account_user_id: undefined,
      user_id: undefined,
      headers: undefined,
    })
  })

  it('keeps Codex imported request headers on single JSON import', async () => {
    const root = mountDialog('codex')
    await settle()

    getButton(root, '导入授权')?.click()
    await settle()

    const textarea = getImportTextarea(root)
    textarea.value = JSON.stringify({
      access_token: 'jwt-access-token',
      headers: {
        authorization: 'Bearer session-token',
      },
      accountId: 'acct-1',
    })
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '导入')?.click()
    await settle()

    expect(endpointMocks.importProviderRefreshToken).toHaveBeenCalledWith('provider-1', expect.objectContaining({
      access_token: 'jwt-access-token',
      headers: {
        authorization: 'Bearer session-token',
      },
      account_id: 'acct-1',
    }))
  })

  it('sends a single Codex Agent Identity auth JSON through batch import', async () => {
    const root = mountDialog('codex')
    await settle()

    getButton(root, '导入授权')?.click()
    await settle()

    const credentials = JSON.stringify({
      auth_mode: 'agentIdentity',
      agent_identity: {
        agent_runtime_id: 'runtime-1',
        agent_private_key: 'base64-pkcs8-key',
      },
    })
    const textarea = getImportTextarea(root)
    textarea.value = credentials
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '导入')?.click()
    await settle()

    expect(endpointMocks.startBatchImportOAuthTask).toHaveBeenCalledWith(
      'provider-1',
      credentials,
      undefined,
    )
    expect(endpointMocks.importProviderRefreshToken).not.toHaveBeenCalled()
  })

  it('shows a dedicated Codex Agent Identity mode and creates from an access token', async () => {
    const root = mountDialog('codex')
    await settle()

    expect(getExactButton(root, '获取授权')).toBeTruthy()
    expect(getExactButton(root, '导入授权')).toBeTruthy()
    expect(getExactButton(root, 'Agent Identity')).toBeTruthy()

    getExactButton(root, 'Agent Identity')?.click()
    await settle()

    const textarea = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="粘贴 AT 或 ChatGPT auth/session JSON"]',
    )
    expect(textarea).toBeTruthy()
    if (!textarea) throw new Error('Expected Agent Identity access token textarea to exist')
    expect(textarea.classList.contains('min-h-[200px]')).toBe(true)
    expect(root.textContent).not.toContain('ChatGPT Session Token')
    expect(root.textContent).not.toContain('仅用于一次性注册')
    expect(root.textContent).not.toContain('创建并导入 Agent Identity')
    textarea.value = 'access-token-for-test-only'
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '创建')?.click()
    await settle()

    expect(endpointMocks.importProviderRefreshToken).toHaveBeenCalledWith('provider-1', {
      access_token: 'access-token-for-test-only',
      create_agent_identity: true,
      proxy_node_id: undefined,
    })
    expect(endpointMocks.startBatchImportOAuthTask).not.toHaveBeenCalled()
  })

  it('extracts only accessToken from ChatGPT auth/session JSON for Agent Identity', async () => {
    const root = mountDialog('codex')
    await settle()

    getExactButton(root, 'Agent Identity')?.click()
    await settle()

    const textarea = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="粘贴 AT 或 ChatGPT auth/session JSON"]',
    )
    if (!textarea) throw new Error('Expected Agent Identity access token textarea to exist')
    textarea.value = JSON.stringify({
      WARNING_BANNER: 'sensitive session data',
      accessToken: 'access-token-from-json',
      sessionToken: 'session-token-must-not-be-used',
      user: { email: 'private@example.com' },
    })
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '创建')?.click()
    await settle()

    expect(endpointMocks.importProviderRefreshToken).toHaveBeenCalledWith('provider-1', {
      access_token: 'access-token-from-json',
      create_agent_identity: true,
      proxy_node_id: undefined,
    })
    expect(endpointMocks.importProviderRefreshToken).not.toHaveBeenCalledWith(
      'provider-1',
      expect.objectContaining({ session_token: 'session-token-must-not-be-used' }),
    )
  })

  it('does not use sessionToken when ChatGPT auth/session JSON has no accessToken', async () => {
    const root = mountDialog('codex')
    await settle()

    getExactButton(root, 'Agent Identity')?.click()
    await settle()

    const textarea = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="粘贴 AT 或 ChatGPT auth/session JSON"]',
    )
    if (!textarea) throw new Error('Expected Agent Identity access token textarea to exist')
    textarea.value = JSON.stringify({ sessionToken: 'session-token-must-not-be-used' })
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '创建')?.click()
    await settle()

    expect(endpointMocks.importProviderRefreshToken).not.toHaveBeenCalled()
  })

  it('treats a saved Agent Identity with pending task initialization as accepted', async () => {
    endpointMocks.importProviderRefreshToken.mockResolvedValueOnce({
      key_id: 'key-agent',
      provider_type: 'codex',
      has_refresh_token: false,
      task_ready: false,
      recoverable: true,
      detail: 'pending task',
    })
    const root = mountDialog('codex')
    await settle()

    getExactButton(root, 'Agent Identity')?.click()
    await settle()
    const textarea = root.querySelector<HTMLTextAreaElement>(
      'textarea[placeholder="粘贴 AT 或 ChatGPT auth/session JSON"]',
    )
    if (!textarea) throw new Error('Expected Agent Identity access token textarea to exist')
    textarea.value = 'access-token-for-pending-task'
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '创建')?.click()
    await settle()

    expect(toastMocks.warning).toHaveBeenCalled()
    expect(toastMocks.error).not.toHaveBeenCalled()
  })

  it('keeps Agent Identity creation unavailable for non-Codex providers', async () => {
    const root = mountDialog('openai')
    await settle()

    expect(getExactButton(root, '获取授权')).toBeTruthy()
    expect(getExactButton(root, '导入授权')).toBeTruthy()
    expect(getExactButton(root, 'Agent Identity')).toBeFalsy()
  })

  it('sends a complete sub2api Agent Identity export through batch import', async () => {
    const root = mountDialog('codex')
    await settle()

    getButton(root, '导入授权')?.click()
    await settle()

    const credentials = JSON.stringify({
      type: 'sub2api-data',
      version: 1,
      accounts: [{
        name: 'agent@example.com',
        platform: 'openai',
        type: 'oauth',
        credentials: {
          auth_mode: 'agentIdentity',
          agent_runtime_id: 'runtime-1',
          agent_private_key: 'base64-pkcs8-key',
          task_id: 'task-1',
        },
      }],
    })
    const textarea = getImportTextarea(root)
    textarea.value = credentials
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getExactButton(root, '导入')?.click()
    await settle()

    expect(endpointMocks.startBatchImportOAuthTask).toHaveBeenCalledWith(
      'provider-1',
      credentials,
      undefined,
    )
    expect(endpointMocks.importProviderRefreshToken).not.toHaveBeenCalled()
  })

  it('keeps Grok multiline token import on the batch task path', async () => {
    const root = mountDialog('grok')
    await settle()

    const textarea = getImportTextarea(root)
    textarea.value = 'sso-1\nsso-2'
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getButton(root, '导入账号')?.click()
    await settle()

    expect(endpointMocks.startBatchImportOAuthTask).toHaveBeenCalledWith(
      'provider-1',
      '["sso-1","sso-2"]',
      undefined,
    )
    expect(endpointMocks.importProviderRefreshToken).not.toHaveBeenCalled()
  })

  it('extracts Grok account fields from a pasted browser cookie header', async () => {
    const root = mountDialog('grok')
    await settle()

    const textarea = getImportTextarea(root)
    textarea.value = 'i18nextLng=zh; cf_clearance=cf-1; sso-rw=rw-1; sso=sso-1; x-userid=user-1'
    textarea.dispatchEvent(new Event('input'))
    await settle()

    getButton(root, '导入账号')?.click()
    await settle()

    expect(endpointMocks.importProviderRefreshToken).toHaveBeenCalledWith('provider-1', expect.objectContaining({
      access_token: 'sso-1',
      sso_rw_token: 'rw-1',
      cf_cookies: 'i18nextlng=zh; cf_clearance=cf-1; x-userid=user-1',
      cf_clearance: 'cf-1',
      user_agent: expect.any(String),
      browser_profile: 'chrome136',
      user_id: 'user-1',
    }))
  })
})
