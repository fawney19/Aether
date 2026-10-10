import { onMounted, watch } from 'vue'
import { useModuleStore } from '@/stores/modules'

export function useNavigationModules(canAccessAdmin: () => boolean) {
  const moduleStore = useModuleStore()
  function loadModules(isAdmin: boolean) {
    if (isAdmin) {
      if (!moduleStore.loaded && !moduleStore.loading) {
        void moduleStore.fetchModules().catch(() => {
          // Store 记录错误，管理员路由守卫会在需要时重试。
        })
      }
    } else {
      // 每次进入用户布局都刷新公开状态，普通用户无需管理员权限。
      void moduleStore.fetchUserModules().catch(() => {
        // 加载失败时 Store 清除旧状态，菜单默认隐藏相关入口。
      })
    }
  }
  onMounted(() => loadModules(canAccessAdmin()))
  // 同一布局内跨标签页同步身份后，按新权限补齐对应状态。
  watch(canAccessAdmin, loadModules)
}
