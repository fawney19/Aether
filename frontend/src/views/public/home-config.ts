import { computed, type Ref } from 'vue'
import { Apple, Box, Layers, Monitor, Puzzle, Terminal, Users } from 'lucide-vue-next'
import type { Component } from 'vue'

// Section index constants
export const SECTIONS = {
  HOME: 0,
  CLAUDE: 1,
  CODEX: 2,
  GEMINI: 3,
  FEATURES: 4
} as const

export type SectionIndex = (typeof SECTIONS)[keyof typeof SECTIONS]

// Section navigation configuration
export const sections = [
  { name: '首页' },
  { name: 'Claude' },
  { name: 'OpenAI' },
  { name: 'Gemini' },
  { name: '更多' }
] as const

// Feature cards data
export const featureCards = [
  {
    icon: Layers,
    title: 'Claude / OpenAI / Gemini',
    desc: '已完整接入三大主流 AI 编程助手的标准 API',
    status: 'completed' as const
  },
  {
    icon: Puzzle,
    title: '格式转换',
    desc: '开启/关闭API格式相互转换、自定义请求头',
    status: 'completed' as const
  },
  {
    icon: Users,
    title: '协同开发',
    desc: '远程开发、Skill分享、Playground等功能即将到来',
    status: 'in-progress' as const
  }
]

// CLI configuration generators
export function useCliConfigs(baseUrl: Ref<string>) {
  const claudeConfig = computed(() => `{
  "env": {
    "ANTHROPIC_AUTH_TOKEN": "your-api-key",
    "ANTHROPIC_BASE_URL": "${baseUrl.value}"
  }
}`)

  const codexConfig = computed(() => `model_provider = "aether"
model = "latest-model-name"
model_reasoning_effort = "high"
network_access = "enabled"
disable_response_storage = true

[model_providers.aether]
name = "OpenAI"
base_url = "${baseUrl.value}/v1"
wire_api = "responses"
supports_websockets = true
requires_openai_auth = true`)

  const codexAuthConfig = computed(() => `{
  "OPENAI_API_KEY": "your-api-key"
}`)

  const geminiEnvConfig = computed(() => `GOOGLE_GEMINI_BASE_URL=${baseUrl.value}
GEMINI_API_KEY=your-api-key
GEMINI_MODEL=latest-model-name`)

  const geminiSettingsConfig = computed(() => `{
  "ide": {
    "enabled": true
  },
  "security": {
    "auth": {
      "selectedType": "gemini-api-key"
    }
  }
}`)

  return {
    claudeConfig,
    codexConfig,
    codexAuthConfig,
    geminiEnvConfig,
    geminiSettingsConfig
  }
}

// CSS class constants
export const panelClasses = {
  commandPanel: 'rounded-xl border command-panel-surface',
  configPanel: 'rounded-xl border config-panel',
  panelHeader: 'px-4 py-2 panel-header',
  codeBody: 'code-panel-body',
  iconButtonSmall: [
    'flex items-center justify-center rounded-lg border h-7 w-7',
    'border-[#e5e4df] dark:border-[rgba(227,224,211,0.12)]',
    'bg-transparent',
    'text-[#666663] dark:text-[#f1ead8]',
    'transition hover:bg-[#f0f0eb] dark:hover:bg-[#3a3731]'
  ].join(' ')
} as const

// Platform option type
export interface PlatformOption {
  value: string
  label: string
  hint: string
  icon: Component
  command: string
}

// Platform presets configuration
export const platformPresets = {
  claude: {
    options: [
      { value: 'mac', label: 'Mac / Linux', hint: 'Terminal', icon: Terminal, command: 'curl -fsSL https://claude.ai/install.sh | bash' },
      { value: 'windows', label: 'Windows', hint: 'PowerShell', icon: Monitor, command: 'irm https://claude.ai/install.ps1 | iex' },
      { value: 'nodejs', label: 'Node.js', hint: 'npm', icon: Box, command: 'npm install -g @anthropic-ai/claude-code' },
      { value: 'homebrew', label: 'Mac', hint: 'Homebrew', icon: Apple, command: 'brew install --cask claude-code' }
    ] as PlatformOption[],
    defaultValue: 'mac'
  },
  codex: {
    options: [
      { value: 'nodejs', label: 'Node.js', hint: 'npm', icon: Box, command: 'npm install -g @openai/codex' },
      { value: 'homebrew', label: 'Mac', hint: 'Homebrew', icon: Apple, command: 'brew install --cask codex' }
    ] as PlatformOption[],
    defaultValue: 'nodejs'
  },
  gemini: {
    options: [
      { value: 'nodejs', label: 'Node.js', hint: 'npm', icon: Box, command: 'npm install -g @google/gemini-cli' },
      { value: 'homebrew', label: 'Mac', hint: 'Homebrew', icon: Apple, command: 'brew install gemini-cli' }
    ] as PlatformOption[],
    defaultValue: 'nodejs'
  }
} as const

// Helper to get command by platform value
export function getInstallCommand(preset: keyof typeof platformPresets, value: string): string {
  const config = platformPresets[preset]
  return config.options.find((opt) => opt.value === value)?.command ?? ''
}

// Logo type mapping
export function getLogoType(section: number): 'claude' | 'openai' | 'gemini' | 'aether' {
  switch (section) {
    case SECTIONS.CLAUDE: return 'claude'
    case SECTIONS.CODEX: return 'openai'
    case SECTIONS.GEMINI: return 'gemini'
    default: return 'aether'
  }
}

// Logo color class mapping
export function getLogoClass(section: number): string {
  switch (section) {
    case SECTIONS.CLAUDE: return 'text-[#D97757]'
    case SECTIONS.CODEX: return 'text-[#191919] dark:text-white'
    case SECTIONS.GEMINI: return '' // Gemini uses gradient
    default: return 'text-[#191919] dark:text-white'
  }
}
