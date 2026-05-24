export type PaymentInstructionMap = Record<string, unknown>

export interface StripePaymentInstructions {
  gateway: string
  displayName: string
  gatewayOrderId: string
  intentId: string
  clientSecret: string
  publishableKey: string
  expiresAt: string
  payAmount: number | null
  payCurrency: string
  paymentChannel: string
  paymentMethodTypes: string[]
  submitMethod: string
}

function getInstructionValue(
  instructions: PaymentInstructionMap | null | undefined,
  key: string
): unknown {
  if (!instructions || typeof instructions !== 'object' || Array.isArray(instructions)) {
    return undefined
  }
  return instructions[key]
}

export function getPaymentInstructionString(
  instructions: PaymentInstructionMap | null | undefined,
  key: string
): string {
  const value = getInstructionValue(instructions, key)
  return typeof value === 'string' ? value.trim() : ''
}

export function getPaymentInstructionNumber(
  instructions: PaymentInstructionMap | null | undefined,
  key: string
): number | null {
  const value = getInstructionValue(instructions, key)
  const parsed = typeof value === 'number' ? value : Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

export function getPaymentInstructionStringArray(
  instructions: PaymentInstructionMap | null | undefined,
  key: string
): string[] {
  const value = getInstructionValue(instructions, key)
  if (Array.isArray(value)) {
    return value
      .map(item => (typeof item === 'string' ? item.trim() : ''))
      .filter(Boolean)
  }
  if (typeof value === 'string') {
    return value
      .split(',')
      .map(item => item.trim())
      .filter(Boolean)
  }
  return []
}

export function getStripePaymentInstructions(
  instructions: PaymentInstructionMap | null | undefined
): StripePaymentInstructions | null {
  const clientSecret = getPaymentInstructionString(instructions, 'client_secret')
  const publishableKey = getPaymentInstructionString(instructions, 'publishable_key')
  if (!clientSecret || !publishableKey) {
    return null
  }

  const intentId = getPaymentInstructionString(instructions, 'intent_id')
    || getPaymentInstructionString(instructions, 'gateway_order_id')
  const gatewayOrderId = getPaymentInstructionString(instructions, 'gateway_order_id')
    || intentId
  const paymentChannel = getPaymentInstructionString(instructions, 'payment_channel')

  return {
    gateway: getPaymentInstructionString(instructions, 'gateway') || 'stripe',
    displayName: getPaymentInstructionString(instructions, 'display_name') || paymentChannel || 'Stripe',
    gatewayOrderId,
    intentId,
    clientSecret,
    publishableKey,
    expiresAt: getPaymentInstructionString(instructions, 'expires_at'),
    payAmount: getPaymentInstructionNumber(instructions, 'pay_amount'),
    payCurrency: getPaymentInstructionString(instructions, 'pay_currency'),
    paymentChannel,
    paymentMethodTypes: getPaymentInstructionStringArray(instructions, 'payment_method_types'),
    submitMethod: getPaymentInstructionString(instructions, 'submit_method') || 'stripe_payment_intent',
  }
}

export function hasStripePaymentInstructions(
  instructions: PaymentInstructionMap | null | undefined
): boolean {
  return getStripePaymentInstructions(instructions) !== null
}
