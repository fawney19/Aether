<template>
  <PageContainer>
    <PageHeader
      title="模块管理"
      description="管理系统功能模块的启用状态"
    >
      <template #actions>
        <Button
          variant="outline"
          :disabled="loading"
          @click="fetchModules"
        >
          <RefreshCw
            class="w-4 h-4 mr-2"
            :class="{ 'animate-spin': loading }"
          />
          刷新
        </Button>
      </template>
    </PageHeader>

    <!-- 搜索栏 -->
    <div class="mt-6 mb-6">
      <div class="relative">
        <Search class="absolute left-4 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
        <Input
          v-model="searchQuery"
          placeholder="搜索模块名称或描述..."
          class="pl-11 h-11"
        />
      </div>
    </div>

    <div>
      <!-- 内置工具 -->
      <div
        v-if="filteredBuiltinTools.length > 0"
        class="mb-8"
      >
        <h3 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider mb-4">
          内置工具
        </h3>
        <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-5">
          <div
            v-for="tool in filteredBuiltinTools"
            :key="tool.name"
            class="group relative border rounded-2xl p-6 transition-all duration-200 hover:shadow-lg border-border bg-card hover:border-primary/20 cursor-pointer"
            @click="router.push(tool.href)"
          >
            <div class="flex items-start gap-4 mb-3">
              <div class="w-11 h-11 rounded-xl flex items-center justify-center shrink-0 transition-colors bg-primary/15 text-primary">
                <component
                  :is="tool.icon"
                  class="w-5 h-5"
                />
              </div>
              <div class="flex-1 min-w-0 pt-1">
                <h4 class="font-semibold text-base truncate">
                  {{ tool.name }}
                </h4>
              </div>
            </div>
            <p class="text-sm text-muted-foreground leading-relaxed line-clamp-2 min-h-[2.5rem]">
              {{ tool.description }}
            </p>
            <div class="mt-5 pt-4 border-t border-border/50 flex items-center justify-end">
              <Button
                variant="outline"
                size="sm"
                class="gap-1.5"
                @click.stop="router.push(tool.href)"
              >
                <Settings class="w-3.5 h-3.5" />
                管理
              </Button>
            </div>
          </div>
        </div>
      </div>

      <!-- 扩展模块 -->
      <h3 class="text-sm font-semibold text-muted-foreground uppercase tracking-wider mb-4">
        扩展模块
      </h3>

      <!-- 模块卡片网格 -->
      <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-3 gap-5">
        <div
          v-for="module in filteredModules"
          :key="module.name"
          class="group relative border rounded-2xl p-6 transition-all duration-200 hover:shadow-lg"
          :class="{
            'bg-muted/40 border-muted': !module.available,
            'border-primary/40 bg-gradient-to-br from-primary/5 to-primary/10 shadow-sm': module.active,
            'border-border bg-card hover:border-primary/20': !module.active && module.available
          }"
        >
          <!-- 状态指示器 -->
          <div class="absolute top-5 right-5">
            <div
              class="w-2.5 h-2.5 rounded-full ring-2 ring-offset-2 ring-offset-background"
              :class="{
                'bg-green-500 ring-green-500/30': module.active,
                'bg-amber-500 ring-amber-500/30': module.available && module.enabled && !module.active,
                'bg-gray-300 ring-gray-300/30': module.available && !module.enabled,
                'bg-red-400 ring-red-400/30': !module.available
              }"
            />
          </div>

          <!-- 模块图标和名称 -->
          <div class="flex items-start gap-4 mb-3">
            <div
              class="w-11 h-11 rounded-xl flex items-center justify-center shrink-0 transition-colors"
              :class="module.active
                ? 'bg-primary/15 text-primary'
                : 'bg-muted text-muted-foreground group-hover:bg-muted/80'"
            >
              <component
                :is="getCategoryIcon(module.category)"
                class="w-5 h-5"
              />
            </div>
            <div class="flex-1 min-w-0 pt-1">
              <h4 class="font-semibold text-base truncate">
                {{ module.display_name }}
              </h4>
            </div>
          </div>

          <!-- 描述 -->
          <p class="text-sm text-muted-foreground leading-relaxed line-clamp-2 min-h-[2.5rem]">
            {{ module.description }}
          </p>

          <p
            v-if="module.name === 'chat_pii_redaction'"
            class="mt-3 rounded-lg border border-border bg-muted/40 px-3 py-2 text-xs text-muted-foreground"
          >
            {{ getModuleStatusCopy(module) }}
          </p>

          <!-- 不可用提示 -->
          <div
            v-if="!module.available"
            class="mt-4 text-xs text-orange-700 dark:text-orange-400 bg-orange-100 dark:bg-orange-950/50 rounded-lg px-3 py-2"
          >
            模块不可用，请检查环境变量和依赖库
          </div>

          <!-- 操作区域 -->
          <div class="mt-5 pt-4 border-t border-border/50 flex items-center justify-between">
            <div class="flex items-center gap-3">
              <Switch
                :model-value="module.enabled"
                :disabled="!module.available || !module.config_validated || toggling[module.name]"
                @update:model-value="(val: boolean) => toggleModule(module.name, val)"
              />
              <div class="flex flex-col">
                <span
                  class="text-sm"
                  :class="module.enabled ? 'text-foreground' : 'text-muted-foreground'"
                >
                  {{ module.enabled ? '启用' : '禁用' }}
                </span>
                <!-- 配置未验证提示（小字） -->
                <span
                  v-if="module.available && !module.config_validated"
                  class="text-xs text-muted-foreground"
                >
                  {{ module.config_error || '请先完成配置' }}
                </span>
              </div>
            </div>
            <Button
              v-if="module.admin_route"
              variant="outline"
              size="sm"
              class="gap-1.5"
              @click="router.push(module.admin_route)"
            >
              <Settings class="w-3.5 h-3.5" />
              配置
            </Button>
          </div>
        </div>
      </div>

      <!-- 搜索无结果 -->
      <div
        v-if="filteredModules.length === 0 && filteredBuiltinTools.length === 0 && searchQuery && !loading"
        class="text-center py-16"
      >
        <Search class="w-12 h-12 mx-auto text-muted-foreground/50 mb-4" />
        <p class="text-muted-foreground">
          没有找到匹配的模块
        </p>
      </div>

      <!-- 空状态 -->
      <div
        v-if="allModules.length === 0 && !loading"
        class="text-center py-16"
      >
        <Puzzle class="w-12 h-12 mx-auto text-muted-foreground/50 mb-4" />
        <p class="text-muted-foreground">
          暂无可管理的模块
        </p>
      </div>
    </div>
  </PageContainer>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { RefreshCw, Puzzle, Users, Shield, Gauge, Link, Search, Settings } from 'lucide-vue-next'
