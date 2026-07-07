import { createApp } from 'vue'
import { createPinia } from 'pinia'
import router from './router'
import './style.css'
import App from './App.vue'
import { preloadCriticalModules } from './utils/importRetry'
import { createI18n } from './i18n'

const app = createApp(App)
const pinia = createPinia()
const i18n = createI18n()

app.use(pinia)
app.use(i18n)
app.use(router)

// 预加载关键模块
preloadCriticalModules()

// 全局错误处理器 - 只在开发环境下记录详细日志
app.config.errorHandler = (err: unknown, _instance, info) => {
  if (import.meta.env.DEV) {
    console.error('Global error handler:', err, info)
  }

  // 模块加载错误处理已移至 App.vue 的统一处理器中
}

app.mount('#app')
