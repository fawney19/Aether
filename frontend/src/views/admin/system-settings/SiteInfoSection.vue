<template>
  <CardSection
    title="站点信息"
    description="自定义站点名称、开源标识和文档页，影响导航栏、登录页、指南页面和邮件等全站显示"
  >
    <template #actions>
      <Button
        size="sm"
        :disabled="loading || !hasChanges"
        @click="$emit('save')"
      >
        {{ loading ? '保存中...' : '保存' }}
      </Button>
    </template>
    <div class="grid grid-cols-1 md:grid-cols-2 gap-6">
      <div>
        <Label
          for="site-name"
          class="block text-sm font-medium"
        >
          站点名称
        </Label>
        <Input
          id="site-name"
          :model-value="siteName"
          type="text"
          placeholder="Aether"
          class="mt-1"
          @update:model-value="$emit('update:siteName', $event)"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          显示在导航栏、登录页标题和邮件中
        </p>
      </div>
      <div>
        <Label
          for="site-subtitle"
          class="block text-sm font-medium"
        >
          站点副标题
        </Label>
        <Input
          id="site-subtitle"
          :model-value="siteSubtitle"
          type="text"
          placeholder="AI Gateway"
          class="mt-1"
          @update:model-value="$emit('update:siteSubtitle', $event)"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          显示在导航栏品牌名称下方
        </p>
      </div>

      <div class="flex items-center">
        <div class="flex items-center space-x-2">
          <Checkbox
            id="show-github-link"
            :checked="showGithubLink"
            @update:checked="$emit('update:showGithubLink', $event)"
          />
          <div>
            <Label
              for="show-github-link"
              class="cursor-pointer"
            >
              显示 GitHub 图标
            </Label>
            <p class="text-xs text-muted-foreground">
              关闭后首页、控制台和文档页右上角不再显示开源仓库入口，适合包装为商业站
            </p>
          </div>
        </div>
      </div>

      <div>
        <Label
          for="guide-mode"
          class="block text-sm font-medium mb-2"
        >
          文档页
        </Label>
        <Select
          :model-value="guideMode"
          @update:model-value="$emit('update:guideMode', $event)"
        >
          <SelectTrigger id="guide-mode">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="builtin">
              使用内置文档
            </SelectItem>
            <SelectItem value="hidden">
              隐藏文档页
            </SelectItem>
            <SelectItem value="custom">
              使用自定义文档
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="mt-1 text-xs text-muted-foreground">
          隐藏后首页不再显示「文档」，直接访问 /guide 会回到首页
        </p>
      </div>
    </div>

    <div
      v-if="guideMode === 'custom'"
      class="mt-6 grid grid-cols-1 md:grid-cols-2 gap-6 border-t pt-6"
    >
      <div>
        <Label
          for="guide-custom-type"
          class="block text-sm font-medium mb-2"
        >
          自定义方式
        </Label>
        <Select
          :model-value="guideCustomType"
          @update:model-value="$emit('update:guideCustomType', $event)"
        >
          <SelectTrigger id="guide-custom-type">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="url">
              HTTP 链接
            </SelectItem>
            <SelectItem value="html">
              HTML 内容
            </SelectItem>
          </SelectContent>
        </Select>
        <p class="mt-1 text-xs text-muted-foreground">
          链接会嵌在 /guide 中；部分网站禁止 iframe 时可让用户新窗口打开
        </p>
      </div>

      <div v-if="guideCustomType === 'url'">
        <Label
          for="guide-url"
          class="block text-sm font-medium"
        >
          文档链接
        </Label>
        <Input
          id="guide-url"
          :model-value="guideUrl"
          type="url"
          placeholder="https://docs.example.com"
          class="mt-1"
          @update:model-value="$emit('update:guideUrl', $event)"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          必须是 http:// 或 https:// 地址
        </p>
      </div>

      <div
        v-else
        class="md:col-span-2"
      >
        <div class="flex items-center justify-between gap-3">
          <Label
            for="guide-html"
            class="block text-sm font-medium"
          >
            HTML 内容
          </Label>
          <label class="text-xs text-primary cursor-pointer hover:underline">
            上传 HTML 文件
            <input
              type="file"
              accept=".html,.htm,text/html"
              class="hidden"
              @change="onHtmlFileChange"
            >
          </label>
        </div>
        <Textarea
          id="guide-html"
          :model-value="guideHtml"
          class="mt-1 min-h-[180px] font-mono text-xs"
          placeholder="<h1>使用说明</h1>"
          @update:model-value="$emit('update:guideHtml', $event)"
        />
        <p class="mt-1 text-xs text-muted-foreground">
          最大 512KB，将在沙箱中展示。不要粘贴不可信的脚本。
        </p>
      </div>
    </div>
  </CardSection>
</template>

<script setup lang="ts">
import Button from '@/components/ui/button.vue'
import Input from '@/components/ui/input.vue'
import Label from '@/components/ui/label.vue'
import Textarea from '@/components/ui/textarea.vue'
import Checkbox from '@/components/ui/checkbox.vue'
import Select from '@/components/ui/select.vue'
import SelectTrigger from '@/components/ui/select-trigger.vue'
import SelectValue from '@/components/ui/select-value.vue'
import SelectContent from '@/components/ui/select-content.vue'
import SelectItem from '@/components/ui/select-item.vue'
import { CardSection } from '@/components/layout'
import type { GuideCustomType, GuideMode } from '@/composables/useSiteInfo'
import { useToast } from '@/composables/useToast'

const MAX_GUIDE_HTML_BYTES = 512 * 1024

const props = defineProps<{
  siteName: string
  siteSubtitle: string
  showGithubLink: boolean
  guideMode: GuideMode
  guideCustomType: GuideCustomType
  guideUrl: string
  guideHtml: string
  loading: boolean
  hasChanges: boolean
}>()

const emit = defineEmits<{
  save: []
  'update:siteName': [value: string]
  'update:siteSubtitle': [value: string]
  'update:showGithubLink': [value: boolean]
  'update:guideMode': [value: GuideMode]
  'update:guideCustomType': [value: GuideCustomType]
  'update:guideUrl': [value: string]
  'update:guideHtml': [value: string]
}>()

void props

const { error } = useToast()

function onHtmlFileChange(event: Event) {
  const input = event.target as HTMLInputElement
  const file = input.files?.[0]
  input.value = ''
  if (!file) return
  if (file.size > MAX_GUIDE_HTML_BYTES) {
    error('HTML 文件不能超过 512KB')
    return
  }
  const reader = new FileReader()
  reader.onload = () => {
    if (typeof reader.result === 'string') {
      emit('update:guideHtml', reader.result)
    }
  }
  reader.readAsText(file)
}
</script>
