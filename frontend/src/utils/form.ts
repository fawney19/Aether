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
export function createNumberInputHandler<T extends Record<string, any>>(
  obj: T,
  field: keyof T,
  options: Parameters<typeof parseNumberInput>[1] = {}
) {
  return (value: string | number | null | undefined) => {
    (obj as any)[field] = parseNumberInput(value, options)
  }
}
