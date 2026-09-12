<template>
  <div class="mx-auto flex min-h-[calc(100vh-9rem)] w-full max-w-[1800px] flex-col gap-4 pb-2">
    <header class="flex flex-col gap-3 border-b border-border/60 pb-4 sm:flex-row sm:items-center sm:justify-between">
      <div class="min-w-0">
        <h1 class="flex items-center gap-2 text-lg font-semibold text-foreground">
          <SquareTerminal class="h-5 w-5 text-primary" />
          {{ t('vscodex.title') }}
        </h1>
      </div>

      <div class="flex min-w-0 items-center gap-2">
        <label
          v-if="devices.length > 0"
          class="sr-only"
          for="vscodex-device-select"
        >{{ t('vscodex.devices.label') }}</label>
        <select
          v-if="devices.length > 0"
          id="vscodex-device-select"
          v-model="selectedDeviceId"
          data-testid="vscodex-device-select"
          class="h-9 min-w-0 max-w-64 rounded-md border border-border/70 bg-background px-3 text-sm text-foreground outline-none transition-colors focus:border-primary focus:ring-2 focus:ring-primary/20"
        >
          <option
            v-for="device in devices"
            :key="device.id"
            :value="device.id"
          >
            {{ device.name }} · {{ statusLabel(device.status) }}
          </option>
        </select>
        <button
          v-if="selectedDevice"
          type="button"
          data-testid="vscodex-revoke-device"
          class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-destructive disabled:pointer-events-none disabled:opacity-50"
          :disabled="revokingDevice"
          :aria-label="t('vscodex.devices.revoke')"
          :title="t('vscodex.devices.revoke')"
          @click="revokeSelectedDevice"
        >
          <Loader2
            v-if="revokingDevice"
            class="h-4 w-4 animate-spin"
          />
          <Trash2
            v-else
            class="h-4 w-4"
          />
        </button>
        <button
          type="button"
          data-testid="vscodex-refresh"
          class="flex h-9 w-9 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary disabled:pointer-events-none disabled:opacity-50"
          :disabled="loadingDevices"
          :aria-label="t('vscodex.devices.refresh')"
          :title="t('vscodex.devices.refresh')"
          @click="refreshDevices()"
        >
          <RefreshCcw
            class="h-4 w-4"
            :class="{ 'animate-spin': loadingDevices }"
          />
        </button>
      </div>
    </header>

    <div
      v-if="loadError"
      class="flex flex-wrap items-center justify-between gap-3 rounded-md border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive"
      role="alert"
    >
      <span class="flex items-center gap-2">
        <AlertCircle class="h-4 w-4 shrink-0" />
        {{ t('vscodex.devices.loadFailed') }}
      </span>
      <Button
        variant="outline"
        size="sm"
        @click="refreshDevices()"
      >
        {{ t('vscodex.connection.retry') }}
      </Button>
    </div>

    <div
      v-if="revokeError"
      class="flex items-center gap-2 rounded-md border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive"
      role="alert"
    >
      <AlertCircle class="h-4 w-4 shrink-0" />
      {{ t('vscodex.devices.revokeFailed') }}
    </div>

    <LoadingState
      v-if="loadingDevices && devices.length === 0 && !pairing"
      class="flex-1"
      :message="t('vscodex.devices.loading')"
      full-height
    />

    <section
      v-else-if="!selectedDevice"
      data-testid="vscodex-pairing-state"
      class="flex flex-1 items-center justify-center py-8"
    >
      <div class="w-full max-w-xl rounded-lg border border-dashed border-border bg-card/40 px-5 py-8 text-center sm:px-8">
        <div class="mx-auto flex h-11 w-11 items-center justify-center rounded-full bg-muted text-muted-foreground">
          <Link2 class="h-5 w-5" />
        </div>
        <h2 class="mt-4 text-base font-semibold text-foreground">
          {{ pairing ? t('vscodex.pairing.title') : t('vscodex.devices.emptyTitle') }}
        </h2>
        <p class="mx-auto mt-2 max-w-md text-sm leading-6 text-muted-foreground">
          {{ pairing ? t('vscodex.pairing.description') : t('vscodex.devices.emptyDescription') }}
        </p>

        <div
          v-if="pairing"
          class="mt-6"
        >
          <div class="text-xs font-medium uppercase text-muted-foreground">
            {{ t('vscodex.pairing.code') }}
          </div>
          <div class="mt-2 flex items-center justify-center gap-2">
            <code class="select-all rounded-md border border-border bg-background px-4 py-2 font-mono text-xl font-semibold text-foreground">
              {{ pairing.code }}
            </code>
            <button
              type="button"
              class="flex h-10 w-10 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary"
              :aria-label="codeCopied ? t('vscodex.pairing.copied') : t('vscodex.pairing.copy')"
              :title="codeCopied ? t('vscodex.pairing.copied') : t('vscodex.pairing.copy')"
              @click="copyPairingCode"
            >
              <Check
                v-if="codeCopied"
                class="h-4 w-4 text-emerald-600"
              />
              <Copy
                v-else
                class="h-4 w-4"
              />
            </button>
          </div>
          <p
            v-if="pairingExpiryLabel"
            class="mt-3 text-xs text-muted-foreground"
          >
            {{ t('vscodex.pairing.expires', { time: pairingExpiryLabel }) }}
          </p>
          <div class="mt-5 flex flex-wrap items-center justify-center gap-2">
            <Button
              variant="outline"
              size="sm"
              :disabled="creatingPairing"
              @click="createPairing"
            >
              <RefreshCcw class="mr-2 h-4 w-4" />
              {{ t('vscodex.pairing.newCode') }}
            </Button>
            <Button
              size="sm"
              :disabled="loadingDevices"
              @click="refreshDevices()"
            >
              {{ t('vscodex.devices.checkConnection') }}
            </Button>
          </div>
        </div>

        <Button
          v-else
          data-testid="vscodex-create-pairing"
          class="mt-6"
          :disabled="creatingPairing"
          @click="createPairing"
        >
          <Loader2
            v-if="creatingPairing"
            class="mr-2 h-4 w-4 animate-spin"
          />
          <Link2
            v-else
            class="mr-2 h-4 w-4"
          />
          {{ creatingPairing ? t('vscodex.pairing.creating') : t('vscodex.pairing.create') }}
        </Button>

        <p
          v-if="pairingError"
          class="mt-4 text-sm text-destructive"
          role="alert"
        >
          {{ t('vscodex.pairing.failed') }}
        </p>
      </div>
    </section>

    <section
      v-else
      class="flex min-h-[520px] flex-1 flex-col gap-3"
    >
      <div class="flex min-h-6 flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
        <span class="flex items-center gap-2">
          <span
            class="h-2 w-2 rounded-full"
            :class="statusDotClass(selectedDevice.status)"
          />
          {{ selectedDevice.name }} · {{ statusLabel(selectedDevice.status) }}
        </span>
        <span
          v-if="ticketLoading"
          class="flex items-center gap-1.5"
        >
          <Loader2 class="h-3.5 w-3.5 animate-spin" />
          {{ t('vscodex.connection.connecting') }}
        </span>
      </div>

      <div
        v-if="connectionError"
        class="flex flex-wrap items-center justify-between gap-3 rounded-md border border-destructive/30 bg-destructive/5 px-4 py-2.5 text-sm text-destructive"
        role="alert"
      >
        <span>{{ t('vscodex.connection.ticketFailed') }}</span>
        <Button
          variant="outline"
          size="sm"
          :disabled="ticketLoading || !frameReady"
          @click="requestTicket"
        >
          {{ t('vscodex.connection.retry') }}
        </Button>
      </div>

      <div class="relative min-h-[480px] flex-1 overflow-hidden rounded-lg border border-border bg-background">
        <iframe
          :key="frameKey"
          ref="frameRef"
          data-testid="vscodex-frame"
          class="h-full min-h-[480px] w-full border-0 bg-background"
          :src="childFrameUrl"
          :title="t('vscodex.frame.title')"
          sandbox="allow-scripts allow-same-origin allow-forms allow-downloads"
          allow="clipboard-read; clipboard-write"
          @load="frameLoaded = true"
        />
        <div
          v-if="!frameLoaded"
          class="pointer-events-none absolute inset-0 flex items-center justify-center bg-background"
        >
          <div class="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 class="h-4 w-4 animate-spin" />
            {{ t('vscodex.connection.loading') }}
          </div>
        </div>
      </div>
    </section>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  AlertCircle,
  Check,
  Copy,
  Link2,
  Loader2,
  RefreshCcw,
  SquareTerminal,
  Trash2,
} from 'lucide-vue-next'
import { vscodexApi, type VscodexDevice, type VscodexDeviceStatus, type VscodexPairing } from '@/api/vscodex'
import { LoadingState } from '@/components/common'
import { Button } from '@/components/ui'
import { useDarkMode } from '@/composables/useDarkMode'
import { useI18n } from '@/i18n'

