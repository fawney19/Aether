import { ref } from 'vue'

export type ConfirmVariant = 'danger' | 'destructive' | 'warning' | 'info' | 'question'

export interface ConfirmOptions {
  title?: string
  message: string
  confirmText?: string
  cancelText?: string
  variant?: ConfirmVariant
}

interface ConfirmState extends ConfirmOptions {
  isOpen: boolean
  resolve?: (value: boolean) => void
}

const state = ref<ConfirmState>({
  isOpen: false,
  message: '',
  title: '确认操作',
  confirmText: '确认',
  cancelText: '取消',
  variant: 'question'
})

export function useConfirm() {
  /**
   * 显示确认对话框
   * @param options 对话框选项
   * @returns Promise<boolean> - true表示确认，false表示取消
   */
  const confirm = (options: ConfirmOptions): Promise<boolean> => {
    return new Promise((resolve) => {
      state.value = {
        isOpen: true,
        title: options.title || '确认操作',
        message: options.message,
        confirmText: options.confirmText || '确认',
        cancelText: options.cancelText || '取消',
        variant: options.variant || 'question',
        resolve
      }
    })
  }

  /**
   * 便捷方法：危险操作确认（红色主题）
   */
  const confirmDanger = (message: string, title?: string, confirmText?: string): Promise<boolean> => {
    return confirm({
      message,
      title: title || '危险操作',
      confirmText: confirmText || '删除',
      variant: 'danger'
    })
  }

  /**
   * 便捷方法：警告确认（黄色主题）
   */
  const confirmWarning = (message: string, title?: string): Promise<boolean> => {
    return confirm({
      message,
      title: title || '警告',
      confirmText: '继续',
      variant: 'warning'
    })
  }

  /**
   * 便捷方法：信息确认（蓝色主题）
   */
  const confirmInfo = (message: string, title?: string): Promise<boolean> => {
    return confirm({
      message,
      title: title || '提示',
      confirmText: '确定',
      variant: 'info'
    })
  }

  /**
   * 处理确认
   */
  const handleConfirm = () => {
    if (state.value.resolve) {
      state.value.resolve(true)
    }
    state.value.isOpen = false
  }

  /**
   * 处理取消
   */
  const handleCancel = () => {
    if (state.value.resolve) {
      state.value.resolve(false)
    }
    state.value.isOpen = false
  }

  return {
    state,
    confirm,
    confirmDanger,
    confirmWarning,
    confirmInfo,
    handleConfirm,
    handleCancel
  }
}