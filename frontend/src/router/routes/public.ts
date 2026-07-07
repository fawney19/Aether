import type { RouteRecordRaw } from 'vue-router'
import { view } from './helpers'

export const publicRoutes: RouteRecordRaw[] = [
  {
    path: '/',
    name: 'Home',
    component: view(() => import('@/views/public/Home.vue')),
    meta: { requiresAuth: false }
  },
  {
    path: '/register',
    name: 'RegisterEntry',
    component: view(() => import('@/views/public/Home.vue')),
    meta: { requiresAuth: false }
  },
  {
    path: '/privacy-policy',
    name: 'PrivacyPolicy',
    component: view(() => import('@/views/public/PrivacyPolicy.vue')),
    meta: { requiresAuth: false }
  },
  {
    path: '/guide',
    component: view(() => import('@/views/public/guide/GuideLayout.vue')),
    meta: { requiresAuth: false },
    children: [
      {
        path: '',
        name: 'GuideOverview',
        component: view(() => import('@/views/public/guide/Overview.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'architecture',
        name: 'GuideArchitecture',
        component: view(() => import('@/views/public/guide/ArchitectureGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'concepts',
        name: 'GuideConcepts',
        component: view(() => import('@/views/public/guide/ConceptsGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'strategy',
        name: 'GuideStrategy',
        component: view(() => import('@/views/public/guide/StrategyGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'advanced',
        name: 'GuideAdvanced',
        component: view(() => import('@/views/public/guide/AdvancedGuide.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'faq',
        name: 'GuideFaq',
        component: view(() => import('@/views/public/guide/GuideFaq.vue')),
        meta: { requiresAuth: false }
      },
      {
        path: 'modules',
        name: 'GuideModules',
        component: view(() => import('@/views/public/guide/ModulesGuide.vue')),
        meta: { requiresAuth: false }
      }
    ]
  },
  {
    path: '/logo-demo',
    name: 'LogoColorDemo',
    component: view(() => import('@/views/public/LogoColorDemo.vue')),
    meta: { requiresAuth: false }
  },
  {
    path: '/auth/callback',
    name: 'AuthCallback',
    component: view(() => import('@/views/public/AuthCallback.vue')),
    meta: { requiresAuth: false }
  }
]