const PROTOCOL_VERSION = 1
const PAIRING_POLL_INTERVAL_MS = 4_000
const childFrameUrl = `${import.meta.env.BASE_URL}aether-vscodex/index.html?embed=aether`

const { locale, t } = useI18n()
const { isDark } = useDarkMode()

const devices = ref<VscodexDevice[]>([])
const selectedDeviceId = ref('')
const loadingDevices = ref(true)
const loadError = ref(false)
const pairing = ref<VscodexPairing | null>(null)
const pairingCreatedAt = ref(0)
const creatingPairing = ref(false)
const pairingError = ref(false)
const codeCopied = ref(false)
const frameRef = ref<HTMLIFrameElement | null>(null)
const frameKey = ref(0)
const frameLoaded = ref(false)
const frameReady = ref(false)
const ticketLoading = ref(false)
const connectionError = ref(false)
const revokingDevice = ref(false)
const revokeError = ref(false)

let disposed = false
let deviceRequestVersion = 0
let connectionVersion = 0
let ticketRequest: Promise<void> | null = null
let pairingPollTimer: ReturnType<typeof setInterval> | null = null

const selectedDevice = computed(() => (
  devices.value.find(device => device.id === selectedDeviceId.value) ?? null
))

const pairingExpiryLabel = computed(() => {
  if (!pairing.value) return ''
  if (pairing.value.expires_at) {
    const expiry = new Date(pairing.value.expires_at)
    if (!Number.isNaN(expiry.getTime())) return expiry.toLocaleString(locale.value)
  }
  if (pairing.value.expires_in_seconds) {
    const expiry = new Date(pairingCreatedAt.value + pairing.value.expires_in_seconds * 1_000)
    return expiry.toLocaleString(locale.value)
  }
  return ''
})

