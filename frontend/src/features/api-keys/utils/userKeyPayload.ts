export interface UserApiKeyMutationPayload {
  name: string
  rate_limit: number
  daily_usage_limit_usd: number
  concurrent_limit?: number
}

interface BuildUserApiKeyMutationPayloadInput {
  name: string
  rate_limit?: number
  daily_usage_limit_usd?: number
  concurrent_limit?: number
}

export function buildUserApiKeyMutationPayload(
  input: BuildUserApiKeyMutationPayloadInput,
): UserApiKeyMutationPayload {
  return {
    name: input.name,
    rate_limit: input.rate_limit ?? 0,
    daily_usage_limit_usd: input.daily_usage_limit_usd ?? 0,
    ...(input.concurrent_limit === undefined ? {} : { concurrent_limit: input.concurrent_limit }),
  }
}
