<template>
  <ServerUserSelector
    :model-value="model || '__all__'"
    :placeholder="label"
    :label="label"
    :initial-users="selectedUsers"
    trigger-class="h-9 rounded-md px-3 text-sm shadow-none"
    dropdown
    @update:model-value="model = $event === '__all__' ? '' : $event"
  />
</template>

<script setup lang="ts">
import ServerUserSelector from '@/features/usage/components/ServerUserSelector.vue'
import { ref, watch } from 'vue'
import { usersApi } from '@/api/users'
defineProps<{ label: string }>()
const model = defineModel<string>({ required: true })
const selectedUsers = ref<Array<{ id: string; username: string; email: string }>>([])
watch(model, async id => {
  if (!id || selectedUsers.value.some(user => user.id === id)) return
  try {
    const user = await usersApi.getUser(id)
    if (model.value === id) selectedUsers.value = [{ id: user.id, username: user.username, email: user.email || '' }]
  } catch {
    // 用户已删除或无权读取时仍保留所选 ID，避免悄悄放宽筛选。
  }
}, { immediate: true })
</script>
