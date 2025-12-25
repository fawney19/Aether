import { onMounted, onUnmounted, ref } from 'vue'

/**
 * ESC 键监听 Composable（简化版本，直接使用独立监听器）
 * 用于按 ESC 键关闭弹窗或其他可关闭的组件
 *
 * @param callback - 按 ESC 键时执行的回调函数，返回 true 表示已处理事件，阻止其他监听器执行
 * @param options - 配置选项
 */
export function useEscapeKey(
  callback: () => void | boolean,
  options: {
    /** 是否在输入框获得焦点时禁用 ESC 键，默认 true */
    disableOnInput?: boolean
    /** 是否只监听一次，默认 false */
    once?: boolean
  } = {}
) {
  const { disableOnInput = true, once = false } = options
  const isActive = ref(true)

  function handleKeyDown(event: KeyboardEvent) {
    // 只处理 ESC 键
    if (event.key !== 'Escape') return

    // 检查组件是否还活跃
    if (!isActive.value) return

    // 如果配置了在输入框获得焦点时禁用，则检查当前焦点元素
    if (disableOnInput) {
      const activeElement = document.activeElement
      const isInputElement = activeElement && (
        activeElement.tagName === 'INPUT' ||
        activeElement.tagName === 'TEXTAREA' ||
        activeElement.tagName === 'SELECT' ||
        activeElement.contentEditable === 'true' ||
        activeElement.getAttribute('role') === 'textbox' ||
        activeElement.getAttribute('role') === 'combobox'
      )

      // 如果焦点在输入框中，不处理 ESC 键
      if (isInputElement) return
    }

    // 执行回调，如果返回 true 则阻止其他监听器
    const handled = callback()
    if (handled === true) {
      event.stopImmediatePropagation()
    }

    // 移除当前元素的焦点，避免残留样式
    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur()
    }

    // 如果只监听一次，则移除监听器
    if (once) {
      removeEventListener()
    }
  }

  function addEventListener() {
    document.addEventListener('keydown', handleKeyDown)
  }

  function removeEventListener() {
    document.removeEventListener('keydown', handleKeyDown)
  }

  onMounted(() => {
    addEventListener()
  })

  onUnmounted(() => {
    isActive.value = false
    removeEventListener()
  })

  return {
    addEventListener,
    removeEventListener
  }
}