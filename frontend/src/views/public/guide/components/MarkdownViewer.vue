<template>
  <div class="markdown-viewer-container">
    <!-- eslint-disable vue/no-v-html -->
    <div
      class="markdown-body"
      v-html="renderedHtml"
    />
    <!-- eslint-enable vue/no-v-html -->
  </div>
</template>

<script setup lang="ts">
import { ref, watch, onMounted } from 'vue'
import { marked, type Renderer } from 'marked'
import hljs from 'highlight.js'
import { sanitizeMarkdown } from '@/utils/sanitize'
import 'highlight.js/styles/github-dark.css'

const props = defineProps<{
  content: string
}>()

const renderedHtml = ref('')

// marked v16+ 需要通过自定义 renderer 实现代码高亮
const renderer: Partial<Renderer> = {
  code({ text, lang }) {
    const language = lang && hljs.getLanguage(lang) ? lang : 'plaintext'
    const highlighted = hljs.highlight(text, { language }).value
    return `<pre><code class="hljs language-${language}">${highlighted}</code></pre>`
  }
}

marked.use({ renderer, gfm: true, breaks: true })

const renderMarkdown = () => {
  if (!props.content) {
    renderedHtml.value = ''
    return
  }

  try {
    const rawHtml = marked.parse(props.content) as string
    renderedHtml.value = sanitizeMarkdown(rawHtml)
  } catch {
    renderedHtml.value = '<p class="text-red-500">Failed to render content</p>'
  }
}

watch(() => props.content, () => {
  renderMarkdown()
})

onMounted(() => {
  renderMarkdown()
})
</script>

<style>
/* Literary Tech Markdown Styles */
.markdown-viewer-container {
  @apply w-full max-w-none;
}

.markdown-body {
  @apply text-[var(--color-text)] font-serif leading-relaxed;
}

.markdown-body h1 {
  @apply text-4xl mb-8 mt-12 font-medium tracking-tight text-[var(--color-text)];
  font-family: var(--serif);
}

.markdown-body h2 {
  @apply text-2xl mb-6 mt-12 font-medium tracking-tight text-[var(--color-text)] border-b pb-2;
  border-color: var(--color-border-soft);
  font-family: var(--serif);
}

.markdown-body h3 {
  @apply text-xl mb-4 mt-8 font-medium tracking-tight text-[var(--color-text)];
  font-family: var(--serif);
}

.markdown-body p {
  @apply mb-4 text-[1.05rem] opacity-90;
  font-family: var(--serif);
}

.markdown-body ul, 
.markdown-body ol {
  @apply pl-6 mb-6 opacity-90 space-y-2 text-[1.05rem];
  font-family: var(--serif);
}

.markdown-body ul {
  list-style-type: disc;
}

.markdown-body ol {
  list-style-type: decimal;
}

.markdown-body blockquote {
  @apply border-l-4 pl-4 italic opacity-80 mb-6;
  border-color: var(--book-cloth);
  background: var(--color-background-soft);
  @apply py-2 rounded-r-lg;
}

/* Code Blocks & Inline Code */
.markdown-body pre {
  @apply p-4 rounded-xl overflow-x-auto mb-6 text-sm backdrop-blur-md;
  background: var(--color-background-soft);
  border: 1px solid var(--color-border-soft);
  font-family: var(--monospace) !important;
  box-shadow: var(--shadow-sm);
}

.markdown-body code {
  font-family: var(--monospace) !important;
}

.markdown-body :not(pre) > code {
  @apply px-1.5 py-0.5 rounded text-sm;
  background: var(--color-background-soft);
  color: var(--book-cloth);
  font-family: var(--monospace) !important;
  border: 1px solid var(--color-border-soft);
}

/* Tables */
.markdown-body table {
  @apply w-full mb-6 border-collapse text-left;
  font-family: var(--sans-serif);
}

.markdown-body th {
  @apply px-4 py-3 font-medium bg-[var(--color-background-soft)] border;
  border-color: var(--color-border-soft);
}

.markdown-body td {
  @apply px-4 py-3 border opacity-90;
  border-color: var(--color-border-soft);
}

.markdown-body tr:nth-child(even) {
  @apply bg-[var(--color-background)]/50;
}

/* Links */
.markdown-body a {
  @apply text-[var(--book-cloth)] underline decoration-dashed underline-offset-4 transition-all;
}

.markdown-body a:hover {
  @apply decoration-solid opacity-80;
}

/* Images */
.markdown-body img {
  @apply max-w-full rounded-xl border object-contain mx-auto mb-6 shadow-sm;
  border-color: var(--color-border-soft);
  max-height: 600px;
}

.markdown-body p:has(img) {
  @apply text-center;
}
</style>
