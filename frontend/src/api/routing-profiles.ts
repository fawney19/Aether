import client from './client'
import type { RoutingDecisionTrace } from '@/features/routing/utils/routingTrace'
import type {
  RoutingGroupConfig,
  RoutingRulePhase,
} from '@/features/routing/utils/routingPolicy'

export type RoutingBindingSubjectType = 'user' | 'api_key' | 'user_group'

export interface RoutingGroupRecord {
  id: string
  name: string
  description?: string | null
  enabled: boolean
  is_system_default: boolean
  sort_order: number
  config_json: RoutingGroupConfig
  version: number
  created_at: number
  updated_at: number
  published_at?: number | null
}

export interface RoutingGroupListResponse {
  items: RoutingGroupRecord[]
  total: number
}

export interface RoutingGroupVersionRecord {
  id: string
  group_id: string
  version: number
  config_json: RoutingGroupConfig
  created_at: number
  created_by?: string | null
}

export interface RoutingGroupVersionListResponse {
  items: RoutingGroupVersionRecord[]
  total: number
}

export interface RoutingGroupBindingRecord {
  id: string
  group_id: string
  subject_type: RoutingBindingSubjectType
  subject_id: string
  is_default: boolean
  allow_explicit_select: boolean
  created_at: number
  updated_at: number
}

export interface RoutingGroupBindingListResponse {
  items: RoutingGroupBindingRecord[]
  total: number
}

export interface RoutingGroupCreateRequest {
  id?: string
  name: string
  description?: string | null
  enabled?: boolean
  is_system_default?: boolean
  sort_order?: number
  config_json?: RoutingGroupConfig
}

export interface RoutingGroupUpdateRequest {
  name?: string
  description?: string | null
  enabled?: boolean
  is_system_default?: boolean
  sort_order?: number
  config_json?: RoutingGroupConfig
  version?: number
  published_at?: number | null
}

export interface RoutingGroupBindingCreateRequest {
  id?: string
  group_id: string
  subject_type: RoutingBindingSubjectType
  subject_id: string
  is_default?: boolean
  allow_explicit_select?: boolean
}

export interface RoutingGroupBindingUpdateRequest {
  group_id?: string
  subject_type?: RoutingBindingSubjectType
  subject_id?: string
  is_default?: boolean
  allow_explicit_select?: boolean
}

export interface RoutingDryRunRequest {
  model: string
  resolved_model?: string
  api_format?: string
  user_id?: string
  api_key_id?: string
  headers?: Record<string, string>
  body?: unknown
  phase?: RoutingRulePhase
}

export interface RoutingDryRunResponse {
  group: RoutingGroupRecord
  policy: unknown
  trace_seed: RoutingDecisionTrace
  patch_summary: unknown
  mutated_body: unknown
  mutated_headers: Record<string, string>
  candidate_preview: unknown
}

export async function listRoutingGroups(): Promise<RoutingGroupListResponse> {
  const response = await client.get<RoutingGroupListResponse>('/api/admin/routing/groups')
  return response.data
}

export async function getRoutingGroup(groupId: string): Promise<RoutingGroupRecord> {
  const response = await client.get<RoutingGroupRecord>(`/api/admin/routing/groups/${groupId}`)
  return response.data
}

export async function createRoutingGroup(data: RoutingGroupCreateRequest): Promise<RoutingGroupRecord> {
  const response = await client.post<RoutingGroupRecord>('/api/admin/routing/groups', data)
  return response.data
}

export async function updateRoutingGroup(
  groupId: string,
  data: RoutingGroupUpdateRequest
): Promise<RoutingGroupRecord> {
  const response = await client.patch<RoutingGroupRecord>(`/api/admin/routing/groups/${groupId}`, data)
  return response.data
}

export async function deleteRoutingGroup(groupId: string): Promise<void> {
  await client.delete(`/api/admin/routing/groups/${groupId}`)
}

export async function publishRoutingGroup(groupId: string): Promise<RoutingGroupRecord> {
  const response = await client.post<RoutingGroupRecord>(`/api/admin/routing/groups/${groupId}/publish`)
  return response.data
}

export async function listRoutingGroupVersions(groupId: string): Promise<RoutingGroupVersionListResponse> {
  const response = await client.get<RoutingGroupVersionListResponse>(`/api/admin/routing/groups/${groupId}/versions`)
  return response.data
}

export async function dryRunRoutingGroup(
  groupId: string,
  data: RoutingDryRunRequest
): Promise<RoutingDryRunResponse> {
  const response = await client.post<RoutingDryRunResponse>(`/api/admin/routing/groups/${groupId}/dry-run`, data)
  return response.data
}

export async function listRoutingGroupBindings(params?: {
  group_id?: string
  subject_type?: RoutingBindingSubjectType
  subject_id?: string
}): Promise<RoutingGroupBindingListResponse> {
  const response = await client.get<RoutingGroupBindingListResponse>('/api/admin/routing/bindings', { params })
  return response.data
}

export async function createRoutingGroupBinding(
  data: RoutingGroupBindingCreateRequest
): Promise<RoutingGroupBindingRecord> {
  const response = await client.post<RoutingGroupBindingRecord>('/api/admin/routing/bindings', data)
  return response.data
}

export async function updateRoutingGroupBinding(
  bindingId: string,
  data: RoutingGroupBindingUpdateRequest
): Promise<RoutingGroupBindingRecord> {
  const response = await client.patch<RoutingGroupBindingRecord>(`/api/admin/routing/bindings/${bindingId}`, data)
  return response.data
}

export async function deleteRoutingGroupBinding(bindingId: string): Promise<void> {
  await client.delete(`/api/admin/routing/bindings/${bindingId}`)
}
