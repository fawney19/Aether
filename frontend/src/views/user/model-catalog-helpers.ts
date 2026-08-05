import type { PublicGlobalModel } from '@/api/public-models'
import { hasVideoPricing as hasVideoPricingConfigured } from '@/utils/form'

function isEmbeddingApiFormat(format: unknown): boolean {
  const value = String(format).trim().toLowerCase()
  return value.endsWith(':embedding') || value === 'aliyun:multimodal_embedding'
}

export function supportsEmbedding(model: PublicGlobalModel): boolean {
  return model.supports_embedding === true
    || model.supported_capabilities?.includes('embedding') === true
    || model.config?.embedding === true
    || model.config?.model_type === 'embedding'
    || (Array.isArray(model.config?.api_formats) && model.config.api_formats.some(isEmbeddingApiFormat))
}

export function supportsRerank(model: PublicGlobalModel): boolean {
  return model.supported_capabilities?.includes('rerank') === true
    || model.config?.rerank === true
    || model.config?.model_type === 'rerank'
    || (Array.isArray(model.config?.api_formats) && model.config.api_formats.some((format) => String(format).endsWith(':rerank')))
}

/**
 * 是否按视频计费（两种方式任一）。
 *
 * 这里是能力标签，不是价格展示，所以按秒和按 token 都算；判定委托给共享
 * helper，避免各处重复实现时漏掉计费方式。
 */
export function hasVideoPricing(model: PublicGlobalModel): boolean {
  return hasVideoPricingConfigured(model.config)
}

export function getModelCapabilityLabels(model: PublicGlobalModel): string[] {
  const labels: string[] = []
  if (supportsRerank(model)) {
    labels.push('Rerank')
  } else if (supportsEmbedding(model)) {
    labels.push('Embedding')
  } else {
    labels.push('Chat')
  }
  if (model.config?.image_generation === true) labels.push('Image')
  if (hasVideoPricing(model)) labels.push('Video')
  return labels
}
