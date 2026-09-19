import { describe, expect, it } from 'vitest'

import {
  CANDIDATE_SKIP_REASON_LABELS,
  formatCandidateSkipReason,
  isNonAttemptedCandidateStatus,
  isSkippedCandidateStatus,
} from '../skipReason'

describe('candidate skip reason formatting', () => {
  it('translates known skip reasons into Chinese labels', () => {
    expect(formatCandidateSkipReason('key_rpm_exhausted')).toBe('密钥本分钟请求数已达上限')
    expect(formatCandidateSkipReason('provider_concurrency_limit_reached')).toBe('上游提供商并发已达上限')
    expect(formatCandidateSkipReason('key_circuit_open')).toBe('密钥熔断中（连续失败后暂停）')
  })

  it('trims surrounding whitespace before lookup', () => {
    expect(formatCandidateSkipReason('  key_rpm_exhausted  ')).toBe('密钥本分钟请求数已达上限')
  })

  it('falls back to the raw reason so new backend reasons stay visible', () => {
    // 后端新增白名单原因、前端还没补翻译时，不能丢信息。
    expect(formatCandidateSkipReason('brand_new_reason')).toBe('brand_new_reason')
  })

  it('returns an empty string for missing or blank reasons', () => {
    expect(formatCandidateSkipReason(undefined)).toBe('')
    expect(formatCandidateSkipReason(null)).toBe('')
    expect(formatCandidateSkipReason('   ')).toBe('')
  })

  it('labels every backend allowlisted reason', () => {
    // 与后端 REQUEST_CANDIDATE_SKIP_REASONS 白名单保持同步的关键集合抽查。
    const criticalReasons = [
      'account_quota_exhausted',
      'api_key_concurrency_limit_reached',
      'auth_api_key_concurrency_limit_reached',
      'key_circuit_open',
      'key_health_score_zero',
      'key_rpm_exhausted',
      'pool_key_lease_busy',
      'provider_concurrency_limit_reached',
      'provider_key_concurrency_limit_reached',
      'provider_quota_blocked',
      'transport_unsupported',
    ]
    for (const reason of criticalReasons) {
      expect(CANDIDATE_SKIP_REASON_LABELS[reason], `missing label for ${reason}`).toBeTruthy()
    }
  })

  it('distinguishes skipped candidates from attempted ones', () => {
    expect(isSkippedCandidateStatus('skipped')).toBe(true)
    expect(isSkippedCandidateStatus('failed')).toBe(false)
    expect(isSkippedCandidateStatus(undefined)).toBe(false)

    // available/unused 同样是"从未尝试"，用于时间线是否展示的判定。
    expect(isNonAttemptedCandidateStatus('skipped')).toBe(true)
    expect(isNonAttemptedCandidateStatus('available')).toBe(true)
    expect(isNonAttemptedCandidateStatus('unused')).toBe(true)
    expect(isNonAttemptedCandidateStatus('success')).toBe(false)
    expect(isNonAttemptedCandidateStatus('failed')).toBe(false)
  })
})
