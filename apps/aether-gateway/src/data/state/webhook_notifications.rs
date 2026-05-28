use aether_data::repository::webhook_notifications::{
    ClaimWebhookDeliveriesRequest, CreateWebhookDeliveryRecord, RecordWebhookDeliveryAttemptRecord,
    RescheduleWebhookDeliveryRecord, StoredWebhookDelivery, StoredWebhookDeliveryAttempt,
    StoredWebhookDeliveryPage, StoredWebhookEndpoint, UpdateWebhookEndpointTestResultRecord,
    UpsertWebhookEndpointRecord, WebhookDeliveryAttemptListQuery, WebhookDeliveryListQuery,
    WebhookEndpointListQuery,
};
use aether_data::DataLayerError;

use super::GatewayDataState;

impl GatewayDataState {
    pub(crate) async fn list_webhook_endpoints(
        &self,
        query: &WebhookEndpointListQuery,
    ) -> Result<Vec<StoredWebhookEndpoint>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = query;
            Ok(Vec::new())
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_reader {
                Some(repository) => repository.list_webhook_endpoints(query).await,
                None => Ok(Vec::new()),
            }
        }
    }

    pub(crate) async fn find_webhook_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<Option<StoredWebhookEndpoint>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = endpoint_id;
            Ok(None)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_reader {
                Some(repository) => repository.find_webhook_endpoint(endpoint_id).await,
                None => Ok(None),
            }
        }
    }

    pub(crate) async fn upsert_webhook_endpoint(
        &self,
        record: &UpsertWebhookEndpointRecord,
    ) -> Result<Option<StoredWebhookEndpoint>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = record;
            Ok(None)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_writer {
                Some(repository) => repository.upsert_webhook_endpoint(record).await.map(Some),
                None => Ok(None),
            }
        }
    }

    pub(crate) async fn delete_webhook_endpoint(
        &self,
        endpoint_id: &str,
    ) -> Result<bool, DataLayerError> {
        #[cfg(test)]
        {
            let _ = endpoint_id;
            Ok(false)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_writer {
                Some(repository) => repository.delete_webhook_endpoint(endpoint_id).await,
                None => Ok(false),
            }
        }
    }

    pub(crate) async fn create_webhook_delivery(
        &self,
        record: &CreateWebhookDeliveryRecord,
    ) -> Result<Option<StoredWebhookDelivery>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = record;
            Ok(None)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_writer {
                Some(repository) => repository.create_webhook_delivery(record).await.map(Some),
                None => Ok(None),
            }
        }
    }

    pub(crate) async fn find_webhook_delivery(
        &self,
        delivery_id: &str,
    ) -> Result<Option<StoredWebhookDelivery>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = delivery_id;
            Ok(None)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_reader {
                Some(repository) => repository.find_webhook_delivery(delivery_id).await,
                None => Ok(None),
            }
        }
    }

    pub(crate) async fn list_webhook_deliveries(
        &self,
        query: &WebhookDeliveryListQuery,
    ) -> Result<StoredWebhookDeliveryPage, DataLayerError> {
        #[cfg(test)]
        {
            let _ = query;
            Ok(StoredWebhookDeliveryPage::default())
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_reader {
                Some(repository) => repository.list_webhook_deliveries(query).await,
                None => Ok(StoredWebhookDeliveryPage::default()),
            }
        }
    }

    pub(crate) async fn claim_due_webhook_deliveries(
        &self,
        request: &ClaimWebhookDeliveriesRequest,
    ) -> Result<Vec<StoredWebhookDelivery>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = request;
            Ok(Vec::new())
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_writer {
                Some(repository) => repository.claim_due_webhook_deliveries(request).await,
                None => Ok(Vec::new()),
            }
        }
    }

    pub(crate) async fn record_webhook_delivery_attempt(
        &self,
        record: &RecordWebhookDeliveryAttemptRecord,
    ) -> Result<Option<StoredWebhookDeliveryAttempt>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = record;
            Ok(None)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_writer {
                Some(repository) => repository
                    .record_webhook_delivery_attempt(record)
                    .await
                    .map(Some),
                None => Ok(None),
            }
        }
    }

    pub(crate) async fn reschedule_webhook_delivery(
        &self,
        record: &RescheduleWebhookDeliveryRecord,
    ) -> Result<bool, DataLayerError> {
        #[cfg(test)]
        {
            let _ = record;
            Ok(false)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_writer {
                Some(repository) => repository.reschedule_webhook_delivery(record).await,
                None => Ok(false),
            }
        }
    }

    pub(crate) async fn list_webhook_delivery_attempts(
        &self,
        query: &WebhookDeliveryAttemptListQuery,
    ) -> Result<Vec<StoredWebhookDeliveryAttempt>, DataLayerError> {
        #[cfg(test)]
        {
            let _ = query;
            Ok(Vec::new())
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_reader {
                Some(repository) => repository.list_webhook_delivery_attempts(query).await,
                None => Ok(Vec::new()),
            }
        }
    }

    pub(crate) async fn update_webhook_endpoint_test_result(
        &self,
        record: &UpdateWebhookEndpointTestResultRecord,
    ) -> Result<bool, DataLayerError> {
        #[cfg(test)]
        {
            let _ = record;
            Ok(false)
        }
        #[cfg(not(test))]
        {
            match &self.webhook_notification_writer {
                Some(repository) => repository.update_webhook_endpoint_test_result(record).await,
                None => Ok(false),
            }
        }
    }
}
