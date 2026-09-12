import { describe, it, expect } from 'vitest'
import { sanitizeHtml, sanitizeText, sanitizeMarkdown } from '../sanitize'

describe('sanitize utils', () => {
  describe('sanitizeHtml', () => {
    it('should allow safe HTML tags', () => {
      const input = '<p>Hello <strong>World</strong></p>'
      const result = sanitizeHtml(input)
      expect(result).toContain('<p>')
      expect(result).toContain('<strong>')
    })

    it('should remove script tags', () => {
      const input = '<p>Hello</p><script>alert("xss")</script>'
      const result = sanitizeHtml(input)
      expect(result).not.toContain('<script>')
      expect(result).not.toContain('alert')
      expect(result).toContain('<p>')
    })

    it('should remove onclick handlers', () => {
      const input = '<p onclick="alert(1)">Click me</p>'
      const result = sanitizeHtml(input)
      expect(result).not.toContain('onclick')
      expect(result).not.toContain('alert')
    })

    it('should remove javascript: URLs', () => {
      const input = '<a href="javascript:alert(1)">Link</a>'
      const result = sanitizeHtml(input)
      expect(result).not.toContain('javascript:')
    })

    it('should allow safe links', () => {
      const input = '<a href="https://example.com">Link</a>'
      const result = sanitizeHtml(input)
      expect(result).toContain('href')
      expect(result).toContain('https://example.com')
      expect(result).toContain('referrerpolicy="no-referrer"')
    })

    it('should isolate links that open a new browsing context', () => {
      const result = sanitizeHtml('<a href="https://example.com" target="_blank">Link</a>')
      expect(result).toContain('target="_blank"')
      expect(result).toContain('rel="noopener noreferrer"')
    })

    it('should remove attacker-controlled named browsing contexts', () => {
      const result = sanitizeHtml('<a href="https://example.com" target="admin-window">Link</a>')
      expect(result).not.toContain('target=')
    })

    it('should allow code blocks', () => {
      const input = '<pre><code>const x = 1;</code></pre>'
      const result = sanitizeHtml(input)
      expect(result).toContain('<pre>')
      expect(result).toContain('<code>')
    })

    it('should remove dangerous attributes', () => {
      const input = '<div onerror="alert(1)" style="background:url(javascript:alert(1))">Text</div>'
      const result = sanitizeHtml(input)
      expect(result).not.toContain('onerror')
      expect(result).not.toContain('style')
    })
  })

  describe('sanitizeText', () => {
    it('should remove all HTML tags', () => {
      const input = '<p>Hello <strong>World</strong></p>'
      const result = sanitizeText(input)
      expect(result).not.toContain('<')
      expect(result).not.toContain('>')
      expect(result).toBe('Hello World')
    })

    it('should remove script tags and content', () => {
      const input = 'Safe text<script>alert("xss")</script>More text'
      const result = sanitizeText(input)
      expect(result).not.toContain('<script>')
      expect(result).not.toContain('alert')
    })
  })

  describe('sanitizeMarkdown', () => {
    it('should allow markdown-rendered HTML', () => {
      const input = '<h1>Title</h1><p>Paragraph with <em>emphasis</em></p>'
      const result = sanitizeMarkdown(input)
      expect(result).toContain('<h1>')
      expect(result).toContain('<em>')
    })

    it('should allow images', () => {
      const input = '<img src="https://example.com/image.png" alt="Test">'
      const result = sanitizeMarkdown(input)
      expect(result).toContain('<img')
      expect(result).toContain('src')
      expect(result).toContain('alt')
      expect(result).toContain('referrerpolicy="no-referrer"')
      expect(result).toContain('loading="lazy"')
    })

    it('should remove malicious image sources', () => {
      const input = '<img src="javascript:alert(1)" onerror="alert(1)">'
      const result = sanitizeMarkdown(input)
      expect(result).not.toContain('javascript:')
      expect(result).not.toContain('onerror')
    })

    it('should allow tables', () => {
      const input = '<table><thead><tr><th>Header</th></tr></thead><tbody><tr><td>Data</td></tr></tbody></table>'
      const result = sanitizeMarkdown(input)
      expect(result).toContain('<table>')
      expect(result).toContain('<thead>')
      expect(result).toContain('<th>')
    })

    it('should remove script tags in markdown', () => {
      const input = '<p>Safe content</p><script>alert("xss")</script>'
      const result = sanitizeMarkdown(input)
      expect(result).not.toContain('<script>')
      expect(result).toContain('<p>')
    })

    it('should preserve code blocks with syntax highlighting classes', () => {
      const input = '<pre><code class="language-javascript">const x = 1;</code></pre>'
      const result = sanitizeMarkdown(input)
      expect(result).toContain('<pre>')
      expect(result).toContain('<code')
      expect(result).toContain('class')
    })
  })

  describe('edge cases', () => {
    it('should handle empty strings', () => {
      expect(sanitizeHtml('')).toBe('')
      expect(sanitizeText('')).toBe('')
      expect(sanitizeMarkdown('')).toBe('')
    })

    it('should handle strings without HTML', () => {
      const plain = 'Just plain text'
      expect(sanitizeHtml(plain)).toBe(plain)
      expect(sanitizeText(plain)).toBe(plain)
      expect(sanitizeMarkdown(plain)).toBe(plain)
    })

    it('should handle nested XSS attempts', () => {
      const input = '<div><p onclick="alert(1)"><script>alert(2)</script></p></div>'
      const result = sanitizeHtml(input)
      expect(result).not.toContain('onclick')
      expect(result).not.toContain('<script>')
      expect(result).not.toContain('alert')
    })

    it('should handle encoded script attempts', () => {
      const input = '<img src=x onerror="&#97;&#108;&#101;&#114;&#116;&#40;&#39;&#88;&#83;&#83;&#39;&#41;">'
      const result = sanitizeHtml(input)
      expect(result).not.toContain('onerror')
    })
  })
})
