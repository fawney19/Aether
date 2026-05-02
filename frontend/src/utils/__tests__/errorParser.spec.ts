import { describe, expect, it } from 'vitest'

import { parseApiError } from '@/utils/errorParser'

describe('errorParser', () => {
  it('normalizes reused refresh token errors from legacy string details', () => {
    const error = {
      response: {
        data: {
          detail: "token refresh 失败: {'message': 'Your refresh token has already been used to generate a new access token. Please try signing in again.', 'type': 'invalid_request_error', 'param': None, 'code': 'refresh_token_reused'}",
        },
      },
    }

    expect(parseApiError(error, 'Token 刷新失败')).toBe(
      'Token 刷新失败：refresh_token 已被使用并轮换，请重新登录授权',
    )
  })

  it('keeps normalized Chinese refresh failures intact', () => {
    const error = {
      response: {
        data: {
          detail: 'Token 刷新失败：refresh_token 已被使用并轮换，请重新登录授权',
        },
      },
    }

    expect(parseApiError(error, 'Token 刷新失败')).toBe(
      'Token 刷新失败：refresh_token 已被使用并轮换，请重新登录授权',
    )
  })

  it('normalizes expired refresh token errors', () => {
    const error = {
      response: {
        data: {
          detail: '{"error":{"message":"Could not validate your refresh token. Please try signing in again.","type":"invalid_request_error","code":"refresh_token_expired"}}',
        },
      },
    }

    expect(parseApiError(error, 'Token 刷新失败')).toBe(
      'Token 刷新失败：refresh_token 无效、已过期或已撤销，请重新登录授权',
    )
  })
})
