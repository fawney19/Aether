const DEVICE_ID_KEY = 'aether_client_device_id'

function generateDeviceId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID()
  }
  return `device-${Math.random().toString(36).slice(2, 10)}-${Date.now()}`
}

export function getClientDeviceId(): string {
  const existing = localStorage.getItem(DEVICE_ID_KEY)
  if (existing) {
    return existing
  }

  const created = generateDeviceId()
  localStorage.setItem(DEVICE_ID_KEY, created)
  return created
}
