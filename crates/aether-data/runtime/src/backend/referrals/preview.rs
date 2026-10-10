use super::*;
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize)]
pub struct ReferralRefundPreview {
    pub reward_id: String,
    pub inviter_user_id: String,
    pub inviter_username: Option<String>,
    pub amount_to_reverse_usd: f64,
    pub available_gift_balance_usd: f64,
    pub deductible_amount_usd: f64,
    pub pending_amount_usd: f64,
    pub already_reversed_amount_usd: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ReferralReversalSummary {
    pub reversed_amount_usd: f64,
    pub pending_reversal_amount_usd: f64,
}

fn allocate_reversal(remaining: &mut f64, due: f64) -> (f64, f64) {
    let deductible = remaining.max(0.0).min(due.max(0.0));
    *remaining = (*remaining - deductible).max(0.0);
    (deductible, (due - deductible).max(0.0))
}

impl ReferralDataState<'_> {
    pub async fn referral_order_reversal_summary(
        &self,
        order_id: &str,
    ) -> Result<ReferralReversalSummary, DataLayerError> {
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let row=sqlx::query("SELECT CAST(COALESCE(SUM(reversed_amount_usd),0) AS DOUBLE PRECISION) AS reversed_amount_usd,CAST(COALESCE(SUM(pending_reversal_amount_usd),0) AS DOUBLE PRECISION) AS pending_reversal_amount_usd FROM referral_rewards WHERE source_order_id=$1")
                .bind(order_id).fetch_one(&backend.pool_clone()).await.map_err(DataLayerError::postgres)?;
            return Ok(ReferralReversalSummary {
                reversed_amount_usd: referral_stats_amount(row_f64!(row, "reversed_amount_usd")),
                pending_reversal_amount_usd: referral_stats_amount(row_f64!(
                    row,
                    "pending_reversal_amount_usd"
                )),
            });
        }
        Ok(ReferralReversalSummary::default())
    }
    /// `proposed_amount` is this refund, excluding any already succeeded refund.
    /// The durable order context subtracts reservations still processing.
    pub async fn referral_refund_preview(
        &self,
        order_id: &str,
        proposed_amount: f64,
    ) -> Result<Vec<ReferralRefundPreview>, DataLayerError> {
        if !proposed_amount.is_finite() || proposed_amount < 0.0 {
            return Err(DataLayerError::InvalidInput("退款金额无效".into()));
        }
        let Some(context) = self
            .find_referral_payment_order_refund_context(order_id)
            .await?
        else {
            return Ok(Vec::new());
        };
        if !context.amount_usd.is_finite()
            || context.amount_usd <= 0.0
            || !context.refunded_amount_usd.is_finite()
        {
            return Err(DataLayerError::InvalidInput("退款订单金额无效".into()));
        }
        let cumulative = (context.refunded_amount_usd + proposed_amount).min(context.amount_usd);
        #[cfg(feature = "postgres")]
        if let Some(backend) = self.backends.and_then(DataBackends::postgres) {
            let rows=sqlx::query("SELECT rw.id,rw.inviter_user_id,u.username AS inviter_username,CAST(rw.amount_usd AS DOUBLE PRECISION) AS amount_usd,CAST(rw.reversed_amount_usd AS DOUBLE PRECISION) AS reversed_amount_usd,CAST(rw.pending_reversal_amount_usd AS DOUBLE PRECISION) AS pending_reversal_amount_usd,CAST(CASE WHEN w.status='active' AND u.is_active IS TRUE AND u.is_deleted IS FALSE THEN w.gift_balance ELSE 0 END AS DOUBLE PRECISION) AS gift_balance FROM referral_rewards rw LEFT JOIN users u ON u.id=rw.inviter_user_id LEFT JOIN wallets w ON w.user_id=rw.inviter_user_id WHERE rw.source_order_id=$1 AND rw.status IN ('applied','reversed') ORDER BY rw.id")
                .bind(order_id).fetch_all(&backend.pool_clone()).await.map_err(DataLayerError::postgres)?;
            let mut available = HashMap::<String, f64>::new();
            let mut result = Vec::new();
            for row in rows {
                let inviter = row_string!(row, "inviter_user_id");
                let gift = row
                    .try_get::<Option<f64>, _>("gift_balance")
                    .map_err(DataLayerError::postgres)?
                    .unwrap_or(0.0);
                if !gift.is_finite() || gift < 0.0 {
                    return Err(DataLayerError::InvalidInput("邀请人钱包余额无效".into()));
                }
                let amount = row_f64!(row, "amount_usd");
                let reversed = row_f64!(row, "reversed_amount_usd");
                let pending = row_f64!(row, "pending_reversal_amount_usd");
                let target = referral_reversal_target(amount, context.amount_usd, cumulative);
                if !referral_reversal_inputs_valid(amount, target, reversed, pending) {
                    return Err(DataLayerError::InvalidInput("邀请返利冲回状态无效".into()));
                }
                let due = referral_reversal_due_bounded(target, amount, reversed, pending);
                let remaining = available.entry(inviter.clone()).or_insert(gift);
                let (deductible, shortfall) = allocate_reversal(remaining, due);
                result.push(ReferralRefundPreview {
                    reward_id: row_string!(row, "id"),
                    inviter_user_id: inviter,
                    inviter_username: row_optional_string!(row, "inviter_username"),
                    amount_to_reverse_usd: due,
                    available_gift_balance_usd: gift,
                    deductible_amount_usd: deductible,
                    pending_amount_usd: shortfall,
                    already_reversed_amount_usd: reversed,
                });
            }
            return Ok(result);
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewards_share_one_wallet_without_double_counting_gifts() {
        let mut available = 5.0;
        assert_eq!(allocate_reversal(&mut available, 4.0), (4.0, 0.0));
        assert_eq!(allocate_reversal(&mut available, 4.0), (1.0, 3.0));
        assert_eq!(available, 0.0);
    }
    #[test]
    fn cumulative_refunds_and_existing_debt_are_not_added_twice() {
        let target = referral_reversal_target(10.0, 100.0, 50.0);
        assert_eq!(referral_reversal_due_bounded(target, 10.0, 2.0, 3.0), 3.0);
    }
}
