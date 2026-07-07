import { beforeEach } from 'vitest'

function createMemoryStorage(): Storage {
  const store = new Map<string, string>()

  return {
    get length() {
      return store.size
    },
    clear() {
      store.clear()
    },
    getItem(key: string) {
      return store.get(String(key)) ?? null
    },
    key(index: number) {
      return Array.from(store.keys())[index] ?? null
    },
    removeItem(key: string) {
      store.delete(String(key))
    },
    setItem(key: string, value: string) {
      store.set(String(key), String(value))
    },
  }
}

function installStorage(name: 'localStorage' | 'sessionStorage') {
  const storage = createMemoryStorage()

  Object.defineProperty(globalThis, name, {
    value: storage,
    configurable: true,
  })

  if (typeof window !== 'undefined') {
    Object.defineProperty(window, name, {
      value: storage,
      configurable: true,
    })
  }
}

installStorage('localStorage')
installStorage('sessionStorage')

if (typeof globalThis.requestAnimationFrame !== 'function') {
  Object.defineProperty(globalThis, 'requestAnimationFrame', {
    value: (callback: FrameRequestCallback) => window.setTimeout(() => callback(Date.now()), 16),
    configurable: true,
  })
}

if (typeof globalThis.cancelAnimationFrame !== 'function') {
  Object.defineProperty(globalThis, 'cancelAnimationFrame', {
    value: (handle: number) => window.clearTimeout(handle),
    configurable: true,
  })
}

beforeEach(async () => {
  localStorage.clear()
  sessionStorage.clear()
  const { setI18nLocale } = await import('@/i18n')
  setI18nLocale('zh-CN')
})
