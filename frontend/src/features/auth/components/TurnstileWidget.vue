<template>
  <div class="space-y-2">
    <div
      ref="containerRef"
      class="min-h-[65px]"
      :class="disabled ? 'pointer-events-none opacity-60' : ''"
    />
    <p
      v-if="errorMessage"
      class="text-xs text-destructive"
    >
      {{ errorMessage }}
    </p>
  </div>
</template>

<script setup lang="ts">
import { nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'

const props = withDefaults(defineProps<{
  modelValue?: string
  siteKey: string
  action?: string
  disabled?: boolean
}>(), {
  modelValue: '',
  action: undefined,
  disabled: false,
})

const emit = defineEmits<{
  'update:modelValue': [value: string]
  error: [message: string]
}>()

const TURNSTILE_SCRIPT_URL = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit'

type TurnstileWidgetId = string

interface TurnstileRenderOptions {
  sitekey: string
  action?: string
  execution?: 'render' | 'execute'
  appearance?: 'always' | 'execute' | 'interaction-only'
  callback?: (token: string) => void
  'error-callback'?: () => void
  'expired-callback'?: () => void
  'timeout-callback'?: () => void
}

interface TurnstileApi {
  render: (container: HTMLElement, options: TurnstileRenderOptions) => TurnstileWidgetId
  execute?: (widgetId: TurnstileWidgetId) => void
  reset: (widgetId: TurnstileWidgetId) => void
  remove?: (widgetId: TurnstileWidgetId) => void
}

declare global {
  interface Window {
    turnstile?: TurnstileApi
    __aetherTurnstileScriptPromise?: Promise<void>
  }
}

const containerRef = ref<HTMLElement | null>(null)
const widgetId = ref<TurnstileWidgetId | null>(null)
const errorMessage = ref('')
let pendingReject: ((error: Error) => void) | null = null

function loadTurnstileScript(): Promise<void> {
  if (window.turnstile) {
    return Promise.resolve()
  }
  if (window.__aetherTurnstileScriptPromise) {
    return window.__aetherTurnstileScriptPromise
  }

  window.__aetherTurnstileScriptPromise = new Promise((resolve, reject) => {
    const rejectAndReset = (script: HTMLScriptElement) => {
      script.remove()
      delete window.__aetherTurnstileScriptPromise
      reject(new Error('Turnstile script failed'))
    }
    const existing = document.querySelector<HTMLScriptElement>(
      'script[data-aether-turnstile="true"]'
    )
    if (existing) {
      existing.addEventListener('load', () => resolve(), { once: true })
      existing.addEventListener('error', () => rejectAndReset(existing), {
        once: true,
      })
      return
    }

    const script = document.createElement('script')
    script.src = TURNSTILE_SCRIPT_URL
    script.async = true
    script.defer = true
    script.dataset.aetherTurnstile = 'true'
    script.onload = () => resolve()
    script.onerror = () => rejectAndReset(script)
    document.head.appendChild(script)
  })

  return window.__aetherTurnstileScriptPromise
}

function clearWidget() {
  if (widgetId.value && window.turnstile) {
    if (window.turnstile.remove) {
      window.turnstile.remove(widgetId.value)
    } else {
      window.turnstile.reset(widgetId.value)
    }
  }
  widgetId.value = null
  emit('update:modelValue', '')
}

async function renderWidget() {
  if (!props.siteKey || !containerRef.value) return
  clearWidget()
  errorMessage.value = ''
  try {
    await loadTurnstileScript()
    await nextTick()
    if (!window.turnstile || !containerRef.value) return
    widgetId.value = window.turnstile.render(containerRef.value, {
      sitekey: props.siteKey,
      action: props.action,
      callback: (token: string) => {
        errorMessage.value = ''
        emit('update:modelValue', token)
      },
      'expired-callback': () => {
        emit('update:modelValue', '')
      },
      'error-callback': () => {
        const message = '人机验证加载失败，请重试'
        errorMessage.value = message
        emit('update:modelValue', '')
        emit('error', message)
      },
      'timeout-callback': () => {
        const message = '人机验证超时，请重试'
        errorMessage.value = message
        emit('update:modelValue', '')
        emit('error', message)
      },
    })
  } catch {
    const message = '人机验证加载失败，请重试'
    errorMessage.value = message
    emit('update:modelValue', '')
    emit('error', message)
  }
}

async function execute(action: string): Promise<string> {
  await loadTurnstileScript()
  const turnstile = window.turnstile
  const container = containerRef.value
  if (!turnstile || !container || !turnstile.execute) {
    throw new Error('Turnstile unavailable')
  }
  const execute = turnstile.execute.bind(turnstile)

  clearWidget()

  return new Promise((resolve, reject) => {
    pendingReject = reject
    const id = turnstile.render(container, {
      sitekey: props.siteKey,
      action,
      execution: 'execute',
      appearance: 'interaction-only',
      callback: (token: string) => {
        pendingReject = null
        resolve(token)
      },
      'error-callback': () => {
        pendingReject = null
        reject(new Error('Turnstile challenge failed'))
      },
      'expired-callback': () => {
        pendingReject = null
        reject(new Error('Turnstile token expired'))
      },
      'timeout-callback': () => {
        pendingReject = null
        reject(new Error('Turnstile challenge timed out'))
      },
    })
    widgetId.value = id
    execute(id)
  })
}

function reset() {
  if (pendingReject) {
    pendingReject(new Error('Turnstile reset'))
    pendingReject = null
  }
  emit('update:modelValue', '')
  errorMessage.value = ''
  if (widgetId.value && window.turnstile) {
    window.turnstile.reset(widgetId.value)
    return
  }
  void renderWidget()
}

onMounted(() => {
  void renderWidget()
})

onBeforeUnmount(() => {
  if (pendingReject) {
    pendingReject(new Error('Turnstile reset'))
    pendingReject = null
  }
  if (widgetId.value && window.turnstile) {
    if (window.turnstile.remove) {
      window.turnstile.remove(widgetId.value)
    } else {
      window.turnstile.reset(widgetId.value)
    }
  }
})

watch([() => props.siteKey, () => props.action], () => {
  void renderWidget()
})

defineExpose({ execute, reset })
</script>
