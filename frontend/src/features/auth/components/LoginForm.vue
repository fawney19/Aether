<template>
  <div class="px-6 py-6 sm:px-8 sm:py-8">
    <div class="flex flex-col items-center text-center mb-8">
      <img
        src="/aether_adaptive.svg"
        :alt="siteName"
        class="h-16 w-16 mb-4"
      >
      <h2 class="text-2xl font-semibold text-foreground">
        登录到 {{ siteName }}
      </h2>
    </div>

    <div
      v-if="isDemo"
      class="rounded-lg border border-primary/20 bg-primary/5 p-3 mb-5"
    >
      <p class="text-xs font-medium text-foreground mb-2">
        演示模式
      </p>
      <div class="space-y-1.5">
        <button
          type="button"
          class="flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground transition-colors w-full"
          @click="emit('fill-demo-account', 'admin')"
        >
          <span class="inline-flex items-center justify-center w-4 h-4 rounded bg-primary/20 text-primary text-[10px] font-bold">A</span>
          <span>admin@demo.aether.io / demo123</span>
        </button>
        <button
          type="button"
          class="flex items-center gap-2 text-xs text-muted-foreground hover:text-foreground transition-colors w-full"
          @click="emit('fill-demo-account', 'user')"
        >
          <span class="inline-flex items-center justify-center w-4 h-4 rounded bg-muted text-muted-foreground text-[10px] font-bold">U</span>
          <span>user@demo.aether.io / demo123</span>
        </button>
      </div>
    </div>

    <div
      v-if="oauthProviders.length > 0"
      class="mb-5"
    >
      <div
        v-if="oauthProviders.length === 1"
        class="space-y-2"
      >
        <button
          type="button"
          class="oauth-btn"
          @click="emit('oauth-login', oauthProviders[0].provider_type)"
        >
          <!-- eslint-disable vue/no-v-html -->
          <span
            class="oauth-icon"
            v-html="getOAuthIcon(oauthProviders[0].provider_type)"
          />
          <!-- eslint-enable vue/no-v-html -->
          <span>使用 {{ oauthProviders[0].display_name }} 登录</span>
        </button>
      </div>

      <div
        v-else
        class="flex flex-col items-center gap-3"
      >
        <span class="text-xs text-muted-foreground">使用以下方式登录</span>
        <div class="flex items-center justify-center gap-3">
          <button
            v-for="p in oauthProviders"
            :key="p.provider_type"
            type="button"
            class="oauth-icon-btn"
            :title="p.display_name"
            @click="emit('oauth-login', p.provider_type)"
          >
            <!-- eslint-disable vue/no-v-html -->
            <span
              class="oauth-icon-lg"
              v-html="getOAuthIcon(p.provider_type)"
            />
            <!-- eslint-enable vue/no-v-html -->
          </button>
        </div>
      </div>
    </div>

    <div
      v-if="oauthProviders.length > 0"
      class="flex items-center gap-3 mb-5"
    >
      <div class="flex-1 h-px bg-border" />
      <span class="text-xs text-muted-foreground px-2">或使用账号密码</span>
      <div class="flex-1 h-px bg-border" />
    </div>

    <div
      v-if="showAuthTypeTabs"
      class="auth-type-tabs mb-4"
    >
      <button
        type="button"
        class="auth-tab"
        :class="[authTypeModel === 'local' && 'active']"
        @click="authTypeModel = 'local'"
      >
        本地登录
      </button>
      <button
        type="button"
        class="auth-tab"
        :class="[authTypeModel === 'ldap' && 'active']"
        @click="authTypeModel = 'ldap'"
      >
        LDAP 登录
      </button>
    </div>

    <form
      class="space-y-4"
      @submit.prevent="emit('submit')"
    >
      <div class="space-y-1.5">
        <div class="flex items-center justify-between">
          <Label
            for="login-email"
            class="text-sm"
          >
            {{ emailLabel }}
          </Label>
          <button
            v-if="ldapExclusive && authTypeModel === 'ldap'"
            type="button"
            class="text-xs text-muted-foreground/60 hover:text-muted-foreground transition-colors"
            @click="authTypeModel = 'local'"
          >
            管理员本地登录
          </button>
          <button
            v-if="ldapExclusive && authTypeModel === 'local'"
            type="button"
            class="text-xs text-muted-foreground/60 hover:text-muted-foreground transition-colors"
            @click="authTypeModel = 'ldap'"
          >
            返回 LDAP 登录
          </button>
        </div>
        <Input
          id="login-email"
          v-model="emailModel"
          type="text"
          required
          placeholder="用户名或邮箱"
          autocomplete="off"
        />
      </div>

      <div class="space-y-1.5">
        <Label
          for="login-password"
          class="text-sm"
        >
          密码
        </Label>
        <Input
          id="login-password"
          v-model="passwordModel"
          type="password"
          required
          placeholder="输入密码"
          autocomplete="off"
        />
      </div>

      <Button
        type="submit"
        :disabled="loading"
        class="w-full h-12"
      >
        {{ loading ? '登录中...' : '登录' }}
      </Button>

      <p
        v-if="!isDemo && !allowRegistration"
        class="text-xs text-muted-foreground text-center"
      >
        如需开通账户，请联系管理员
      </p>
    </form>

    <div
      v-if="allowRegistration"
      class="mt-5 pt-5 border-t border-border text-center text-sm text-muted-foreground"
    >
      还没有账户？
      <button
        type="button"
        class="text-primary hover:text-primary/80 font-medium transition-colors"
        @click="emit('switch-to-register')"
      >
        立即注册
      </button>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { OAuthProviderInfo } from '@/api/oauth'
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import { getOAuthIcon } from '@/utils/oauth-icons'

