import apiClient from './client'

export interface OAuthProviderInfo {
  provider_type: string
  display_name: string
  icon_url?: string | null
}

export interface OAuthProvidersResponse {
  providers: OAuthProviderInfo[]
}

export interface OAuthLinkInfo {
  provider_type: string
  display_name: string
  icon_url?: string | null
  provider_username?: string | null
  provider_email?: string | null
  linked_at?: string | null
  last_login_at?: string | null
  provider_enabled?: boolean
}

export interface OAuthLinksResponse {
  links: OAuthLinkInfo[]
}

// Admin
export interface SupportedOAuthType {
  provider_type: string
  display_name: string
  default_authorization_url: string
  default_token_url: string
  default_userinfo_url: string
  default_scopes: string[]
}

export interface OAuthProviderAdminConfig {
  provider_type: string
  display_name: string
  client_id: string
  has_secret: boolean
  authorization_url_override?: string | null
  token_url_override?: string | null
  userinfo_url_override?: string | null
  scopes?: string[] | null
  redirect_uri: string
  frontend_callback_url: string
  attribute_mapping?: Record<string, unknown> | null
  extra_config?: Record<string, unknown> | null
  icon_url?: string | null
  is_enabled: boolean
}

export interface OAuthProviderUpsertRequest {
  display_name: string
  client_id: string
  client_secret?: string
  authorization_url_override?: string | null
  token_url_override?: string | null
  userinfo_url_override?: string | null
  scopes?: string[] | null
  redirect_uri: string
  frontend_callback_url: string
  attribute_mapping?: Record<string, unknown> | null
  extra_config?: Record<string, unknown> | null
  icon_url?: string | null
  is_enabled: boolean
  force?: boolean
}

export interface OAuthProviderTestResponse {
  authorization_url_reachable: boolean
  token_url_reachable: boolean
  secret_status: 'likely_valid' | 'configured' | 'invalid' | 'unknown' | 'not_provided' | string
  details?: string
}

export interface OAuthProviderTestRequest {
  client_id: string
  client_secret?: string
  authorization_url_override?: string | null
  token_url_override?: string | null
  redirect_uri: string
}

export const oauthApi = {
  async getProviders(): Promise<OAuthProviderInfo[]> {
    const response = await apiClient.get<OAuthProvidersResponse>('/api/oauth/providers')
    return response.data.providers || []
  },

  async getBindableProviders(): Promise<OAuthProviderInfo[]> {
    const response = await apiClient.get<OAuthProvidersResponse>('/api/user/oauth/bindable-providers')
    return response.data.providers || []
  },

  async getMyLinks(): Promise<OAuthLinkInfo[]> {
    const response = await apiClient.get<OAuthLinksResponse>('/api/user/oauth/links')
    return response.data.links || []
  },

  async createBindToken(providerType: string): Promise<string> {
    const response = await apiClient.post<{ bind_token: string }>(`/api/user/oauth/${providerType}/bind-token`)
    return response.data.bind_token
  },

  async unbind(providerType: string): Promise<{ message: string }> {
    const response = await apiClient.delete<{ message: string }>(`/api/user/oauth/${providerType}`)
    return response.data
  },

  admin: {
    async getSupportedTypes(): Promise<SupportedOAuthType[]> {
      const response = await apiClient.get<SupportedOAuthType[]>('/api/admin/oauth/supported-types')
      return response.data || []
    },

    async listProviderConfigs(): Promise<OAuthProviderAdminConfig[]> {
      const response = await apiClient.get<OAuthProviderAdminConfig[]>('/api/admin/oauth/providers')
      return response.data || []
    },

    async getProviderConfig(providerType: string): Promise<OAuthProviderAdminConfig> {
      const response = await apiClient.get<OAuthProviderAdminConfig>(`/api/admin/oauth/providers/${encodeURIComponent(providerType)}`)
      return response.data
    },

    async upsertProviderConfig(providerType: string, payload: OAuthProviderUpsertRequest): Promise<OAuthProviderAdminConfig> {
      const response = await apiClient.put<OAuthProviderAdminConfig>(`/api/admin/oauth/providers/${encodeURIComponent(providerType)}`, payload)
      return response.data
    },

    async deleteProviderConfig(providerType: string): Promise<{ message: string }> {
      const response = await apiClient.delete<{ message: string }>(`/api/admin/oauth/providers/${encodeURIComponent(providerType)}`)
      return response.data
    },

    async testProviderConfig(providerType: string, payload: OAuthProviderTestRequest): Promise<OAuthProviderTestResponse> {
      const response = await apiClient.post<OAuthProviderTestResponse>(`/api/admin/oauth/providers/${encodeURIComponent(providerType)}/test`, payload)
      return response.data
    },
  }
}