function statusLabel(status: VscodexDeviceStatus): string {
  return t(`vscodex.devices.${status}`)
}

function statusDotClass(status: VscodexDeviceStatus): string {
  if (status === 'online') return 'bg-emerald-500'
  if (status === 'connecting') return 'bg-amber-500'
  return 'bg-muted-foreground/50'
}

function stopPairingPoll(): void {
  if (pairingPollTimer) {
    clearInterval(pairingPollTimer)
    pairingPollTimer = null
  }
}

function startPairingPoll(): void {
  stopPairingPoll()
  pairingPollTimer = setInterval(() => {
    if (!loadingDevices.value && !disposed) void refreshDevices({ silent: true })
  }, PAIRING_POLL_INTERVAL_MS)
}

function invalidateConnection(reloadFrame = true): void {
  postToFrame({ type: 'aether-vscodex/disconnect' })
  connectionVersion += 1
  ticketRequest = null
  ticketLoading.value = false
  connectionError.value = false
  frameReady.value = false
  frameLoaded.value = false
  if (reloadFrame) frameKey.value += 1
}

async function refreshDevices(options: { silent?: boolean } = {}): Promise<void> {
  const requestVersion = ++deviceRequestVersion
  if (!options.silent) {
    loadingDevices.value = true
    loadError.value = false
  }

  try {
    const nextDevices = await vscodexApi.listDevices()
    if (disposed || requestVersion !== deviceRequestVersion) return

    devices.value = nextDevices
    const currentStillExists = nextDevices.some(device => device.id === selectedDeviceId.value)
    if (!currentStillExists) {
      selectedDeviceId.value = nextDevices.find(device => device.status === 'online')?.id
        ?? nextDevices[0]?.id
        ?? ''
    }

    if (nextDevices.length > 0) {
      pairing.value = null
      pairingError.value = false
      stopPairingPoll()
    }
  } catch {
    if (!disposed && requestVersion === deviceRequestVersion && !options.silent) {
      loadError.value = true
    }
  } finally {
    if (!disposed && requestVersion === deviceRequestVersion && !options.silent) {
      loadingDevices.value = false
    }
  }
}

