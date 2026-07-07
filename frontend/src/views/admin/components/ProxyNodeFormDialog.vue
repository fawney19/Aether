<template>
  <Dialog
    :model-value="open"
    :title="editingNode ? legacyT('编辑代理节点') : legacyT('添加代理节点')"
    :description="editingNode ? legacyT('修改手动代理节点的配置') : legacyT('推荐使用一键脚本部署 aether-tunnel，也可手动或批量添加已有 HTTP/SOCKS 代理')"
    :icon="editingNode ? SquarePen : Plus"
    size="lg"
    @update:model-value="$emit('update:open', $event)"
  >
    <div
      v-if="!editingNode"
      class="mb-4 grid grid-cols-1 sm:grid-cols-3 gap-2 rounded-lg border border-border/60 bg-muted/30 p-1"
    >
      <Button
        type="button"
        :variant="addMode === 'script' ? 'default' : 'ghost'"
        class="h-9"
        @click="$emit('update:addMode', 'script')"
      >
        <Terminal class="w-3.5 h-3.5 mr-1.5" />
        {{ legacyT('脚本自动添加') }}
      </Button>
      <Button
        type="button"
        :variant="addMode === 'manual' ? 'default' : 'ghost'"
        class="h-9"
        @click="$emit('update:addMode', 'manual')"
      >
        <Plus class="w-3.5 h-3.5 mr-1.5" />
        {{ legacyT('手动添加') }}
      </Button>
      <Button
        type="button"
        :variant="addMode === 'batch' ? 'default' : 'ghost'"
        class="h-9"
        @click="$emit('update:addMode', 'batch')"
      >
        <ListPlus class="w-3.5 h-3.5 mr-1.5" />
        {{ legacyT('批量添加') }}
      </Button>
    </div>

    <div
      v-if="!editingNode && addMode === 'script'"
      class="space-y-4"
    >
      <div class="rounded-lg border border-border/60 bg-muted/30 p-3 text-xs text-muted-foreground">
        {{ legacyT('输入节点名称后生成一次性安装命令，有效期 15 分钟。复制的命令不含敏感授权信息，只能在目标机器使用一次，避免 Token 暴露在页面或聊天记录中。') }}
      </div>

      <div class="space-y-1.5">
        <Label>{{ legacyT('节点名称 *') }}</Label>
        <Input
          v-model="installNodeName"
          :placeholder="legacyT('例如: jp-proxy-01')"
          @keyup.enter="$emit('refresh-install-command')"
        />
      </div>

      <div class="space-y-2">
        <Label class="text-sm font-semibold">{{ legacyT('目标系统') }}</Label>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-2">
          <Button
            type="button"
            :variant="installSystem === 'unix' ? 'default' : 'outline'"
            class="justify-start h-auto py-3"
            @click="$emit('update:installSystem', 'unix')"
          >
            macOS / Linux
          </Button>
          <Button
            type="button"
            :variant="installSystem === 'windows' ? 'default' : 'outline'"
            class="justify-start h-auto py-3"
            @click="$emit('update:installSystem', 'windows')"
          >
            Windows PowerShell
          </Button>
        </div>
      </div>

      <div class="space-y-2">
        <div class="flex items-center justify-between gap-2">
          <Label class="text-sm font-semibold">{{ legacyT('复制到代理节点机器执行') }}</Label>
          <div class="flex items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              class="gap-1.5"
              :disabled="installLoading || !proxyInstallCommand"
              @click="$emit('copy-install-command')"
            >
              <CheckCircle
                v-if="installCopied"
                class="h-3.5 w-3.5 text-emerald-600 dark:text-emerald-400"
              />
              <Copy
                v-else
                class="h-3.5 w-3.5"
              />
              {{ installCopied ? legacyT('已复制') : legacyT('一键复制') }}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              :disabled="installLoading || !installForm.node_name.trim()"
              @click="$emit('refresh-install-command')"
            >
              {{ legacyT(installLoading ? '生成中...' : proxyInstallCommand ? '重新生成' : '生成命令') }}
            </Button>
          </div>
        </div>
        <div class="rounded-lg border border-border/60 bg-background overflow-hidden">
          <pre class="max-h-32 overflow-x-auto whitespace-pre-wrap break-all p-3 text-xs font-mono">{{ proxyInstallCommand || legacyT('输入节点名称后点击“生成命令”') }}</pre>
        </div>
        <p class="text-xs text-muted-foreground">
          {{ legacyT(proxyInstallHint) }}
        </p>
      </div>
    </div>

    <div
      v-else-if="!editingNode && addMode === 'batch'"
      class="space-y-4"
    >
      <div class="rounded-lg border border-border/60 bg-muted/30 p-3 text-xs text-muted-foreground">
        {{ legacyT('支持一行一个，或使用英文逗号分隔。URL 中的用户名和密码会自动拆分到手动添加接口，节点名称自动使用主机和端口。') }}
      </div>

      <div class="space-y-1.5">
        <Label>{{ legacyT('代理地址 *') }}</Label>
        <Textarea
          v-model="batchContent"
          class="min-h-[180px] font-mono text-xs break-all !rounded-xl"
          placeholder="socks5://username:password@1.2.3.4:1080&#10;http://username:password@5.6.7.8:8080"
        />
      </div>

      <div
        v-if="batchParseResult.errors.length"
        class="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive"
      >
        <div class="font-medium">
          {{ legacyT(`有 ${batchParseResult.errors.length} 条格式错误`) }}
        </div>
        <ul class="mt-1 space-y-1">
          <li
            v-for="message in batchParseResult.errors.slice(0, 3)"
            :key="message"
          >
            {{ legacyT(message) }}
          </li>
        </ul>
        <div
          v-if="batchParseResult.errors.length > 3"
          class="mt-1 text-destructive/80"
        >
          {{ legacyT(`还有 ${batchParseResult.errors.length - 3} 条错误未显示`) }}
        </div>
      </div>
      <p
        v-else-if="batchForm.content.trim()"
        class="text-xs text-muted-foreground"
      >
        {{ legacyT(`已识别 ${batchParseResult.nodes.length} 个代理节点。`) }}
      </p>
    </div>

    <form
      v-else
      class="space-y-4"
      @submit.prevent="$emit('submit-manual')"
    >
      <div class="space-y-1.5">
        <Label>{{ legacyT('名称 *') }}</Label>
        <Input
          v-model="manualName"
          :placeholder="legacyT('例如: 美西 VPN 代理')"
        />
      </div>
      <div class="space-y-1.5">
        <Label>{{ legacyT('代理地址 *') }}</Label>
        <Input
          v-model="manualProxyUrl"
          :placeholder="legacyT('http://proxy:port 或 socks5://proxy:port')"
        />
      </div>
      <div class="grid grid-cols-2 gap-3">
        <div class="space-y-1.5">
          <Label>{{ legacyT('用户名') }}</Label>
          <Input
            v-model="manualUsername"
            :placeholder="legacyT('可选')"
            autocomplete="off"
            data-form-type="other"
            data-lpignore="true"
            data-1p-ignore="true"
          />
        </div>
        <div class="space-y-1.5">
          <Label>{{ legacyT('密码') }}</Label>
          <Input
            v-model="manualPassword"
            type="text"
            masked
            :placeholder="legacyT('可选')"
            autocomplete="new-password"
            data-form-type="other"
            data-lpignore="true"
            data-1p-ignore="true"
          />
        </div>
      </div>
      <div class="space-y-1.5">
        <Label>{{ legacyT('区域') }}</Label>
        <Input
          v-model="manualRegion"
          :placeholder="legacyT('可选，例如: US-West')"
        />
      </div>
    </form>

    <template #footer>
      <div
        v-if="!editingNode && addMode === 'script'"
        class="flex items-center justify-end gap-2 w-full"
      >
        <Button
          variant="outline"
          @click="$emit('update:open', false)"
        >
          {{ legacyT('关闭') }}
        </Button>
        <Button
          :disabled="installLoading || !proxyInstallCommand"
          @click="$emit('copy-install-command')"
        >
          {{ installCopied ? legacyT('已复制') : legacyT('复制命令') }}
        </Button>
      </div>
      <div
        v-else-if="!editingNode && addMode === 'batch'"
        class="flex items-center justify-between gap-3 w-full"
      >
        <span class="text-xs text-muted-foreground">
          {{ batchForm.content.trim() ? legacyT(`待添加 ${batchParseResult.nodes.length} 个`) : legacyT('等待输入代理地址') }}
        </span>
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            @click="$emit('update:open', false)"
          >
            {{ legacyT('取消') }}
          </Button>
          <Button
            :disabled="addingNode || !batchForm.content.trim() || batchParseResult.errors.length > 0 || batchParseResult.nodes.length === 0"
            @click="$emit('submit-batch')"
          >
            {{ addingNode ? legacyT('添加中...') : legacyT('批量添加') }}
          </Button>
        </div>
      </div>
      <div
        v-else
        class="flex items-center justify-between w-full"
      >
        <Button
          variant="outline"
          :disabled="testingUrl || !addForm.proxy_url"
          @click="$emit('test-url')"
        >
          {{ testingUrl ? legacyT('测试中...') : legacyT('测试') }}
        </Button>
        <div class="flex items-center gap-2">
          <Button
            variant="outline"
            @click="$emit('update:open', false)"
          >
            {{ legacyT('取消') }}
          </Button>
          <Button
            :disabled="addingNode || !addForm.name || !addForm.proxy_url"
            @click="$emit('submit-manual')"
          >
            {{ submitLabel }}
          </Button>
        </div>
      </div>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { CheckCircle, Copy, ListPlus, Plus, SquarePen, Terminal } from 'lucide-vue-next'
