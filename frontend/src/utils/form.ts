/**
 * Form utility functions
 */

/**
 * Parse number input value, handling empty strings and NaN
 * Use this for optional number fields that should be `undefined` when empty
 *
 * @param value - Input value (string or number)
 * @param options - Parse options
 * @returns Parsed number or undefined
 *
 * @example
 * // In template:
 * <Input
 *   :model-value="form.rate_limit ?? ''"
 *   @update:model-value="(v) => form.rate_limit = parseNumberInput(v)"
 * />
 */
export function parseNumberInput(
  value: string | number | null | undefined,
  options: {
    allowFloat?: boolean
    min?: number
    max?: number
  } = {}
): number | undefined {
  const { allowFloat = false, min, max } = options

  // Handle empty/null/undefined
  if (value === '' || value === null || value === undefined) {
    return undefined
  }

  // Parse the value
  const num = typeof value === 'string'
    ? (allowFloat ? parseFloat(value) : parseInt(value, 10))
    : value

  // Handle NaN
  if (isNaN(num)) {
    return undefined
  }

  // Apply min/max constraints
  let result = num
  if (min !== undefined && result < min) {
    result = min
  }
  if (max !== undefined && result > max) {
    result = max
  }

  return result
}

/**
 * Parse number input value for nullable fields (like rpm_limit)
 * Returns `null` when empty (to signal "use adaptive/default mode")
 * Returns `undefined` when not provided (to signal "keep original value")
 *
 * @param value - Input value (string or number)
 * @param options - Parse options
 * @returns Parsed number, null (for empty/adaptive), or undefined
 */
export function parseNullableNumberInput(
  value: string | number,
  options?: { allowFloat?: boolean; min?: number; max?: number }
): number | null
export function parseNullableNumberInput(
  value: string | number | null | undefined,
  options?: { allowFloat?: boolean; min?: number; max?: number }
): number | null | undefined
export function parseNullableNumberInput(
  value: string | number | null | undefined,
  options: {
    allowFloat?: boolean
    min?: number
    max?: number
  } = {}
): number | null | undefined {
  const { allowFloat = false, min, max } = options

  // Empty string means "null" (adaptive mode)
  if (value === '') {
    return null
  }

  // null/undefined means "keep original value"
  if (value === null || value === undefined) {
    return undefined
  }

  // Parse the value
  const num = typeof value === 'string'
    ? (allowFloat ? parseFloat(value) : parseInt(value, 10))
    : value

  // Handle NaN - treat as null (adaptive mode)
  if (isNaN(num)) {
    return null
  }

  // Apply min/max constraints
  let result = num
  if (min !== undefined && result < min) {
    result = min
  }
  if (max !== undefined && result > max) {
    result = max
  }

  return result
}

/**
 * Create a handler function for number input with specific field
 * Useful for creating inline handlers in templates
 *
 * @param obj - Reactive object containing the field
 * @param field - Field name to update
 * @param options - Parse options
 * @returns Handler function
 *
 * @example
 * // In script:
 * const handleRateLimit = createNumberInputHandler(form, 'rate_limit')
 *
 * // In template:
 * <Input @update:model-value="handleRateLimit" />
 */
export function createNumberInputHandler<T extends Record<string, unknown>>(
  obj: T,
  field: keyof T,
  options: Parameters<typeof parseNumberInput>[1] = {}
) {
  return (value: string | number | null | undefined) => {
    (obj as Record<string, unknown>)[field as string] = parseNumberInput(value, options)
  }
}

/**
 * 获取分辨率的排序权重（用于从低到高排序）
 * 支持的格式：
 * - NNNp 格式：480p, 720p, 1080p, 2160p
 * - 4k/8k 格式：4k -> 2160, 8k -> 4320
 * - WxH 格式：720x1080 -> 按像素总数排序
 *
 * @param resolution - 分辨率字符串
 * @returns 排序权重（数字越大分辨率越高）
 */
export function getResolutionSortWeight(resolution: string): number {
  const normalized = (resolution || '').trim().toLowerCase()

  // 4k/8k 格式
  if (normalized === '4k') return 2160 * 2160
  if (normalized === '8k') return 4320 * 4320

  // NNNp 格式（如 480p, 720p, 1080p）
  const pMatch = normalized.match(/^(\d+)p$/)
  if (pMatch) {
    const height = parseInt(pMatch[1], 10)
    // 假设 16:9 宽高比计算像素数
    return height * height * (16 / 9)
  }

  // WxH 格式（如 720x1080, 1024x1792）
  const wxhMatch = normalized.replace(/×/g, 'x').match(/^(\d+)x(\d+)$/)
  if (wxhMatch) {
    const w = parseInt(wxhMatch[1], 10)
    const h = parseInt(wxhMatch[2], 10)
    return w * h
  }

  // 无法识别的格式，放到最后
  return Infinity
}

/**
 * 对分辨率价格条目进行排序（从低分辨率到高分辨率）
 *
 * @param entries - 分辨率价格条目数组 [[resolution, price], ...]
 * @returns 排序后的数组
 */
export function sortResolutionEntries<T>(entries: [string, T][]): [string, T][] {
  return [...entries].sort((a, b) => getResolutionSortWeight(a[0]) - getResolutionSortWeight(b[0]))
}