async function createPairing(): Promise<void> {
  if (creatingPairing.value) return
  creatingPairing.value = true
  pairingError.value = false
  codeCopied.value = false

  try {
    pairing.value = await vscodexApi.createPairing()
    pairingCreatedAt.value = Date.now()
    startPairingPoll()
  } catch {
    pairingError.value = true
  } finally {
    creatingPairing.value = false
  }
}

async function revokeSelectedDevice(): Promise<void> {
  const device = selectedDevice.value
  if (!device || revokingDevice.value) return
  if (!window.confirm(t('vscodex.devices.revokeConfirm', { name: device.name }))) return

  revokingDevice.value = true
  revokeError.value = false
  try {
    await vscodexApi.deleteDevice(device.id)
    invalidateConnection()
    devices.value = devices.value.filter(item => item.id !== device.id)
    selectedDeviceId.value = devices.value.find(item => item.status === 'online')?.id
      ?? devices.value[0]?.id
      ?? ''
    await refreshDevices({ silent: true })
  } catch {
    revokeError.value = true
  } finally {
    revokingDevice.value = false
  }
}

async function copyPairingCode(): Promise<void> {
  if (!pairing.value || !navigator.clipboard) return
  try {
    await navigator.clipboard.writeText(pairing.value.code)
    codeCopied.value = true
    window.setTimeout(() => {
      codeCopied.value = false
    }, 1_500)
  } catch {
    codeCopied.value = false
  }
}

function postToFrame(message: Record<string, unknown>, target = frameRef.value?.contentWindow): void {
  if (!target) return
  target.postMessage({ v: PROTOCOL_VERSION, ...message }, window.location.origin)
}

function postContext(): void {
  postToFrame({
    type: 'aether-vscodex/context',
    locale: locale.value,
    theme: isDark.value ? 'dark' : 'light',
  })
}

async function requestTicket(): Promise<void> {
  if (!selectedDevice.value || !frameReady.value) return
  if (ticketRequest) return ticketRequest

  const requestVersion = connectionVersion
  const deviceId = selectedDevice.value.id
  const target = frameRef.value?.contentWindow
  if (!target) return

  ticketLoading.value = true
  connectionError.value = false
  const request = (async () => {
    try {
      const result = await vscodexApi.createWsTicket(deviceId)
      if (
        disposed
        || requestVersion !== connectionVersion
        || selectedDeviceId.value !== deviceId
        || frameRef.value?.contentWindow !== target
      ) return

      postToFrame({
        type: 'aether-vscodex/connect',
        ticket: result.ticket,
        wsUrl: result.ws_url,
        deviceId,
        locale: locale.value,
        theme: isDark.value ? 'dark' : 'light',
      }, target)
    } catch {
      if (disposed || requestVersion !== connectionVersion) return
      connectionError.value = true
      postToFrame({
        type: 'aether-vscodex/error',
        code: 'ticket_unavailable',
      }, target)
    } finally {
      if (!disposed && requestVersion === connectionVersion) ticketLoading.value = false
    }
  })()

  ticketRequest = request
  try {
    await request
  } finally {
    if (ticketRequest === request) ticketRequest = null
  }
}

function isFrameMessage(value: unknown): value is { v: number; type: string } {
  return typeof value === 'object'
    && value !== null
    && (value as Record<string, unknown>).v === PROTOCOL_VERSION
    && typeof (value as Record<string, unknown>).type === 'string'
}

function handleFrameMessage(event: MessageEvent): void {
  const target = frameRef.value?.contentWindow
  if (!target || event.origin !== window.location.origin || event.source !== target) return
  if (!isFrameMessage(event.data)) return

  if (event.data.type === 'aether-vscodex/ready') {
    frameReady.value = true
    postContext()
    void requestTicket()
  } else if (event.data.type === 'aether-vscodex/request-ticket') {
    frameReady.value = true
    void requestTicket()
  }
}

watch(selectedDeviceId, async (next, previous) => {
  if (next === previous) return
  invalidateConnection()
  await nextTick()
})

watch([locale, isDark], () => {
  if (frameReady.value) postContext()
})

onMounted(() => {
  window.addEventListener('message', handleFrameMessage)
  void refreshDevices()
})

onBeforeUnmount(() => {
  postToFrame({ type: 'aether-vscodex/disconnect' })
  disposed = true
  connectionVersion += 1
  deviceRequestVersion += 1
  stopPairingPoll()
  window.removeEventListener('message', handleFrameMessage)
})

</script>