import { Button, Dialog, Input, Label, Textarea } from '@/components/ui'
import type { ProxyNode } from '@/api/proxy-nodes'
import { useI18n } from '@/i18n'
import type { BatchProxyNodeParseResult } from '../proxy-node-batch'
import type {
  ProxyNodeAddMode,
  ProxyNodeBatchForm,
  ProxyNodeInstallForm,
  ProxyNodeInstallSystem,
  ProxyNodeManualForm,
} from './proxy-node-types'

const props = defineProps<{
  open: boolean
  editingNode: ProxyNode | null
  addMode: ProxyNodeAddMode
  addForm: ProxyNodeManualForm
  batchForm: ProxyNodeBatchForm
  installForm: ProxyNodeInstallForm
  installSystem: ProxyNodeInstallSystem
  installLoading: boolean
  installCopied: boolean
  proxyInstallCommand: string
  proxyInstallHint: string
  batchParseResult: BatchProxyNodeParseResult
  addingNode: boolean
  testingUrl: boolean
}>()

const emit = defineEmits<{
  'update:open': [value: boolean]
  'update:addMode': [value: ProxyNodeAddMode]
  'update:addForm': [value: ProxyNodeManualForm]
  'update:batchForm': [value: ProxyNodeBatchForm]
  'update:installForm': [value: ProxyNodeInstallForm]
  'update:installSystem': [value: ProxyNodeInstallSystem]
  'refresh-install-command': []
  'copy-install-command': []
  'submit-manual': []
  'submit-batch': []
  'test-url': []
}>()

