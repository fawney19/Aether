import { ref } from 'vue'
import { TOAST_CONFIG } from '@/config/constants'
import { getI18nLocale } from '@/i18n'
import { translateLegacyText } from '@/i18n/messages'

export type ToastVariant = 'success' | 'error' | 'warning' | 'info'

export interface Toast {
  id: string
  title?: string
  message?: string
  variant?: ToastVariant
  duration?: number
}

export type ToastOptions = Omit<Toast, 'id' | 'variant'> & {
  description?: string
  variant?: ToastVariant | 'destructive'
}

const toasts = ref<Toast[]>([])

export function useToast() {
  function localizeToastText(value: string | undefined): string | undefined {
    return value === undefined ? undefined : translateLegacyText(value, getI18nLocale())
  }

  function normalizeToastVariant(variant: ToastOptions['variant']): ToastVariant {
    return variant === 'destructive' ? 'error' : variant || 'info'
  }

  function showToast(options: ToastOptions) {
    const { description, ...toastOptions } = options
    const toast: Toast = {
      id: Date.now().toString(),
      duration: 5000,
      ...toastOptions,
      variant: normalizeToastVariant(options.variant),
      title: localizeToastText(options.title),
      message: localizeToastText(options.message ?? description),
    }


    toasts.value.push(toast)

    // 注释掉这里的 setTimeout，因为现在由组件自己处理
    // if (toast.duration && toast.duration > 0) {
    //   setTimeout(() => {
    //     removeToast(toast.id)
    //   }, toast.duration)
    // }

    return toast.id
  }

  function removeToast(id: string) {
    const index = toasts.value.findIndex(t => t.id === id)
    if (index > -1) {
      toasts.value.splice(index, 1)
    }
  }

  function success(message: string, title?: string) {
    return showToast({ message, title, variant: 'success', duration: TOAST_CONFIG.SUCCESS_DURATION })
  }

  function error(message: string, title?: string) {
    return showToast({ message, title, variant: 'error', duration: TOAST_CONFIG.ERROR_DURATION })
  }

  function warning(message: string, title?: string) {
    return showToast({ message, title, variant: 'warning', duration: TOAST_CONFIG.WARNING_DURATION })
  }

  function info(message: string, title?: string) {
    return showToast({ message, title, variant: 'info', duration: TOAST_CONFIG.INFO_DURATION })
  }

  function clearAll() {
    toasts.value = []
  }

  return {
    toasts,
    showToast,
    removeToast,
    toast: showToast,
    success,
    error,
    warning,
    info,
    clearAll
  }
}
