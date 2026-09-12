import type { Ref } from 'vue'
import { nextTick, watch } from 'vue'
import { translateLegacyText, type Locale } from './messages'

const cjkPattern = /[\u4e00-\u9fff]/
const skippedTags = new Set(['SCRIPT', 'STYLE', 'CODE', 'PRE', 'KBD', 'SAMP', 'TEXTAREA'])
const translatableAttributes = ['alt', 'aria-label', 'placeholder', 'title']
const skipAttributes = ['translate', 'data-i18n-skip', 'contenteditable', 'v-pre']

interface TranslationState {
  source: string
  rendered: string
}

const originalText = new WeakMap<Text, TranslationState>()
const originalAttributes = new WeakMap<Element, Map<string, TranslationState>>()
let stopActiveTranslator: (() => void) | null = null

function shouldSkipElement(element: Element | null): boolean {
  let current = element
  while (current) {
    if (skippedTags.has(current.tagName) || current.hasAttribute('contenteditable') || current.hasAttribute('v-pre') || current.hasAttribute('data-i18n-skip') || current.getAttribute('translate')?.toLowerCase() === 'no') {
      return true
    }
    current = current.parentElement
  }
  return false
}

function translateValue(current: string, previous: TranslationState | undefined, locale: Locale): TranslationState {
  // A renderer may reuse the same node for a new status or record. Only reuse
  // the cached source while the DOM still contains our most recent output.
  const source = previous && current === previous.rendered ? previous.source : current
  return {
    source,
    rendered: cjkPattern.test(source) ? translateLegacyText(source, locale) : source,
  }
}

function translateTextNode(node: Text, locale: Locale): void {
  const current = node.nodeValue ?? ''
  const previous = originalText.get(node)
  if (shouldSkipElement(node.parentElement)) {
    if (previous && current === previous.rendered) node.nodeValue = previous.source
    originalText.delete(node)
    return
  }

  const state = translateValue(current, previous, locale)
  originalText.set(node, state)
  if (state.rendered !== current) node.nodeValue = state.rendered
}

function translateElementAttributes(element: Element, locale: Locale): void {
  let originals = originalAttributes.get(element)
  const skipped = shouldSkipElement(element)

  for (const attribute of translatableAttributes) {
    const current = element.getAttribute(attribute)
    const previous = originals?.get(attribute)
    if (current === null || skipped) {
      if (skipped && previous && current === previous.rendered) element.setAttribute(attribute, previous.source)
      originals?.delete(attribute)
      continue
    }

    const state = translateValue(current, previous, locale)
    if (!originals) {
      originals = new Map()
      originalAttributes.set(element, originals)
    }
    originals.set(attribute, state)
    if (state.rendered !== current) element.setAttribute(attribute, state.rendered)
  }
}

function translateDom(root: ParentNode, locale: Locale): void {
  if (root instanceof Element) translateElementAttributes(root, locale)
  for (const element of root.querySelectorAll('*')) translateElementAttributes(element, locale)

  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  let node = walker.nextNode()
  while (node) {
    translateTextNode(node as Text, locale)
    node = walker.nextNode()
  }
}

export function installLegacyDomTranslator(locale: Ref<Locale>): () => void {
  if (typeof window === 'undefined' || typeof document === 'undefined') return () => {}
  if (stopActiveTranslator) return stopActiveTranslator

  let frame: number | null = null
  let stopped = false
  const schedule = (): void => {
    if (stopped || frame !== null) return
    frame = requestAnimationFrame(() => {
      frame = null
      if (!stopped && document.body) translateDom(document.body, locale.value)
    })
  }

  void nextTick(schedule)
  const stopWatching = watch(locale, () => { void nextTick(schedule) })
  const observer = new MutationObserver(schedule)
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: [...translatableAttributes, ...skipAttributes],
    characterData: true,
    childList: true,
    subtree: true,
  })

  stopActiveTranslator = () => {
    if (stopped) return
    stopped = true
    observer.disconnect()
    stopWatching()
    if (frame !== null) cancelAnimationFrame(frame)
    stopActiveTranslator = null
  }
  return stopActiveTranslator
}
