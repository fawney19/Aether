export const REDACTED_SECRET_PLACEHOLDER = '***'

export type EndpointSecretMarker = 'has_value' | 'has_pattern' | 'has_replacement'

export function retainsEndpointSecret(
  value: unknown,
  marker: unknown,
): boolean {
  return marker === true && value === REDACTED_SECRET_PLACEHOLDER
}

export function endpointSecretMarkerPayload(
  marker: EndpointSecretMarker,
  retained: boolean,
  value: unknown,
): Partial<Record<EndpointSecretMarker, true>> {
  return retained && value === REDACTED_SECRET_PLACEHOLDER
    ? { [marker]: true }
    : {}
}
