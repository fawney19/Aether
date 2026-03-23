const COLOR_KEYS = new Set([
  'backgroundColor',
  'borderColor',
  'color',
  'pointBackgroundColor',
  'pointBorderColor',
  'pointHoverBackgroundColor',
  'pointHoverBorderColor',
  'hoverBackgroundColor',
  'hoverBorderColor',
  'tickColor',
])

const CSS_COLOR_PATTERN = /(var\(|oklch\(|hsl\(|hsla\(|rgb\(|rgba\()/i
const HSL_VAR_PATTERN = /^hsl\(var\((--[^)]+)\)(?:\s*\/\s*([^)]+))?\)$/i

let resolverElement: HTMLSpanElement | null = null

function getResolverElement(): HTMLSpanElement | null {
  if (typeof document === 'undefined') return null
  if (resolverElement && resolverElement.isConnected) return resolverElement

  const element = document.createElement('span')
  element.setAttribute('aria-hidden', 'true')
  element.style.position = 'fixed'
  element.style.pointerEvents = 'none'
  element.style.opacity = '0'
  element.style.visibility = 'hidden'
  element.style.inset = '-9999px'
  document.body.appendChild(element)
  resolverElement = element
  return resolverElement
}

export function resolveCssColor(value: string): string {
  if (!CSS_COLOR_PATTERN.test(value)) return value

  const hslVarMatch = value.trim().match(HSL_VAR_PATTERN)
  if (hslVarMatch) {
    const [, variableName, alphaValue] = hslVarMatch
    const baseColor = resolveCssColor(`var(${variableName})`)
    if (!alphaValue) return baseColor
    return applyAlpha(baseColor, alphaValue)
  }

  const element = getResolverElement()
  if (!element) return value

  element.style.color = ''
  element.style.color = value
  if (!element.style.color) {
    return value
  }
  const resolved = window.getComputedStyle(element).color.trim()

  return resolved || value
}

function applyAlpha(color: string, alphaValue: string): string {
  const alpha = parseAlpha(alphaValue)
  if (alpha == null) return color

  const rgbMatch = color.match(/^rgba?\(([^)]+)\)$/i)
  if (!rgbMatch) return color

  const parts = rgbMatch[1].split(',').map(part => part.trim())
  if (parts.length < 3) return color

  return `rgba(${parts[0]}, ${parts[1]}, ${parts[2]}, ${alpha})`
}

function parseAlpha(value: string): number | null {
  const trimmed = value.trim()
  if (trimmed.endsWith('%')) {
    const percentage = Number.parseFloat(trimmed.slice(0, -1))
    return Number.isFinite(percentage) ? percentage / 100 : null
  }

  const numeric = Number.parseFloat(trimmed)
  return Number.isFinite(numeric) ? numeric : null
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function isColorKey(key: string | undefined): boolean {
  if (!key) return false
  return COLOR_KEYS.has(key) || /color$/i.test(key)
}

function resolveValue(value: unknown, key?: string): unknown {
  if (typeof value === 'string') {
    return isColorKey(key) ? resolveCssColor(value) : value
  }

  if (Array.isArray(value)) {
    return value.map(item => resolveValue(item, key))
  }

  if (!isObject(value)) {
    return value
  }

  const output: Record<string, unknown> = {}
  for (const [childKey, childValue] of Object.entries(value)) {
    output[childKey] = resolveValue(childValue, childKey)
  }
  return output
}

export function resolveChartTheme<T>(value: T): T {
  return resolveValue(value) as T
}

export function observeChartThemeChanges(callback: () => void): () => void {
  if (typeof MutationObserver === 'undefined' || typeof document === 'undefined') {
    return () => {}
  }

  const observer = new MutationObserver(() => callback())
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['class', 'style'],
  })

  if (document.body) {
    observer.observe(document.body, {
      attributes: true,
      attributeFilter: ['class', 'style', 'theme-mode'],
    })
  }

  return () => observer.disconnect()
}
