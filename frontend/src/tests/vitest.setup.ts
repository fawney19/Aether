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