interface Props {
  siteName: string
  isDemo: boolean
  allowRegistration: boolean
  oauthProviders: OAuthProviderInfo[]
  showAuthTypeTabs: boolean
  authType: 'local' | 'ldap'
  ldapExclusive: boolean
  emailLabel: string
  loading: boolean
  formEmail: string
  formPassword: string
}

const props = defineProps<Props>()

const emit = defineEmits<{
  'update:authType': [value: 'local' | 'ldap']
  'update:formEmail': [value: string]
  'update:formPassword': [value: string]
  'fill-demo-account': [type: 'admin' | 'user']
  'submit': []
  'switch-to-register': []
  'oauth-login': [providerType: string]
}>()

const authTypeModel = computed({
  get: () => props.authType,
  set: (value: 'local' | 'ldap') => emit('update:authType', value)
})

const emailModel = computed({
  get: () => props.formEmail,
  set: (value: string) => emit('update:formEmail', value)
})

const passwordModel = computed({
  get: () => props.formPassword,
  set: (value: string) => emit('update:formPassword', value)
})
</script>

<style scoped>
.oauth-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 0.75rem;
  width: 100%;
  padding: 0.625rem 1rem;
  font-size: 0.875rem;
  font-weight: 500;
  color: hsl(var(--foreground));
  background: hsl(var(--muted) / 0.5);
  border: 1px solid hsl(var(--border) / 0.6);
  border-radius: 0.75rem;
  cursor: pointer;
  transition: all 0.15s ease;
}

.oauth-btn:hover {
  background: hsl(var(--muted));
  border-color: hsl(var(--primary) / 0.5);
}

.oauth-icon {
  width: 1.25rem;
  height: 1.25rem;
  flex-shrink: 0;
}

.oauth-icon :deep(svg) {
  width: 100%;
  height: 100%;
}

.oauth-icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 3rem;
  height: 3rem;
  background: hsl(var(--muted) / 0.5);
  border: 1px solid hsl(var(--border) / 0.6);
  border-radius: 0.75rem;
  cursor: pointer;
  transition: all 0.15s ease;
}

.oauth-icon-btn:hover {
  background: hsl(var(--muted));
  border-color: hsl(var(--primary) / 0.5);
  transform: translateY(-1px);
}

.oauth-icon-lg {
  width: 1.5rem;
  height: 1.5rem;
}

.oauth-icon-lg :deep(svg) {
  width: 100%;
  height: 100%;
}

.auth-type-tabs {
  display: flex;
  border-bottom: 1px solid hsl(var(--border));
}

.auth-tab {
  flex: 1;
  padding: 0.5rem 1rem;
  font-size: 0.875rem;
  font-weight: 500;
  color: hsl(var(--muted-foreground));
  background: transparent;
  border: none;
  cursor: pointer;
  transition: color 0.15s ease;
  position: relative;
}

.auth-tab::after {
  content: '';
  position: absolute;
  bottom: -1px;
  left: 0;
  right: 0;
  height: 2px;
  background: transparent;
  transition: background 0.15s ease;
}

.auth-tab:hover:not(.active) {
  color: hsl(var(--foreground));
}

.auth-tab.active {
  color: hsl(var(--primary));
  font-weight: 600;
}

.auth-tab.active::after {
  background: hsl(var(--primary));
}
</style>
