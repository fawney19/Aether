-- 邀请返利历史核查：仅生成待人工核查的候选记录，不推定当前规则适用于旧订单。
-- 本文件不修改表结构、不写业务数据、不执行补发。请在另行确认的只读环境运行。
-- 没有奖励可能是历史开关关闭、费率为零或仅首付人头模式，不能直接认定漏发。

-- 1. 旧订单没有付款时规则快照，也没有付款奖励。需要核对当时规则与支付凭证。
SELECT
    po.id AS payment_order_id,
    po.order_no,
    po.user_id AS invitee_user_id,
    ur.id AS referral_id,
    ur.inviter_user_id,
    po.order_kind,
    po.payment_method,
    po.amount_usd,
    po.credited_at,
    ur.first_paid_order_id,
    'missing_historical_rule_evidence' AS review_reason
FROM payment_orders po
JOIN user_referrals ur ON ur.invitee_user_id = po.user_id
WHERE po.status = 'credited'
  AND po.order_kind IN ('wallet_recharge', 'plan_purchase')
  AND LOWER(BTRIM(po.payment_method)) NOT IN (
      'manual', 'admin_manual', 'redeem_code', 'gift', 'card_code', 'gift_code', 'card_recharge'
  )
  AND po.amount_usd > 0
  AND po.gateway_response -> '_aether_referral_v1' IS NULL
  AND NOT EXISTS (
      SELECT 1 FROM referral_rewards rw WHERE rw.source_order_id = po.id
  )
ORDER BY po.credited_at, po.id;

-- 2. 注册类奖励已入账，但受邀账户已不存在或被删除。
-- 这仅是回滚异常的候选，也可能是正常注销；未经核实不得自动扣款。
SELECT
    rw.id AS reward_id,
    rw.referral_id,
    rw.inviter_user_id,
    rw.invitee_user_id,
    rw.trigger_point,
    rw.amount_usd,
    rw.wallet_transaction_id,
    rw.created_at,
    'registration_reward_for_missing_or_deleted_invitee' AS review_reason
FROM referral_rewards rw
LEFT JOIN users invitee ON invitee.id = rw.invitee_user_id
WHERE rw.status = 'applied'
  AND rw.source_order_id IS NULL
  AND rw.trigger_point IN ('registration', 'email_verified')
  AND (invitee.id IS NULL OR invitee.is_deleted IS TRUE)
ORDER BY rw.created_at, rw.id;

-- 3. 已记录的待冲回款项及邀请人当前赠款余额，便于管理员安排核查。
-- 同一邀请人的多行共享钱包余额，不可把每行余额相加或逐行视作独立可扣额度。
SELECT
    rw.id AS reward_id,
    rw.source_order_id AS payment_order_id,
    rw.inviter_user_id,
    rw.amount_usd,
    rw.reversed_amount_usd,
    rw.pending_reversal_amount_usd,
    wallet.gift_balance AS inviter_gift_balance,
    wallet.status AS inviter_wallet_status,
    rw.updated_at AS last_attempt_at
FROM referral_rewards rw
LEFT JOIN wallets wallet ON wallet.user_id = rw.inviter_user_id
WHERE rw.pending_reversal_amount_usd > 0
ORDER BY rw.updated_at, rw.id;

-- 4. 旧邀请关系未标记首付，但已有真实入账订单。仅核查身份，不推定旧返利资格。
SELECT
    ur.id AS referral_id,
    ur.inviter_user_id,
    ur.invitee_user_id,
    first_order.id AS first_observed_payment_order_id,
    first_order.credited_at,
    first_order.paid_at,
    first_order.created_at,
    'missing_first_paid_identity' AS review_reason
FROM user_referrals ur
JOIN LATERAL (
    SELECT po.id, po.credited_at, po.paid_at, po.created_at
    FROM payment_orders po
    WHERE po.user_id = ur.invitee_user_id
      AND po.status = 'credited'
      AND po.order_kind IN ('wallet_recharge', 'plan_purchase')
      AND LOWER(BTRIM(po.payment_method)) NOT IN (
          'manual', 'admin_manual', 'redeem_code', 'gift', 'card_code', 'gift_code', 'card_recharge'
      )
      AND po.amount_usd > 0
    ORDER BY COALESCE(po.credited_at, po.paid_at, po.created_at), po.id
    LIMIT 1
) first_order ON TRUE
WHERE ur.first_paid_order_id IS NULL
ORDER BY ur.created_at, ur.id;
