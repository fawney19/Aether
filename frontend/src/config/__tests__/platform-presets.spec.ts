import { describe, expect, it } from 'vitest'

import { defaultPlatformOptions, getInstallCommand, platformPresets } from '@/config/platform-presets'

describe('platform presets', () => {
  it('keeps the shared default options as the default preset source', () => {
    expect(platformPresets.default.options).toBe(defaultPlatformOptions)
    expect(platformPresets.default.defaultValue).toBe('mac')
  })

  it('resolves install commands from the shared preset table', () => {
    expect(getInstallCommand('claude', 'nodejs')).toBe('npm install -g @anthropic-ai/claude-code')
    expect(getInstallCommand('codex', 'homebrew')).toBe('brew install --cask codex')
    expect(getInstallCommand('gemini', 'nodejs')).toBe('npm install -g @google/gemini-cli')
  })

  it('returns an empty command for an unknown option value', () => {
    expect(getInstallCommand('claude', 'unknown')).toBe('')
  })
})