import Button from '@/components/ui/button.vue'
import Switch from '@/components/ui/switch.vue'
import Input from '@/components/ui/input.vue'
import { PageHeader, PageContainer } from '@/components/layout'
import { useToast } from '@/composables/useToast'
import { useModuleStore } from '@/stores/modules'
import { BUILTIN_TOOLS } from '@/config/builtin-tools'
import { log } from '@/utils/logger'
import { getErrorMessage } from '@/types/api-error'

const router = useRouter()
const { success, error } = useToast()
const moduleStore = useModuleStore()

const loading = ref(false)
const toggling = ref<Record<string, boolean>>({})
const searchQuery = ref('')

// 过滤后的内置工具
const filteredBuiltinTools = computed(() => {
  if (!searchQuery.value.trim()) return BUILTIN_TOOLS
  const query = searchQuery.value.toLowerCase()
  return BUILTIN_TOOLS.filter(
    t => t.name.toLowerCase().includes(query) || t.description.toLowerCase().includes(query)
  )
})

// 获取分类图标
function getCategoryIcon(category: string) {
  const icons: Record<string, unknown> = {
    auth: Users,
    monitoring: Gauge,
    security: Shield,
    integration: Link,
  }
  return icons[category] || Puzzle
}

function getModuleStatusCopy(module: { name: string; enabled: boolean; active: boolean; config_validated: boolean; config_error: string | null }) {
  if (module.name !== 'chat_pii_redaction') {
    return module.enabled ? '已启用' : '已禁用'
  }
  if (!module.config_validated) return '配置异常，替换保护未生效'
  if (!module.enabled) return '全局替换开关未开启，所有供应商均不会执行替换保护'
  return '全局替换开关已开启，可在供应商中单独启用'
}

// 所有模块列表（按 admin_menu_order 排序）
const allModules = computed(() => {
  return Object.values(moduleStore.modules)
    .sort((a, b) => a.admin_menu_order - b.admin_menu_order)
})

// 过滤后的模块列表
const filteredModules = computed(() => {
  if (!searchQuery.value.trim()) {
    return allModules.value
  }
  const query = searchQuery.value.toLowerCase()
  return allModules.value.filter(
    (m) =>
      m.name.toLowerCase().includes(query) ||
      m.display_name.toLowerCase().includes(query) ||
      m.description.toLowerCase().includes(query)
  )
})

// 获取模块列表
async function fetchModules() {
  loading.value = true
  try {
    await moduleStore.fetchModules()
  } catch (err) {
    error('获取模块列表失败')
    log.error('获取模块列表失败:', err)
  } finally {
    loading.value = false
  }
}

// 切换模块启用状态
async function toggleModule(moduleName: string, enabled: boolean) {
  toggling.value[moduleName] = true
  try {
    await moduleStore.setEnabled(moduleName, enabled)
    success(enabled ? '模块已启用' : '模块已禁用')
  } catch (err) {
    error(getErrorMessage(err, '操作失败'))
    log.error('切换模块状态失败:', err)
  } finally {
    toggling.value[moduleName] = false
  }
}

onMounted(() => {
  fetchModules()
})
</script>
