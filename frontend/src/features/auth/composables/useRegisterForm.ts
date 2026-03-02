import { computed, nextTick, onUnmounted, ref, watch, type Ref } from 'vue'
import { authApi } from '@/api/auth'
import { useToast } from '@/composables/useToast'
import { parseApiError } from '@/utils/errorParser'

interface RegisterFormData {
  email: string
  username: string
  password: string
  confirmPassword: string
  verificationCode: string
}

interface UseRegisterFormOptions {
  requireEmailVerification: Ref<boolean>
  emailConfigured: Ref<boolean>
}

export function useRegisterForm(options: UseRegisterFormOptions) {
  const { success, error: showError } = useToast()

  const formNonce = ref(createFormNonce())
  const codeInputRefs = ref<(HTMLInputElement | null)[]>([])
  const codeDigits = ref<string[]>(['', '', '', '', '', ''])

  const formData = ref<RegisterFormData>({
    email: '',
    username: '',
    password: '',
    confirmPassword: '',
    verificationCode: ''
  })

  const isLoading = ref(false)
  const loadingText = ref('注册中...')
  const isSendingCode = ref(false)
  const emailVerified = ref(false)
  const verificationError = ref(false)
  const codeSentAt = ref<number | null>(null)
  const cooldownSeconds = ref(0)
  const expireMinutes = ref(5)
  const cooldownTimer = ref<number | null>(null)

  const canSendCode = computed(() => {
    if (!formData.value.email) return false
    if (cooldownSeconds.value > 0) return false
    return true
  })

  const sendCodeButtonText = computed(() => {
    if (isSendingCode.value) return '发送中...'
    if (emailVerified.value) return '验证成功'
    if (cooldownSeconds.value > 0) return `${cooldownSeconds.value}秒后重试`
    if (codeSentAt.value) return '重新发送验证码'
    return '发送验证码'
  })

  const usernameRegex = /^[a-zA-Z0-9_.-]+$/
  const uppercaseRegex = /[A-Z]/
  const lowercaseRegex = /[a-z]/
  const digitRegex = /\d/

  const usernameError = computed(() => {
    const username = formData.value.username.trim()
    if (!username) return ''
    if (username.length < 3) return '用户名长度至少为3个字符'
    if (username.length > 30) return '用户名长度不能超过30个字符'
    if (!usernameRegex.test(username)) return '用户名只能包含字母、数字、下划线、连字符和点号'
    return ''
  })

  const passwordHintText = computed(() => {
    const password = formData.value.password
    if (!password) {
      return '至少 8 位，且必须包含大写字母、小写字母和数字'
    }
    return getPasswordValidationError(password) ?? '密码符合要求'
  })

  const passwordHintIsError = computed(() => {
    const password = formData.value.password
    if (!password) return false
    return !!getPasswordValidationError(password)
  })

  const canSubmit = computed(() => {
    const hasBasicInfo =
      formData.value.username &&
      formData.value.password &&
      formData.value.confirmPassword

    if (!hasBasicInfo) return false
    if (usernameError.value) return false

    if (options.requireEmailVerification.value) {
      if (!formData.value.email || !emailVerified.value) {
        return false
      }
    }

    if (formData.value.password !== formData.value.confirmPassword) {
      return false
    }

    if (getPasswordValidationError(formData.value.password)) {
      return false
    }

    return true
  })

  let emailCheckTimer: number | null = null

  watch(
    () => formData.value.email,
    (newEmail, oldEmail) => {
      if (newEmail !== oldEmail) {
        emailVerified.value = false
        verificationError.value = false
        codeSentAt.value = null
        cooldownSeconds.value = 0
        if (cooldownTimer.value !== null) {
          clearInterval(cooldownTimer.value)
          cooldownTimer.value = null
        }
        codeDigits.value = ['', '', '', '', '', '']
      }

      if (emailCheckTimer !== null) {
        clearTimeout(emailCheckTimer)
      }

      const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
      if (!emailRegex.test(newEmail)) return

      emailCheckTimer = window.setTimeout(() => {
        checkAndRestoreVerificationStatus(newEmail)
      }, 500)
    }
  )

  onUnmounted(() => {
    if (cooldownTimer.value !== null) {
      clearInterval(cooldownTimer.value)
    }
    if (emailCheckTimer !== null) {
      clearTimeout(emailCheckTimer)
    }
  })

  function createFormNonce(): string {
    return Math.random().toString(36).slice(2, 10)
  }

  function setCodeInputRef(index: number, el: HTMLInputElement | null) {
    codeInputRefs.value[index] = el
  }

  function handleCodeInput(index: number, event: Event) {
    const input = event.target as HTMLInputElement
    const value = input.value

    if (!/^\d*$/.test(value)) {
      input.value = codeDigits.value[index]
      return
    }

    codeDigits.value[index] = value

    if (value && index < 5) {
      codeInputRefs.value[index + 1]?.focus()
    }

    const fullCode = codeDigits.value.join('')
    if (fullCode.length === 6 && /^\d+$/.test(fullCode)) {
      handleCodeComplete(fullCode)
    }
  }

  function handleCodeKeyDown(index: number, event: KeyboardEvent) {
    if (event.key === 'Backspace') {
      if (!codeDigits.value[index] && index > 0) {
        codeInputRefs.value[index - 1]?.focus()
        codeDigits.value[index - 1] = ''
      } else {
        codeDigits.value[index] = ''
      }
    } else if (event.key === 'ArrowLeft' && index > 0) {
      codeInputRefs.value[index - 1]?.focus()
    } else if (event.key === 'ArrowRight' && index < 5) {
      codeInputRefs.value[index + 1]?.focus()
    }
  }

  function handleCodePaste(event: ClipboardEvent) {
    event.preventDefault()
    const pastedData = event.clipboardData?.getData('text') || ''
    const cleanedData = pastedData.replace(/\D/g, '').slice(0, 6)

    if (!cleanedData) {
      return
    }

    for (let i = 0; i < 6; i++) {
      codeDigits.value[i] = cleanedData[i] || ''
    }

    const nextEmptyIndex = codeDigits.value.findIndex((d) => !d)
    const focusIndex = nextEmptyIndex >= 0 ? nextEmptyIndex : 5
    codeInputRefs.value[focusIndex]?.focus()

    if (cleanedData.length === 6) {
      handleCodeComplete(cleanedData)
    }
  }

  function clearCodeInputs() {
    codeDigits.value = ['', '', '', '', '', '']
    codeInputRefs.value[0]?.focus()
  }

  function startCooldown(seconds: number) {
    if (cooldownTimer.value !== null) {
      clearInterval(cooldownTimer.value)
    }

    cooldownSeconds.value = seconds
    cooldownTimer.value = window.setInterval(() => {
      cooldownSeconds.value--
      if (cooldownSeconds.value <= 0) {
        if (cooldownTimer.value !== null) {
          clearInterval(cooldownTimer.value)
          cooldownTimer.value = null
        }
      }
    }, 1000)
  }

  async function checkAndRestoreVerificationStatus(email: string) {
    if (!email || !options.requireEmailVerification.value) return

    try {
      const status = await authApi.getVerificationStatus(email)

      if (status.has_pending_code) {
        codeSentAt.value = Date.now()
        verificationError.value = false

        if (status.cooldown_remaining && status.cooldown_remaining > 0) {
          startCooldown(status.cooldown_remaining)
        }
      }
    } catch {
      // ignore
    }
  }

  function resetForm() {
    formData.value = {
      email: '',
      username: '',
      password: '',
      confirmPassword: '',
      verificationCode: ''
    }

    emailVerified.value = false
    verificationError.value = false
    isSendingCode.value = false
    codeSentAt.value = null
    cooldownSeconds.value = 0

    formNonce.value = createFormNonce()

    if (cooldownTimer.value !== null) {
      clearInterval(cooldownTimer.value)
      cooldownTimer.value = null
    }

    codeDigits.value = ['', '', '', '', '', '']
  }

  async function handleSendCode() {
    if (!formData.value.email) {
      showError('请输入邮箱')
      return
    }

    const emailRegex = /^[^\s@]+@[^\s@]+\.[^\s@]+$/
    if (!emailRegex.test(formData.value.email)) {
      showError('请输入有效的邮箱地址', '邮箱格式错误')
      return
    }

    isSendingCode.value = true

    try {
      const response = await authApi.sendVerificationCode(formData.value.email)

      if (!response.success) {
        showError(response.message || '请稍后重试', '发送失败')
        return
      }

      codeSentAt.value = Date.now()
      if (response.expire_minutes) {
        expireMinutes.value = response.expire_minutes
      }

      success(`请查收邮件，验证码有效期 ${expireMinutes.value} 分钟`, '验证码已发送')

      startCooldown(60)

      nextTick(() => {
        codeInputRefs.value[0]?.focus()
      })
    } catch (error: unknown) {
      showError(parseApiError(error, '网络错误，请重试'), '发送失败')
    } finally {
      isSendingCode.value = false
    }
  }

  async function handleCodeComplete(code: string) {
    if (!formData.value.email || code.length !== 6 || emailVerified.value) {
      return
    }

    isLoading.value = true
    loadingText.value = '验证中...'
    verificationError.value = false

    try {
      const response = await authApi.verifyEmail(formData.value.email, code)

      if (!response.success) {
        verificationError.value = true
        showError(response.message || '验证码错误', '验证失败')
        clearCodeInputs()
        return
      }

      emailVerified.value = true
      success('邮箱验证通过，请继续完成注册', '验证成功')
    } catch (error: unknown) {
      verificationError.value = true
      showError(parseApiError(error, '验证码错误，请重试'), '验证失败')
      clearCodeInputs()
    } finally {
      isLoading.value = false
      loadingText.value = '注册中...'
    }
  }

  async function handleSubmit(): Promise<boolean> {
    if (formData.value.password !== formData.value.confirmPassword) {
      showError('两次输入的密码不一致', '密码不匹配')
      return false
    }

    const passwordError = getPasswordValidationError(formData.value.password)
    if (passwordError) {
      showError(passwordError, '密码错误')
      return false
    }

    if (options.requireEmailVerification.value && !emailVerified.value) {
      showError('请先完成邮箱验证')
      return false
    }

    isLoading.value = true
    loadingText.value = '注册中...'

    try {
      const registerData: { email?: string; username: string; password: string } = {
        username: formData.value.username,
        password: formData.value.password
      }

      if (formData.value.email && formData.value.email.trim()) {
        registerData.email = formData.value.email
      }

      const response = await authApi.register(registerData)
      success(response.message || '欢迎加入！请登录以继续', '注册成功')
      return true
    } catch (error: unknown) {
      showError(parseApiError(error, '注册失败，请重试'), '注册失败')
      return false
    } finally {
      isLoading.value = false
      loadingText.value = '注册中...'
    }
  }

  function formatMissingItems(items: string[]): string {
    if (items.length === 0) return ''
    if (items.length === 1) return items[0]
    if (items.length === 2) return `${items[0]}和${items[1]}`
    return `${items.slice(0, -1).join('、')}和${items[items.length - 1]}`
  }

  function getPasswordValidationError(password: string): string | null {
    const missingLength = password.length < 8
    const missingTypes: string[] = []

    if (!uppercaseRegex.test(password)) {
      missingTypes.push('大写字母')
    }
    if (!lowercaseRegex.test(password)) {
      missingTypes.push('小写字母')
    }
    if (!digitRegex.test(password)) {
      missingTypes.push('数字')
    }

    const missingComplexityText =
      missingTypes.length > 0 ? `必须包含${formatMissingItems(missingTypes)}` : ''

    if (missingLength && missingComplexityText) {
      return `至少 8 位，且${missingComplexityText}`
    }
    if (missingLength) {
      return '至少 8 位'
    }
    if (missingComplexityText) {
      return missingComplexityText
    }
    return null
  }

  return {
    formNonce,
    formData,
    codeDigits,
    emailVerified,
    isLoading,
    loadingText,
    isSendingCode,
    verificationError,
    canSendCode,
    sendCodeButtonText,
    usernameError,
    passwordHintText,
    passwordHintIsError,
    canSubmit,
    setCodeInputRef,
    handleCodeInput,
    handleCodeKeyDown,
    handleCodePaste,
    handleSendCode,
    handleSubmit,
    resetForm,
    startCooldown,
  }
}
