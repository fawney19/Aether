import type { Ref } from 'vue'
import { nextTick, watch } from 'vue'
import { translateLegacyText, type Locale } from './messages'

const cjkPattern = /[\u4e00-\u9fff]/
const skippedTags = new Set(['SCRIPT', 'STYLE', 'CODE', 'PRE', 'KBD', 'SAMP', 'TEXTAREA'])
const translatableAttributes = ['alt', 'aria-label', 'placeholder', 'title']

const originalText = new WeakMap<Text, string>()
const originalAttributes = new WeakMap<Element, Map<string, string>>()

let observer: MutationObserver | null = null
let scheduled = false

function shouldSkipElement(element: Element | null): boolean {
  let current: Element | null = element
  while (current) {
    if (skippedTags.has(current.tagName) || current.hasAttribute('contenteditable')) {
      return true
    }
    current = current.parentElement
  }
  return false
}

function translateTextNode(node: Text, locale: Locale): void {
  if (shouldSkipElement(node.parentElement)) return

  if (locale !== 'en-US') {
    const original = originalText.get(node)
    if (original !== undefined && node.nodeValue !== original) {
      node.nodeValue = original
    }
    return
  }

  const current = node.nodeValue ?? ''
  const source = originalText.get(node) ?? current
  if (!cjkPattern.test(source)) return

  const translated = translateLegacyText(source, locale)
  if (!originalText.has(node)) {
    originalText.set(node, source)
  }
  if (translated !== current) {
    node.nodeValue = translated
  }
}

function translateElementAttributes(element: Element, locale: Locale): void {
  if (shouldSkipElement(element)) return

  let originals = originalAttributes.get(element)

  for (const attribute of translatableAttributes) {
    const current = element.getAttribute(attribute)
    if (current === null) continue

    if (locale !== 'en-US') {
      const original = originals?.get(attribute)
      if (original !== undefined && current !== original) {
        element.setAttribute(attribute, original)
      }
      continue
    }

    const source = originals?.get(attribute) ?? current
    if (!cjkPattern.test(source)) continue

    const translated = translateLegacyText(source, locale)
    if (!originals) {
      originals = new Map()
      originalAttributes.set(element, originals)
    }
    if (!originals.has(attribute)) {
      originals.set(attribute, source)
    }
    if (translated !== current) {
      element.setAttribute(attribute, translated)
    }
  }
}

function translateDom(root: ParentNode, locale: Locale): void {
  if (root instanceof Element) {
    translateElementAttributes(root, locale)
  }

  const elements = root.querySelectorAll?.('*') ?? []
  for (const element of elements) {
    translateElementAttributes(element, locale)
  }

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  let node = walker.nextNode()
  while (node) {
    translateTextNode(node as Text, locale)
    node = walker.nextNode()
  }
}

function scheduleDomTranslation(locale: Ref<Locale>): void {
  if (scheduled) return
  scheduled = true
  requestAnimationFrame(() => {
    scheduled = false
    if (document.body) {
      translateDom(document.body, locale.value)
    }
  })
}

export function installLegacyDomTranslator(locale: Ref<Locale>): void {
  if (typeof window === 'undefined' || typeof document === 'undefined') return
  if (observer) return

  void nextTick(() => scheduleDomTranslation(locale))

  watch(locale, () => {
    void nextTick(() => scheduleDomTranslation(locale))
  })

  observer = new MutationObserver(() => {
    scheduleDomTranslation(locale)
  })

  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: translatableAttributes,
    characterData: true,
    childList: true,
    subtree: true,
  })
}
