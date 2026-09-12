import { describe, expect, it } from 'vitest'

import {
  safeExternalHttpsUrl,
  safeExternalWebUrl,
  safeInternalNavigationPath,
} from '../navigationSecurity'

const unicodeSegments = ['👩\u200d💻', 'راهنمای\u200cکاربر', '\ue000']
const controlCodePoints = [
  ...Array.from({ length: 32 }, (_value, index) => index),
  ...Array.from({ length: 33 }, (_value, index) => index + 0x7f),
]

describe('safeInternalNavigationPath', () => {
  it('keeps normalized same-origin paths, queries, and fragments', () => {
    expect(safeInternalNavigationPath('/dashboard/../dashboard/api-keys?tab=active#key-1')).toBe(
      '/dashboard/api-keys?tab=active#key-1',
    )
  })

  it.each(unicodeSegments)('keeps valid Unicode query values: %s', (segment) => {
    expect(safeInternalNavigationPath(`/dashboard?search=${segment}`)).toBe(
      `/dashboard?search=${encodeURIComponent(segment)}`,
    )
  })

  it.each(controlCodePoints)('rejects control character code point %i', (codePoint) => {
    expect(safeInternalNavigationPath(`/dashboard/before${String.fromCodePoint(codePoint)}after`))
      .toBeNull()
  })

  it.each([
    'https://attacker.example/steal',
    '//attacker.example/steal',
    '/\\attacker.example/steal',
    'javascript:alert(1)',
    'dashboard',
    '/dashboard\n/steal',
  ])('rejects an unsafe post-authentication path: %s', (value) => {
    expect(safeInternalNavigationPath(value)).toBeNull()
  })
})

describe('safeExternalHttpsUrl', () => {
  it('allows HTTPS links without credentials', () => {
    expect(safeExternalHttpsUrl(' HTTPS://github.com/fawney19/Aether/releases ')).toBe(
      'https://github.com/fawney19/Aether/releases',
    )
  })

  it.each(unicodeSegments)('allows valid Unicode URL paths: %s', (segment) => {
    expect(safeExternalHttpsUrl(`https://provider.example/docs/${segment}`)).toBe(
      `https://provider.example/docs/${encodeURIComponent(segment)}`,
    )
  })

  it.each(controlCodePoints)('rejects control character code point %i', (codePoint) => {
    expect(safeExternalHttpsUrl(`https://provider.example/before${String.fromCodePoint(codePoint)}after`))
      .toBeNull()
  })

  it.each([
    'javascript:alert(1)',
    'data:text/html,attack',
    'http://github.com/fawney19/Aether/releases',
    '//attacker.example/release',
    'https://user:secret@example.com/release',
    'https:\\attacker.example/release',
    'https://trusted.example/release\n@attacker.example',
  ])('rejects an unsafe external link: %s', (value) => {
    expect(safeExternalHttpsUrl(value)).toBeNull()
  })
})

describe('safeExternalWebUrl', () => {
  it.each([
    ['https://provider.example/docs', 'https://provider.example/docs'],
    [' http://127.0.0.1:8080/docs ', 'http://127.0.0.1:8080/docs'],
  ])('allows an absolute provider website: %s', (value, expected) => {
    expect(safeExternalWebUrl(value)).toBe(expected)
  })

  it.each(unicodeSegments)('allows valid Unicode provider websites: %s', (segment) => {
    expect(safeExternalWebUrl(`https://provider.example/docs/${segment}`)).toBe(
      `https://provider.example/docs/${encodeURIComponent(segment)}`,
    )
  })

  it.each(controlCodePoints)('rejects control character code point %i', (codePoint) => {
    expect(safeExternalWebUrl(`https://provider.example/before${String.fromCodePoint(codePoint)}after`))
      .toBeNull()
  })

  it.each([
    'javascript:alert(1)',
    'data:text/html,attack',
    '//attacker.example/provider',
    'ftp://attacker.example/provider',
    'https://user:secret@example.com/provider',
    'https:\\attacker.example/provider',
    'https://trusted.example/provider\t@attacker.example',
  ])('rejects an unsafe provider website: %s', (value) => {
    expect(safeExternalWebUrl(value)).toBeNull()
  })
})
