import js from '@eslint/js'
import tseslint from 'typescript-eslint'
import pluginVue from 'eslint-plugin-vue'
import vueParser from 'vue-eslint-parser'

export default [
  // 忽略的文件和目录
  {
    ignores: ['dist/**', 'node_modules/**', '*.config.js', '*.config.ts'],
  },

  // JavaScript 基础配置
  js.configs.recommended,

  // TypeScript 配置
  ...tseslint.configs.recommended,

  // Vue 配置
  ...pluginVue.configs['flat/recommended'],

  // 全局配置
  {
    languageOptions: {
      ecmaVersion: 'latest',
      sourceType: 'module',
      globals: {
        // 浏览器全局变量
        window: 'readonly',
        document: 'readonly',
        navigator: 'readonly',
        console: 'readonly',
        localStorage: 'readonly',
        sessionStorage: 'readonly',
        fetch: 'readonly',
        URL: 'readonly',
        URLSearchParams: 'readonly',
        FormData: 'readonly',
        Blob: 'readonly',
        File: 'readonly',
        FileReader: 'readonly',
        HTMLElement: 'readonly',
        HTMLInputElement: 'readonly',
        HTMLSelectElement: 'readonly',
        MouseEvent: 'readonly',
        KeyboardEvent: 'readonly',
        Event: 'readonly',
        EventTarget: 'readonly',
        CustomEvent: 'readonly',
        MutationObserver: 'readonly',
        ResizeObserver: 'readonly',
        IntersectionObserver: 'readonly',
        requestAnimationFrame: 'readonly',
        cancelAnimationFrame: 'readonly',
        setTimeout: 'readonly',
        clearTimeout: 'readonly',
        setInterval: 'readonly',
        clearInterval: 'readonly',
        queueMicrotask: 'readonly',
        // Node.js 全局变量
        process: 'readonly',
        __dirname: 'readonly',
        __filename: 'readonly',
        module: 'readonly',
        require: 'readonly',
        exports: 'readonly',
        Buffer: 'readonly',
        // DOM/SVG 全局类型
        alert: 'readonly',
        confirm: 'readonly',
        prompt: 'readonly',
        Node: 'readonly',
        NodeList: 'readonly',
        Element: 'readonly',
        SVGPathElement: 'readonly',
        SVGElement: 'readonly',
        DOMParser: 'readonly',
        XMLSerializer: 'readonly',
        getComputedStyle: 'readonly',
        performance: 'readonly',
        PerformanceObserver: 'readonly',
        Image: 'readonly',
        Audio: 'readonly',
        WebSocket: 'readonly',
        Worker: 'readonly',
        SharedWorker: 'readonly',
        ServiceWorker: 'readonly',
        crypto: 'readonly',
        atob: 'readonly',
        btoa: 'readonly',
        TextEncoder: 'readonly',
        TextDecoder: 'readonly',
        AbortController: 'readonly',
        AbortSignal: 'readonly',
        Headers: 'readonly',
        Request: 'readonly',
        Response: 'readonly',
        ClipboardItem: 'readonly',
        Selection: 'readonly',
        Range: 'readonly',
        matchMedia: 'readonly',
        history: 'readonly',
        location: 'readonly',
        open: 'readonly',
        close: 'readonly',
        print: 'readonly',
        scrollTo: 'readonly',
        scrollBy: 'readonly',
        getSelection: 'readonly',
        Intl: 'readonly',
        globalThis: 'readonly',
        // Canvas/WebGL
        HTMLCanvasElement: 'readonly',
        CanvasRenderingContext2D: 'readonly',
        WebGLRenderingContext: 'readonly',
        WebGL2RenderingContext: 'readonly',
        ImageData: 'readonly',
        Path2D: 'readonly',
        OffscreenCanvas: 'readonly',
        // 更多 DOM 类型
        HTMLButtonElement: 'readonly',
        HTMLTextAreaElement: 'readonly',
        MediaQueryList: 'readonly',
        MediaQueryListEvent: 'readonly',
        FocusEvent: 'readonly',
        DragEvent: 'readonly',
        PointerEvent: 'readonly',
        TouchEvent: 'readonly',
        WheelEvent: 'readonly',
        AnimationEvent: 'readonly',
        TransitionEvent: 'readonly',
        ClipboardEvent: 'readonly',
        InputEvent: 'readonly',
        CompositionEvent: 'readonly',
        UIEvent: 'readonly',
        ProgressEvent: 'readonly',
        ErrorEvent: 'readonly',
        StorageEvent: 'readonly',
        PopStateEvent: 'readonly',
        HashChangeEvent: 'readonly',
        PageTransitionEvent: 'readonly',
        BeforeUnloadEvent: 'readonly',
        MessageEvent: 'readonly',
        SecurityPolicyViolationEvent: 'readonly',
        DeviceMotionEvent: 'readonly',
        DeviceOrientationEvent: 'readonly',
      },
    },
  },

  // Vue 文件配置
  {
    files: ['**/*.vue'],
    languageOptions: {
      parser: vueParser,
      parserOptions: {
        parser: tseslint.parser,
        ecmaVersion: 'latest',
        sourceType: 'module',
      },
    },
    rules: {
      // Vue 规则
      'vue/multi-word-component-names': 'off',
      'vue/no-v-html': 'warn', // 降级为警告，某些场景需要使用
      'vue/component-api-style': ['error', ['script-setup']],
      'vue/component-name-in-template-casing': ['error', 'PascalCase'],
      'vue/custom-event-name-casing': ['warn', 'camelCase'], // 降级为警告，逐步迁移
      'vue/define-macros-order': [
        'error',
        {
          order: ['defineProps', 'defineEmits'],
        },
      ],
      'vue/html-comment-content-spacing': ['error', 'always'],
      'vue/no-unused-refs': 'warn', // 降级为警告
      'vue/no-useless-v-bind': 'error',
      'vue/padding-line-between-blocks': ['error', 'always'],
      'vue/prefer-separate-static-class': 'error',
    },
  },

  // TypeScript 文件配置
  {
    files: ['**/*.ts', '**/*.tsx', '**/*.vue'],
    rules: {
      '@typescript-eslint/no-unused-vars': [
        'error',
        {
          argsIgnorePattern: '^_',
          varsIgnorePattern: '^_',
        },
      ],
      '@typescript-eslint/no-explicit-any': 'warn',
      '@typescript-eslint/explicit-function-return-type': 'off',
      '@typescript-eslint/explicit-module-boundary-types': 'off',
      '@typescript-eslint/no-non-null-assertion': 'warn',
    },
  },

  // 通用规则
  {
    files: ['**/*.js', '**/*.ts', '**/*.tsx', '**/*.vue'],
    rules: {
      'no-console': process.env.NODE_ENV === 'production' ? 'error' : 'warn',
      'no-debugger': process.env.NODE_ENV === 'production' ? 'error' : 'warn',
      'prefer-const': 'error',
      'no-var': 'error',
      'object-shorthand': ['error', 'always'],
      'prefer-template': 'error',
      'prefer-arrow-callback': 'error',
    },
  },

  // 允许 main.ts 和 logger.ts 使用 console
  {
    files: ['**/main.ts', '**/logger.ts'],
    rules: {
      'no-console': 'off',
    },
  },
]
