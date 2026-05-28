import DOMPurify from 'dompurify'

export const EXTERNAL_INTEGRATION_ICON_SVG_MAX_LENGTH = 8 * 1024

const ALLOWED_SVG_TAGS = [
  'svg',
  'g',
  'path',
  'circle',
  'rect',
  'line',
  'polyline',
  'polygon',
  'ellipse',
  'title',
  'desc',
]

const ALLOWED_SVG_ATTRS = [
  'aria-hidden',
  'aria-label',
  'class',
  'cx',
  'cy',
  'd',
  'fill',
  'fill-rule',
  'focusable',
  'height',
  'id',
  'points',
  'r',
  'role',
  'rx',
  'ry',
  'stroke',
  'stroke-linecap',
  'stroke-linejoin',
  'stroke-width',
  'transform',
  'viewBox',
  'width',
  'x',
  'x1',
  'x2',
  'xmlns',
  'y',
  'y1',
  'y2',
]

export function validateExternalIntegrationIconSvg(value?: string | null): string | null {
  const svg = value?.trim() ?? ''
  if (!svg) return null
  if (svg.length > EXTERNAL_INTEGRATION_ICON_SVG_MAX_LENGTH) {
    return 'SVG 图标不能超过 8KB'
  }
  if (!/^<svg[\s>]/i.test(svg)) {
    return 'SVG 图标必须以 <svg> 开始'
  }
  if (!/(<\/svg>|\/>)$/i.test(svg)) {
    return 'SVG 图标必须包含完整的 <svg> 根节点'
  }
  if (/<[!?]/.test(svg)) {
    return 'SVG 图标不能包含声明、DTD 或注释'
  }
  if (/(javascript:|data:|base64|url\()/i.test(svg)) {
    return 'SVG 图标不能包含脚本、Data URL 或外部引用'
  }
  if (/\son[a-z0-9_:-]+\s*=/i.test(svg)) {
    return 'SVG 图标不能包含事件属性'
  }
  if (/\s(?:href|xlink:href|src)\s*=/i.test(svg)) {
    return 'SVG 图标不能包含链接或图片引用'
  }
  if (/\sstyle\s*=/i.test(svg)) {
    return 'SVG 图标不能包含 style 属性'
  }

  const tags = Array.from(svg.matchAll(/<\s*\/?\s*([a-z][a-z0-9:-]*)/gi))
  if (tags.length === 0) {
    return 'SVG 图标格式不正确'
  }
  const allowed = new Set(ALLOWED_SVG_TAGS)
  for (const match of tags) {
    const tagName = match[1]?.toLowerCase()
    if (!tagName || !allowed.has(tagName)) {
      return 'SVG 图标只允许基础图形标签'
    }
  }

  return null
}

export function sanitizeExternalIntegrationIconSvg(value?: string | null): string {
  const svg = value?.trim() ?? ''
  if (!svg || validateExternalIntegrationIconSvg(svg)) return ''

  const sanitized = DOMPurify.sanitize(svg, {
    USE_PROFILES: { svg: true },
    ALLOWED_TAGS: ALLOWED_SVG_TAGS,
    ALLOWED_ATTR: ALLOWED_SVG_ATTRS,
    FORBID_TAGS: [
      'a',
      'animate',
      'embed',
      'foreignObject',
      'iframe',
      'image',
      'object',
      'script',
      'set',
      'style',
      'use',
    ],
    FORBID_ATTR: ['href', 'src', 'style', 'xlink:href'],
    ALLOW_DATA_ATTR: false,
  }).trim()

  return validateExternalIntegrationIconSvg(sanitized) ? '' : sanitized
}