const { legacyT } = useI18n()

const installNodeName = computed({
  get: () => props.installForm.node_name,
  set: (value: string) => emit('update:installForm', { ...props.installForm, node_name: value }),
})

const batchContent = computed({
  get: () => props.batchForm.content,
  set: (value: string) => emit('update:batchForm', { ...props.batchForm, content: value }),
})

const manualName = computed({
  get: () => props.addForm.name,
  set: (value: string) => emit('update:addForm', { ...props.addForm, name: value }),
})

const manualProxyUrl = computed({
  get: () => props.addForm.proxy_url,
  set: (value: string) => emit('update:addForm', { ...props.addForm, proxy_url: value }),
})

const manualUsername = computed({
  get: () => props.addForm.username,
  set: (value: string) => emit('update:addForm', { ...props.addForm, username: value }),
})

const manualPassword = computed({
  get: () => props.addForm.password,
  set: (value: string) => emit('update:addForm', { ...props.addForm, password: value }),
})

const manualRegion = computed({
  get: () => props.addForm.region,
  set: (value: string) => emit('update:addForm', { ...props.addForm, region: value }),
})

const submitLabel = computed(() => {
  if (props.addingNode) {
    return legacyT(props.editingNode ? '保存中...' : '添加中...')
  }
  return legacyT(props.editingNode ? '保存' : '添加')
})
</script>
