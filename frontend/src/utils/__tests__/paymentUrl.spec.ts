import { describe, expect, it } from 'vitest'

import { safePaymentTargetUrl } from '../paymentUrl'

describe('safePaymentTargetUrl', () => {
  it('allows absolute HTTPS targets', () => {
    expect(safePaymentTargetUrl('https://pay.example/submit?order=1')).toBe(
      'https://pay.example/submit?order=1',
    )
    expect(safePaymentTargetUrl(' HTTPS://pay.example/submit ')).toBe(
      'https://pay.example/submit',
    )
  })

  it.each([
    'https://checkout.stripe.com/c/pay/cs_test_checkout#fidkdWxOYHwnPyd1blpxYHZxWjA0',
    'https://pay.example/submit?order=1&signature=a%2Fb%3D#session=a%2Bb%3D',
  ])('preserves payment session fragments: %s', (value) => {
    expect(safePaymentTargetUrl(value)).toBe(value)
  })

  it.each([
    'javascript:alert(1)',
    'data:text/html,attack',
    'http://pay.example/submit',
    '//pay.example/submit',
    'https://user:secret@pay.example/submit',
    'https://user:secret@pay.example/submit#session',
    'https://pay.example\\@attacker.example/submit#session',
    '/\\pay.example/submit',
    '/payment/continue?order=1',
    '../payment/continue?order=1',
    'submit.php',
  ])('rejects an unsafe payment target: %s', (value) => {
    expect(safePaymentTargetUrl(value)).toBeNull()
  })
})
