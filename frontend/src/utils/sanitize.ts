import DOMPurify from 'dompurify'

DOMPurify.addHook('afterSanitizeAttributes', (node) => {
  const tagName = node.tagName?.toLowerCase()

  if (tagName === 'a') {
    const target = node.getAttribute('target')
    if (target && target !== '_blank') {
      node.removeAttribute('target')
    }
    if (target === '_blank') {
      node.setAttribute('rel', 'noopener noreferrer')
    }

    const href = node.getAttribute('href') || ''
    if (/^(?:https?:)?\/\//i.test(href)) {
      node.setAttribute('referrerpolicy', 'no-referrer')
    }
  }

  if (tagName === 'img') {
    node.setAttribute('referrerpolicy', 'no-referrer')
    node.setAttribute('loading', 'lazy')
    node.setAttribute('decoding', 'async')
  }
})

/**
 * 配置 DOMPurify 允许的标签和属性
 */
const DOMPURIFY_CONFIG = {
  // 允许的HTML标签
  ALLOWED_TAGS: [
    'p', 'br', 'strong', 'em', 'u', 's', 'code', 'pre',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6',
    'ul', 'ol', 'li',
    'a', 'blockquote',
    'table', 'thead', 'tbody', 'tr', 'th', 'td',
    'span', 'div'
  ],
  // 允许的属性
  ALLOWED_ATTR: [
    'href', 'title', 'target', 'rel',
    'class', 'id', 'referrerpolicy'
  ],
  // 允许的URI协议
  // eslint-disable-next-line no-useless-escape
  ALLOWED_URI_REGEXP: /^(?:(?:(?:f|ht)tps?|mailto|tel|callto|sms|cid|xmpp):|[^a-z]|[a-z+.\-]+(?:[^a-z+.\-:]|$))/i
}

/**
 * 清理HTML内容，移除潜在的XSS攻击向量
 * @param dirty - 原始HTML字符串
 * @returns 清理后的安全HTML字符串
 */
export function sanitizeHtml(dirty: string): string {
  return DOMPurify.sanitize(dirty, DOMPURIFY_CONFIG)
}

/**
 * 严格模式：只允许纯文本
 * @param dirty - 原始字符串
 * @returns 纯文本（所有HTML标签被移除）
 */
export function sanitizeText(dirty: string): string {
  return DOMPurify.sanitize(dirty, { ALLOWED_TAGS: [] })
}

/**
 * 为 Markdown 内容提供特殊配置
 * @param dirty - Markdown 渲染后的 HTML
 * @returns 清理后的安全 HTML
 */
export function sanitizeMarkdown(dirty: string): string {
  // Markdown 可能需要更多的HTML标签支持
  const markdownConfig = {
    ...DOMPURIFY_CONFIG,
    ALLOWED_TAGS: [
      ...DOMPURIFY_CONFIG.ALLOWED_TAGS,
      'img' // Markdown 支持图片
    ],
    ALLOWED_ATTR: [
      ...DOMPURIFY_CONFIG.ALLOWED_ATTR,
      'src', 'alt', 'width', 'height', 'loading', 'decoding' // 图片属性
    ]
  }

  return DOMPurify.sanitize(dirty, markdownConfig)
}
