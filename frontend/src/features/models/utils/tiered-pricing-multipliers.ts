export function roundTierPrice(value: number): number {
  return Number.parseFloat(Math.max(0, value).toFixed(6))
}

export function cachePriceFromInputMultiplier(inputPrice: number, multiplier: number): number {
  const normalizedMultiplier = Number.isFinite(multiplier) && multiplier >= 0 ? multiplier : 0
  return roundTierPrice(inputPrice * normalizedMultiplier)
}

export function cacheMultiplierFromPrice(
  inputPrice: number,
  cachePrice: number | undefined,
  fallback: number,
): number {
  if (cachePrice == null || inputPrice <= 0) return fallback
  return roundTierPrice(cachePrice / inputPrice)
}
