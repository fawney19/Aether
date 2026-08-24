<template>
  <div
    ref="rootRef"
    :class="dropdown ? 'relative' : ''"
  >
    <button
      v-if="dropdown"
      type="button"
      class="flex h-8 w-full min-w-0 cursor-pointer items-center justify-between gap-2 rounded-2xl border border-border/60 bg-card/80 px-4 py-2 text-left text-xs text-foreground shadow-sm backdrop-blur transition-all focus:border-primary/60 focus:outline-none focus:ring-2 focus:ring-primary/40"
      @click="toggleOpen"
    >
      <span class="truncate">{{ selectedLabel }}</span>
      <ChevronDown class="h-4 w-4 shrink-0 text-muted-foreground opacity-50" />
    </button>

    <div
      v-if="!dropdown || open"
      :class="dropdown ? 'absolute left-0 top-full z-50 mt-1 w-64 rounded-md border border-border bg-popover p-1 text-popover-foreground shadow-lg' : ''"
    >
      <div class="relative mb-1">
        <Search class="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          v-model="searchText"
          class="h-8 pl-8 text-xs"
          placeholder="搜索用户"
          @keydown.stop
        />
      </div>

      <div class="max-h-64 overflow-y-auto pr-0.5">
        <button
          v-if="allowAll"
          type="button"
          class="relative flex w-full items-center rounded-lg py-1.5 pl-8 pr-2 text-left text-sm transition-colors hover:bg-accent focus:bg-accent"
          @click="selectUser('__all__')"
        >
          <Check
            class="absolute left-2 h-4 w-4"
            :class="modelValue === '__all__' ? 'opacity-100' : 'opacity-0'"
          />
          <span>全部用户</span>
        </button>
        <button
          v-if="noneValue"
          type="button"
          class="relative flex w-full items-center rounded-lg py-1.5 pl-8 pr-2 text-left text-sm transition-colors hover:bg-accent focus:bg-accent"
          @click="selectUser(noneValue)"
        >
          <Check
            class="absolute left-2 h-4 w-4"
            :class="modelValue === noneValue ? 'opacity-100' : 'opacity-0'"
          />
          <span>{{ noneLabel }}</span>
        </button>

        <div
          v-if="pinnedUser"
          class="my-1 border-t border-border/60 pt-1"
        >
          <button
            type="button"
            class="relative flex w-full items-center rounded-lg py-1.5 pl-8 pr-2 text-left text-sm transition-colors hover:bg-accent focus:bg-accent"
            @click="selectUser(pinnedUser.id)"
          >
            <Check class="absolute left-2 h-4 w-4 opacity-100" />
            <span class="min-w-0">
              <span class="block truncate">{{ getUserLabel(pinnedUser) }}</span>
              <span
                v-if="pinnedUser.email && pinnedUser.email !== pinnedUser.username"
                class="block truncate text-xs text-muted-foreground"
              >{{ pinnedUser.email }}</span>
            </span>
          </button>
        </div>

        <div
          v-if="loading"
          class="px-3 py-6 text-center text-xs text-muted-foreground"
        >
          加载中...
        </div>
        <div
          v-else-if="visibleUsers.length === 0"
          class="px-3 py-6 text-center text-xs text-muted-foreground"
        >
          未找到用户
        </div>
        <button
          v-for="user in visibleUsers"
          v-else
          :key="user.id"
          type="button"
          class="relative flex w-full items-center rounded-lg py-1.5 pl-8 pr-2 text-left text-sm transition-colors hover:bg-accent focus:bg-accent"
          @click="selectUser(user.id)"
        >
          <Check
            class="absolute left-2 h-4 w-4"
            :class="modelValue === user.id ? 'opacity-100' : 'opacity-0'"
          />
          <span class="min-w-0">
            <span class="block truncate">{{ getUserLabel(user) }}</span>
            <span
              v-if="user.email && user.email !== user.username"
              class="block truncate text-xs text-muted-foreground"
            >{{ user.email }}</span>
          </span>
        </button>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { useDebounceFn } from '@vueuse/core'
