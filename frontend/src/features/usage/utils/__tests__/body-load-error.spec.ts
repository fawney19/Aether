import { describe, expect, it } from 'vitest'
import { AxiosError, type AxiosResponse } from 'axios'
import { formatRequestBodyLoadError, formatStoredBodyLoadError } from '../body-load-error'
import { RequestBodyProtocolError } from '@/api/dashboard'

describe('body load error messages', () => {
  it('distinguishes timeouts, network failures, HTTP failures and storage errors', () => {
    expect(formatRequestBodyLoadError(new AxiosError('timeout', 'ECONNABORTED'))).toContain('30 秒')
    expect(formatRequestBodyLoadError(new AxiosError('offline', 'ERR_NETWORK'))).toContain('无法连接服务器')
    for (const status of [403, 404, 500]) {
      const error = new AxiosError('failed', undefined, undefined, undefined, { status } as AxiosResponse)
      const expected = status === 403 ? '没有权限' : status === 404 ? '不存在' : 'HTTP 500'
      expect(formatRequestBodyLoadError(error)).toContain(expected)
    }
    expect(formatStoredBodyLoadError('too_large')).toContain('64 MiB')
    expect(formatStoredBodyLoadError('decode_failed')).toContain('解压或 JSON 解析失败')
    expect(formatStoredBodyLoadError('missing')).toContain('正文暂不可用')
    expect(formatStoredBodyLoadError('storage_unavailable')).toContain('读取正文存储失败')
    expect(formatStoredBodyLoadError()).toContain('读取正文存储失败')
    expect(formatRequestBodyLoadError(new RequestBodyProtocolError())).toContain('前后端均已更新')
  })
})
