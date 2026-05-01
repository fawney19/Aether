import apiClient from './client'

export interface ModelGroupRoute {
  id?: string
  provider_id: string
  provider_name?: string | null
  provider_api_key_id?: string | null
  provider_api_key_name?: string | null
  priority: number
  user_billing_multiplier_override?: number | null
  is_active: boolean
}

export interface ModelGroupModelRef {
  id: string
  global_model_id: string
  model_name: string
  model_display_name: string
  is_active: boolean
}

export interface ModelGroupUserGroupRef {
  user_group_id: string
  user_group_name: string
  priority: number
  is_active: boolean
}

export interface ModelGroupSummary {
  id: string
  name: string
  display_name: string
  description?: string | null
  default_user_billing_multiplier: number
  is_default: boolean
  is_active: boolean
  sort_order: number
  model_count: number
  user_group_count: number
  created_at: string
  updated_at?: string | null
}

export interface ModelGroupDetail extends ModelGroupSummary {
  models: ModelGroupModelRef[]
  routes: ModelGroupRoute[]
  user_groups: ModelGroupUserGroupRef[]
}

export interface UpsertModelGroupRequest {
  name: string
  display_name: string
  description?: string | null
  default_user_billing_multiplier?: number
  is_active?: boolean
  sort_order?: number
  model_ids?: string[]
  routes?: ModelGroupRoute[]
}

export type UpdateModelGroupRequest = Partial<UpsertModelGroupRequest>

export const modelGroupsApi = {
  async list(): Promise<ModelGroupSummary[]> {
    const response = await apiClient.get<{ model_groups: ModelGroupSummary[] }>('/api/admin/models/groups')
    return response.data.model_groups
  },

  async get(groupId: string): Promise<ModelGroupDetail> {
    const response = await apiClient.get<ModelGroupDetail>(`/api/admin/models/groups/${groupId}`)
    return response.data
  },

  async create(data: UpsertModelGroupRequest): Promise<ModelGroupDetail> {
    const response = await apiClient.post<ModelGroupDetail>('/api/admin/models/groups', data)
    return response.data
  },

  async update(groupId: string, data: UpdateModelGroupRequest): Promise<ModelGroupDetail> {
    const response = await apiClient.patch<ModelGroupDetail>(`/api/admin/models/groups/${groupId}`, data)
    return response.data
  },

  async delete(groupId: string): Promise<void> {
    await apiClient.delete(`/api/admin/models/groups/${groupId}`)
  },
}
