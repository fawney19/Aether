import { Mail, Shield, AlertTriangle, BellRing } from 'lucide-vue-next'
import type { LucideIcon } from 'lucide-vue-next'

export interface BuiltinTool {
  name: string
  description: string
  href: string
  icon: LucideIcon
}

export const BUILTIN_TOOLS: BuiltinTool[] = [
  {
    name: '邮件配置',
    description: '配置 SMTP 邮件服务，管理邮件模板和发送设置',
    href: '/admin/email',
    icon: Mail,
  },
  {
    name: '通知服务',
    description: '管理通知项、模板和推送服务策略',
    href: '/admin/notification-service',
    icon: BellRing,
  },
  {
    name: 'IP 安全',
    description: '管理 IP 黑白名单，控制系统访问权限',
    href: '/admin/ip-security',
    icon: Shield,
  },
  {
    name: '审计日志',
    description: '查看系统操作日志，追踪安全事件与变更记录',
    href: '/admin/audit-logs',
    icon: AlertTriangle,
  },
]

/** href → display name mapping for breadcrumbs */
export const BUILTIN_TOOL_BREADCRUMBS: Record<string, string> = Object.fromEntries(
  BUILTIN_TOOLS.map(t => [t.href, t.name])
)
