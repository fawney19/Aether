/**
 * GlobalModel API 客户端
 * 统一导出，简化导入路径
 */

export * from './endpoints/global-models'
export type {
  GlobalModelCreate,
  GlobalModelUpdate,
  GlobalModelResponse,
  GlobalModelWithStats,
  GlobalModelListResponse,
} from './endpoints/types'

// 重新导出为更简洁的函数名
export {
  getGlobalModels as listGlobalModels,
  getGlobalModel,
  createGlobalModel,
  updateGlobalModel,
  deleteGlobalModel,
  batchAssignToProviders,
  getGlobalModelProviders,
  getGlobalModelRoutingPreview,
} from './endpoints/global-models'
