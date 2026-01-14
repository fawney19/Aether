import { onMounted, onUnmounted, ref } from 'vue'

/**
 * 全局对话框栈，用于管理嵌套对话框的 ESC 键处理顺序
 * 栈顶的对话框优先处理 ESC 键
 */
const dialogStack = ref<Array<() => void | boolean>>([])

/**
 * 全局 ESC 键监听器（只有一个）
 */
let globalListenerAttached = false

function globalEscapeHandler(event: KeyboardEvent) {
  // 只处理 ESC 键
  if (event.key !== 'Escape') return

  // 从栈顶向栈底查找能处理 ESC 键的对话框
  // 倒序遍历，先检查最后加入的（栈顶）
  for (let i = dialogStack.value.length - 1; i >= 0; i--) {
    const handler = dialogStack.value[i]
    const handled = handler()

    if (handled === true) {
      // 该对话框已处理事件，阻止传播
      event.stopPropagation()
      event.stopImmediatePropagation()
      return
    }
  }
}

/**
 * ESC 键监听 Composable（栈管理版本）
 * 用于按 ESC 键关闭弹窗或其他可关闭的组件
 *
 * 支持嵌套对话框场景：只有最上层的对话框（最后打开的）会响应 ESC 键
 *
 * @param callback - 按 ESC 键时执行的回调函数，返回 true 表示已处理事件
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
  const isInStack = ref(false)

  // 包装原始回调，添加输入框检查
  function wrappedCallback(): void | boolean {
    // 检查组件是否还活跃
    if (!isActive.value) return false

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
      if (isInputElement) return false
    }

    // 执行原始回调
    const handled = callback()

    // 如果只监听一次，则从栈中移除
    if (once && handled === true) {
      removeFromStack()
    }

    // 移除当前元素的焦点，避免残留样式
    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur()
    }

    return handled
  }

  function addToStack() {
    if (isInStack.value) return

    // 将当前处理器加入栈顶
    dialogStack.value.push(wrappedCallback)
    isInStack.value = true

    // 确保全局监听器已添加
    if (!globalListenerAttached) {
      document.addEventListener('keydown', globalEscapeHandler, true) // 使用捕获阶段
      globalListenerAttached = true
    }
  }

  function removeFromStack() {
    if (!isInStack.value) return

    // 从栈中移除当前处理器
    const index = dialogStack.value.indexOf(wrappedCallback)
    if (index > -1) {
      dialogStack.value.splice(index, 1)
    }
    isInStack.value = false

    // 如果栈为空，移除全局监听器
    if (dialogStack.value.length === 0 && globalListenerAttached) {
      document.removeEventListener('keydown', globalEscapeHandler, true)
      globalListenerAttached = false
    }
  }

  onMounted(() => {
    addToStack()
  })

  onUnmounted(() => {
    isActive.value = false
    removeFromStack()
  })

  return {
    addToStack,
    removeFromStack
  }
}