import { Check, ChevronDown, Search } from 'lucide-vue-next'

import { Input } from '@/components/ui'
import { usersApi } from '@/api/users'
import type { UserOption } from './UsageRecordsTable.vue'

const props = withDefaults(defineProps<{
  modelValue: string
  initialUsers?: UserOption[]
  dropdown?: boolean
  allowAll?: boolean
  noneValue?: string | null
  noneLabel?: string
}>(), {
  initialUsers: () => [],
  dropdown: false,
  allowAll: true,
  noneValue: null,
  noneLabel: '不对比',
})

const emit = defineEmits<{
  'update:modelValue': [value: string]
  select: [value: string]
}>()

const rootRef = ref<HTMLElement | null>(null)
const open = ref(false)
const loading = ref(false)
const users = ref<UserOption[]>([])
const knownUsers = ref(new Map<string, UserOption>())
const searchText = ref('')
let requestId = 0
let loadedInitialBatch = false

const selectedUser = computed(() => knownUsers.value.get(props.modelValue))
const selectedLabel = computed(() => {
  if (props.noneValue && props.modelValue === props.noneValue) return props.noneLabel
  if (props.allowAll && props.modelValue === '__all__') return '全部用户'
  const user = selectedUser.value
  return user ? getUserLabel(user) : (props.modelValue ? `User ${props.modelValue}` : '选择用户')
})
const pinnedUser = computed(() => {
  if (props.modelValue === '__all__') return null
  const selected = selectedUser.value
  if (!selected) return null
  return users.value.some((user) => user.id === selected.id) ? null : selected
})
const visibleUsers = computed(() => {
  if (!pinnedUser.value) return users.value
  return users.value.filter((user) => user.id !== pinnedUser.value?.id)
})

watch(() => props.initialUsers, (nextUsers) => {
  rememberUsers(nextUsers)
  if (users.value.length === 0 && nextUsers.length > 0) {
    users.value = [...nextUsers]
  }
}, { immediate: true })

watch(searchText, useDebounceFn(() => {
  void loadUsers(searchText.value)
}, 300))

watch(open, (isOpen) => {
  if (isOpen && !loadedInitialBatch && !loading.value) {
    void loadUsers('')
  }
})

function getUserLabel(user: UserOption): string {
  return user.username || user.email || user.id
}

function rememberUsers(nextUsers: UserOption[]) {
  const nextMap = new Map(knownUsers.value)
  for (const user of nextUsers) {
    nextMap.set(user.id, user)
  }
  knownUsers.value = nextMap
}

async function loadUsers(search: string) {
  const currentRequest = ++requestId
  loading.value = true
  try {
    const result = await usersApi.getAllUsers({
      search,
      skip: 0,
      limit: 50,
      cacheTtlMs: search.trim() ? 0 : 30_000,
    })
    if (currentRequest !== requestId) return
    const options = result.map((user) => ({
      id: user.id,
      username: user.username,
      email: user.email,
    }))
    if (!search.trim()) loadedInitialBatch = true
    users.value = options
    rememberUsers(options)
  } catch {
    if (currentRequest === requestId) users.value = []
  } finally {
    if (currentRequest === requestId) loading.value = false
  }
}

function selectUser(value: string) {
  emit('update:modelValue', value)
  emit('select', value)
  if (props.dropdown) open.value = false
}

function toggleOpen() {
  open.value = !open.value
}

function handleDocumentPointerDown(event: PointerEvent) {
  if (!props.dropdown || !open.value) return
  const target = event.target
  if (target instanceof Node && rootRef.value?.contains(target)) return
  open.value = false
}

onMounted(() => {
  if (!props.dropdown && !loadedInitialBatch) {
    void loadUsers('')
  }
  document.addEventListener('pointerdown', handleDocumentPointerDown)
})

onBeforeUnmount(() => {
  document.removeEventListener('pointerdown', handleDocumentPointerDown)
})
</script>
