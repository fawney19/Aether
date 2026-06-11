import { describe, expect, it } from 'vitest'

import {
  formatFailureCodeLabel,
  formatFailureTypeLabel,
  resolveFailureReason,
} from '../failureDisplay'

describe('failure display helpers', () => {
  it('preserves a meaningful upstream message when the error code is an unknown snake_case value', () => {
    expect(resolveFailureReason({
      code: 'invalid_responses_request',
      message: 'invalid codex request (request id: 20260610175106369998440o1JVMfnL)',
      statusCode: 400,
    })).toBe('invalid codex request (request id: 20260610175106369998440o1JVMfnL)')
  })

  it('keeps unknown external codes separate from internal error type inference', () => {
    expect(formatFailureCodeLabel('invalid_responses_request')).toBeNull()
    expect(formatFailureTypeLabel('invalid_responses_request')).toBe('内部执行错误')
  })
})
