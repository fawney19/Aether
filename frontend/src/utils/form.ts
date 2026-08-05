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

/** `config.billing.video`，两种计费方式是覆盖二选一的关系。 */
export type VideoBillingMode = 'per_second' | 'per_token'

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null
}

/** 读取模型配置里的 `billing.video` 段。 */
export function getVideoBillingConfig(config: unknown): Record<string, unknown> | null {
  const billing = asRecord(asRecord(config)?.billing)
  return asRecord(billing?.video)
}

/**
 * 当前生效的视频计费方式。
 *
 * 后端缺省即按秒，所以只有显式写了 `per_token` 才是按 token。切换计费方式时
 * 前端不会清空另一种价格表（方便来回对比），因此展示价格前必须先判定方式，
 * 否则一个改成按 token 的模型仍会显示遗留的秒价。
 */
export function getVideoBillingMode(config: unknown): VideoBillingMode {
  return getVideoBillingConfig(config)?.mode === 'per_token' ? 'per_token' : 'per_second'
}

/** 按秒价格表，仅在按秒计费时返回；其余情况返回空表。 */
export function getVideoPricePerSecondTable(config: unknown): Record<string, number> {
  if (getVideoBillingMode(config) !== 'per_second') return {}
  const table = asRecord(getVideoBillingConfig(config)?.price_per_second_by_resolution)
  if (!table) return {}
  const prices: Record<string, number> = {}
  for (const [resolution, price] of Object.entries(table)) {
    if (typeof price === 'number' && Number.isFinite(price)) prices[resolution] = price
  }
  return prices
}

/** 按秒计费的默认价（未列出的分辨率），仅在按秒计费时返回。 */
export function getVideoDefaultPricePerSecond(config: unknown): number | null {
  if (getVideoBillingMode(config) !== 'per_second') return null
  const value = getVideoBillingConfig(config)?.price_per_second_default
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/** 是否配置了生效的按秒视频计费（价格表或默认价二者其一）。 */
export function hasVideoPerSecondPricing(config: unknown): boolean {
  return Object.keys(getVideoPricePerSecondTable(config)).length > 0
    || getVideoDefaultPricePerSecond(config) !== null
}

/** 是否配置了生效的按 token 视频计费。 */
export function hasVideoPerTokenPricing(config: unknown): boolean {
  if (getVideoBillingMode(config) !== 'per_token') return false
  const video = getVideoBillingConfig(config)
  const table = asRecord(video?.token_prices_by_resolution)
  const withInput = asRecord(asRecord(video?.with_video_input)?.token_prices_by_resolution)
  return Object.keys(table ?? {}).length > 0
    || Object.keys(withInput ?? {}).length > 0
    || asRecord(video?.token_price_default) !== null
    || asRecord(asRecord(video?.with_video_input)?.token_price_default) !== null
}

/** 一条分辨率的 token 单价。 */
export type VideoTokenPriceEntry = {
  resolution: string
  inputPricePer1m: number | null
  outputPricePer1m: number | null
}

function readTokenPriceEntry(value: unknown): { input: number | null, output: number | null } | null {
  const entry = asRecord(value)
  if (!entry) return null
  const read = (field: string): number | null => {
    const raw = entry[field]
    return typeof raw === 'number' && Number.isFinite(raw) ? raw : null
  }
  const input = read('input_price_per_1m')
  const output = read('output_price_per_1m')
  return input === null && output === null ? null : { input, output }
}

/**
 * 按 token 计费的分辨率单价，仅在按 token 计费时返回。
 *
 * 默认价以 `resolution: '默认'` 的形式排在最后，这样展示端可以把它当成表格里
 * 的普通一行。含视频输入的覆盖价不在这里返回 —— 列表只需要呈现基准价。
 */
export function getVideoTokenPriceEntries(config: unknown): VideoTokenPriceEntry[] {
  if (getVideoBillingMode(config) !== 'per_token') return []
  const video = getVideoBillingConfig(config)
  const table = asRecord(video?.token_prices_by_resolution) ?? {}
  const entries: VideoTokenPriceEntry[] = []
  for (const [resolution, value] of sortResolutionEntries(Object.entries(table))) {
    const prices = readTokenPriceEntry(value)
    if (prices) {
      entries.push({
        resolution,
        inputPricePer1m: prices.input,
        outputPricePer1m: prices.output,
      })
    }
  }
  const defaultPrices = readTokenPriceEntry(video?.token_price_default)
  if (defaultPrices) {
    entries.push({
      resolution: '默认',
      inputPricePer1m: defaultPrices.input,
      outputPricePer1m: defaultPrices.output,
    })
  }
  return entries
}

/** 模型是否按视频计费（任一方式），用于「Video」能力标签。 */
export function hasVideoPricing(config: unknown): boolean {
  return hasVideoPerSecondPricing(config) || hasVideoPerTokenPricing(config)
}
