/**
 * shadcn/ui Components
 * 统一导出所有 shadcn UI 组件，简化导入
 *
 * 使用方式:
 * import { Button, Input, Card } from '@/components/ui'
 */

// 布局组件
export { default as Card } from './card.vue'
export { default as Separator } from './separator.vue'

// Tabs 选项卡系列
export { default as Tabs } from './tabs.vue'
export { default as TabsContent } from './tabs-content.vue'
export { default as TabsList } from './tabs-list.vue'
export { default as TabsTrigger } from './tabs-trigger.vue'

// 表单组件
export { default as Button } from './button.vue'
export { default as Input } from './input.vue'
export { default as Textarea } from './textarea.vue'
export { default as Label } from './label.vue'
export { default as Checkbox } from './checkbox.vue'
export { default as Switch } from './switch.vue'

// Select 选择器系列
export { default as Select } from './select.vue'
export { default as SelectTrigger } from './select-trigger.vue'
export { default as SelectValue } from './select-value.vue'
export { default as SelectContent } from './select-content.vue'
export { default as SelectItem } from './select-item.vue'

// 反馈组件
export { default as Badge } from './badge.vue'
export { default as Skeleton } from './skeleton.vue'

// Dialog 对话框系列
export { default as Dialog } from './dialog/Dialog.vue'
export { default as DialogContent } from './dialog/DialogContent.vue'
export { default as DialogHeader } from './dialog/DialogHeader.vue'
export { default as DialogTitle } from './dialog/DialogTitle.vue'
export { default as DialogDescription } from './dialog/DialogDescription.vue'
export { default as DialogFooter } from './dialog/DialogFooter.vue'

// Table 表格系列
export { default as Table } from './table.vue'
export { default as TableBody } from './table-body.vue'
export { default as TableCell } from './table-cell.vue'
export { default as TableHead } from './table-head.vue'
export { default as TableHeader } from './table-header.vue'
export { default as TableRow } from './table-row.vue'
export { default as TableCard } from './table-card.vue'

// Avatar 头像系列
export { default as Avatar } from './avatar.vue'
export { default as AvatarFallback } from './avatar-fallback.vue'
export { default as AvatarImage } from './avatar-image.vue'

// 分页组件
export { default as Pagination } from './pagination.vue'

// 操作按钮
export { default as RefreshButton } from './refresh-button.vue'

// Tooltip 提示系列
export { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from './tooltip'

// Collapsible 折叠系列
export { default as Collapsible } from './collapsible.vue'
export { default as CollapsibleTrigger } from './collapsible-trigger.vue'
export { default as CollapsibleContent } from './collapsible-content.vue'
