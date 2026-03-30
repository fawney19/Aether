import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import path from 'path'
import { execSync } from 'child_process'

function getGitVersion(): string {
  try {
    return execSync('git describe --tags --always').toString().trim()
  } catch {
    return '0.0.0.dev0'
  }
}

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  // GitHub Pages 部署时使用仓库名作为 base
  base: process.env.GITHUB_PAGES === 'true' ? '/Aether/' : '/',
  plugins: [vue()],
  define: {
    __APP_VERSION__: JSON.stringify(getGitVersion()),
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    // 使用 esbuild 进行压缩（默认）
    minify: 'esbuild',
    rollupOptions: {
      output: {
        // 手动分块以优化加载性能
        manualChunks: {
          // Vue 核心库
          'vue-vendor': ['vue', 'vue-router', 'pinia'],
          // UI 组件库
          'ui-vendor': ['radix-vue', 'lucide-vue-next'],
          // 工具库
          'utils-vendor': ['axios', 'marked', 'dompurify'],
          // 图表库
          'chart-vendor': ['chart.js', 'vue-chartjs'],
        },
      },
    },
    // esbuild 配置用于移除 console
    target: 'es2015',
  },
  esbuild: {
    // 生产环境移除 console 和 debugger
    drop: mode === 'production' ? ['console', 'debugger'] : [],
  },
  server: {
    allowedHosts: ['trish-salamandrine-uncongruously.ngrok-free.dev'],
    port: 5173,
    proxy: {
      // 只代理真正的 API 路径
      // 注意：本地开发时后端默认运行在 8084 端口（见 src/config/settings.py）
      // 如果使用 Docker，则通过 APP_PORT 环境变量映射（默认 80）
      '/api/': {
        target: 'http://localhost:8084',  // 本地开发端口
        changeOrigin: true,
        secure: false,
      },
      '/v1/': {
        target: 'http://localhost:8084',  // 本地开发端口
        changeOrigin: true,
        secure: false,
      },
      '/health': {
        target: 'http://localhost:8084',  // 本地开发端口
        changeOrigin: true,
        secure: false,
      },
    },
  },
  preview: {
    port: 5173,
  }
}))
