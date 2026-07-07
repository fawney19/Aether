import { defineConfig } from 'vitest/config'
import vue from '@vitejs/plugin-vue'
import path from 'path'
import { legacyTemplateI18nPlugin } from './src/i18n/legacy-template-transform'

export default defineConfig({
  plugins: [legacyTemplateI18nPlugin(), vue()],
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: ['./src/tests/vitest.setup.ts'],
    coverage: {
      provider: 'v8',
      reporter: ['text', 'json', 'html'],
      exclude: [
        'node_modules/',
        'dist/',
        '**/*.spec.ts',
        '**/*.d.ts',
        '**/vite.config.ts'
      ]
    }
  },
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
})
