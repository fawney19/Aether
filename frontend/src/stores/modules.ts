import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { modulesApi, type ModuleStatus } from '@/api/modules'
import { log } from '@/utils/logger'
import { parseApiError } from '@/utils/errorParser'

export const useModuleStore = defineStore('modules', () => {
  const modules = ref<Record<string, ModuleStatus>>({})
  const loaded = ref(false)
  const loading = ref(false)
  const error = ref<string | null>(null)
  let fetchModulesPromise: Promise<Record<string, ModuleStatus>> | null = null

  /**
   * 获取所有模块状态
   */
  async function fetchModules() {
    if (fetchModulesPromise) return fetchModulesPromise

    loading.value = true
    error.value = null

    fetchModulesPromise = (async () => {
      try {
        const nextModules = await modulesApi.getAllStatus()
        modules.value = nextModules
        loaded.value = true
        return nextModules
      } catch (err: unknown) {
        log.error('Failed to fetch modules status', err)
        error.value = parseApiError(err, '获取模块状态失败')
        throw err
      } finally {
        loading.value = false
        fetchModulesPromise = null
      }
    })()

    return fetchModulesPromise
  }

  /**
   * 检查模块是否部署可用
   */
  function isAvailable(moduleName: string): boolean {
    return modules.value[moduleName]?.available ?? false
  }

  /**
   * 检查模块是否运行启用
   */
  function isEnabled(moduleName: string): boolean {
    return modules.value[moduleName]?.enabled ?? false
  }

  /**
   * 检查模块是否最终激活
   */
  function isActive(moduleName: string): boolean {
    return modules.value[moduleName]?.active ?? false
  }

  /**
   * 设置模块启用状态
   * @throws 如果设置失败会抛出错误
   */
  async function setEnabled(moduleName: string, enabled: boolean) {
    try {
      await modulesApi.setEnabled(moduleName, enabled)
      // 刷新所有模块状态，确保依赖模块的 active 状态同步更新
      await fetchModules()
      return true
    } catch (err: unknown) {
      log.error(`Failed to set module ${moduleName} enabled=${enabled}`, err)
      error.value = parseApiError(err, '设置模块状态失败')
      // 重新抛出错误，让调用方可以获取详细错误信息
      throw err
    }
  }

  /**
   * 获取可用的管理菜单项（available 即显示）
   */
  const availableAdminMenuItems = computed(() => {
    return Object.values(modules.value)
      .filter((m) => m.available && m.admin_route)
      .sort((a, b) => a.admin_menu_order - b.admin_menu_order)
  })

  /**
   * 按分组获取可用的管理菜单项
   */
  const availableAdminMenuItemsByGroup = computed(() => {
    const items = availableAdminMenuItems.value
    const groups: Record<string, ModuleStatus[]> = {}

    for (const item of items) {
      const group = item.admin_menu_group || 'other'
      if (!groups[group]) {
        groups[group] = []
      }
      groups[group].push(item)
    }

    return groups
  })

  return {
    modules,
    loaded,
    loading,
    error,
    fetchModules,
    isAvailable,
    isEnabled,
    isActive,
    setEnabled,
    availableAdminMenuItems,
    availableAdminMenuItemsByGroup,
  }
})
