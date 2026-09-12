import axios from 'axios'
import { RequestBodyProtocolError, type RequestBodyLoadErrorCode } from '@/api/dashboard'
import { NETWORK_CONFIG } from '@/config/constants'
import { BodyDocumentError } from './body-document-protocol'

export function formatStoredBodyLoadError(code?: RequestBodyLoadErrorCode): string {
  switch (code) {
    case 'too_large':
      return '正文超过 64 MiB 的安全读取上限，无法在线预览；重复重试无法解决此问题。'
    case 'decode_failed':
      return '正文解压或 JSON 解析失败，请检查该条记录的存储数据。'
    case 'missing':
      return '正文暂不可用，可能尚未写入或已被清理，请稍后重试。'
    default:
      return '读取正文存储失败，请稍后重试。'
  }
}

export function formatRequestBodyLoadError(error: unknown): string {
  if (error instanceof RequestBodyProtocolError) return '正文接口响应不匹配，请确认前后端均已更新后刷新页面。'
  if (error instanceof BodyDocumentError) {
    if (error.code === 'too_large' || error.code === 'decode_failed') return formatStoredBodyLoadError(error.code)
    if (error.code === 'unsupported') return '当前浏览器不支持正文后台解压，请升级浏览器后重试。'
    if (error.code === 'timeout') return '浏览器后台处理正文超时，请重试或使用性能更好的设备。'
    return '正文后台处理失败，请重新加载正文。'
  }
  if (axios.isAxiosError(error)) {
    const bodyCode = error.response?.headers?.['x-aether-body-error']
    if (bodyCode === 'too_large' || bodyCode === 'missing' || bodyCode === 'storage_unavailable') return formatStoredBodyLoadError(bodyCode)
    if (error.code === 'ECONNABORTED' || error.code === 'ETIMEDOUT') {
      const seconds = Math.ceil((error.config?.timeout || NETWORK_CONFIG.API_TIMEOUT) / 1000)
      return `正文加载超时（${seconds} 秒），请检查网络后重试。`
    }
    if (error.response?.status === 403) return '没有权限查看该正文。'
    if (error.response?.status === 404) return '请求记录不存在或已被清理。'
    if (error.response) return `正文加载失败（HTTP ${error.response.status}），请稍后重试。`
    return '正文加载失败，无法连接服务器，请检查网络后重试。'
  }
  return '正文内容加载失败，请重试。'
}
