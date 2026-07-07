import type { RouteRecordRaw } from 'vue-router'
import { adminRoutes } from './admin'
import { dashboardRoutes } from './dashboard'
import { publicRoutes } from './public'

export const routes: RouteRecordRaw[] = [
  ...publicRoutes,
  ...dashboardRoutes,
  ...adminRoutes,
]
