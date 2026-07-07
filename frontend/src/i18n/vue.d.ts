import type { MessageKey } from './messages'

declare module '@vue/runtime-core' {
  interface ComponentCustomProperties {
    $t: (key: MessageKey, params?: Record<string, string | number>) => string
    $legacyT: (value: string) => string
  }
}
