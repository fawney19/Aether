<template>
  <div class="space-y-6 px-4 sm:px-6 lg:px-0 pb-8">
    <HealthMonitorCard
      title="健康监控"
      :is-admin="isAdminPage"
      :show-provider-info="isAdminPage"
    />
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { useRoute } from 'vue-router'
import { useAuthStore } from '@/stores/auth'
import HealthMonitorCard from '@/features/providers/components/HealthMonitorCard.vue'

const route = useRoute()
const authStore = useAuthStore()
const isAdminPage = computed(() => authStore.isAdmin || route.path.startsWith('/admin'))
</script>
