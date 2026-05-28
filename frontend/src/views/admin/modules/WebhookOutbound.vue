<template>
  <PageContainer>
    <PageHeader
      title="Webhook 出站"
      description="把用户、计费、风控和上游异常事件可靠投递到外部系统。"
      :icon="Webhook"
    />

    <div class="mt-6">
      <WebhookOutboundSection />
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { Webhook } from 'lucide-vue-next'
import { PageContainer, PageHeader } from '@/components/layout'
import WebhookOutboundSection from './WebhookOutboundSection.vue'
</script>
