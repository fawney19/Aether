import { Apple, Box, Monitor, Terminal } from 'lucide-vue-next'
import type { Component } from 'vue'
import type { MessageKey } from '@/i18n'

export interface PlatformOption {
  value: string
  labelKey: MessageKey
  hintKey: MessageKey
  icon: Component
  command: string
}

export const defaultPlatformOptions: PlatformOption[] = [
  { value: 'mac', labelKey: 'platform.macLinux', hintKey: 'platform.terminal', icon: Terminal, command: '' },
  { value: 'windows', labelKey: 'platform.windows', hintKey: 'platform.powershell', icon: Monitor, command: '' }
]

export const platformPresets = {
  default: {
    options: defaultPlatformOptions,
    defaultValue: 'mac'
  },
  claude: {
    options: [
      { value: 'mac', labelKey: 'platform.macLinux', hintKey: 'platform.terminal', icon: Terminal, command: 'curl -fsSL https://claude.ai/install.sh | bash' },
      { value: 'windows', labelKey: 'platform.windows', hintKey: 'platform.powershell', icon: Monitor, command: 'irm https://claude.ai/install.ps1 | iex' },
      { value: 'nodejs', labelKey: 'platform.nodejs', hintKey: 'platform.npm', icon: Box, command: 'npm install -g @anthropic-ai/claude-code' },
      { value: 'homebrew', labelKey: 'platform.mac', hintKey: 'platform.homebrew', icon: Apple, command: 'brew install --cask claude-code' }
    ] as PlatformOption[],
    defaultValue: 'mac'
  },
  codex: {
    options: [
      { value: 'nodejs', labelKey: 'platform.nodejs', hintKey: 'platform.npm', icon: Box, command: 'npm install -g @openai/codex' },
      { value: 'homebrew', labelKey: 'platform.mac', hintKey: 'platform.homebrew', icon: Apple, command: 'brew install --cask codex' }
    ] as PlatformOption[],
    defaultValue: 'nodejs'
  },
  gemini: {
    options: [
      { value: 'nodejs', labelKey: 'platform.nodejs', hintKey: 'platform.npm', icon: Box, command: 'npm install -g @google/gemini-cli' },
      { value: 'homebrew', labelKey: 'platform.mac', hintKey: 'platform.homebrew', icon: Apple, command: 'brew install gemini-cli' }
    ] as PlatformOption[],
    defaultValue: 'nodejs'
  }
} as const

export function getInstallCommand(preset: keyof typeof platformPresets, value: string): string {
  const config = platformPresets[preset]
  return config.options.find((opt) => opt.value === value)?.command ?? ''
}
