import { computed, type Ref } from 'vue'
import { Layers, Puzzle, Users } from 'lucide-vue-next'

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
  { nameKey: 'site.home.section.home' },
  { nameKey: 'site.home.section.claude' },
  { nameKey: 'site.home.section.codex' },
  { nameKey: 'site.home.section.gemini' },
  { nameKey: 'site.home.section.more' }
] as const

// Feature cards data
export const featureCards = [
  {
    icon: Layers,
    titleKey: 'site.home.feature.cards.multi',
    descKey: 'site.home.feature.cards.multiDesc',
    status: 'completed' as const
  },
  {
    icon: Puzzle,
    titleKey: 'site.home.feature.cards.format',
    descKey: 'site.home.feature.cards.formatDesc',
    status: 'completed' as const
  },
  {
    icon: Users,
    titleKey: 'site.home.feature.cards.collaboration',
    descKey: 'site.home.feature.cards.collaborationDesc',
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
