# Thinking 块签名/清洗 - 进展记录

## 当前方案

**整流器模式（ThinkingRectifier）** - 自动错误触发，无需配置

### 工作原理

1. 正常请求时不做任何处理
2. 遇到签名/结构错误时，自动整流请求（移除 thinking 块）
3. 整流后重试一次
4. 移除的 thinking 块不影响模型上下文理解

### 核心文件

- `src/services/message/thinking_rectifier.py` - 整流器实现
- `src/services/orchestration/error_classifier.py` - 错误识别
- `src/services/orchestration/fallback_orchestrator.py` - 错误处理与重试

### 已移除的旧方案

以下预清洗功能已移除：
- 环境变量配置（`THINKING_SANITIZATION_*`）
- 系统配置（数据库）
- 前端配置界面
- Handler 层预清洗逻辑

`ThinkingSanitizer` 类保留但标记为 Legacy，仅供向后兼容。

---

## 2026-01-21 调整记录

### 最终方案

**整流器模式**取代**预清洗模式**：

| 对比 | 预清洗模式（已移除） | 整流器模式（当前） |
|------|---------------------|-------------------|
| 触发时机 | 每次请求前 | 遇到错误时 |
| 配置需求 | 需要启用开关 | 无需配置 |
| 清洗范围 | 可配置保留/移除 | 彻底移除 |
| 实现复杂度 | 高（策略判断复杂） | 低（错误触发即整流） |

### 移除的代码

1. **前端**：`SystemSettings.vue` 中的 thinking 配置 UI
2. **后端配置**：`settings.py` 和 `config.py` 中的 `thinking_sanitization_*`
3. **Handler 预清洗**：`cli_handler_base.py` 和 `chat_handler_base.py` 中的清洗调用
4. **ThinkingSanitizer 配置依赖**：不再从配置读取，使用硬编码默认值

### 保留的代码

- `ThinkingSanitizer` 类（Legacy，向后兼容）
- `ThinkingRectifier` 类（当前方案）
- 错误识别逻辑（扩展了更多错误模式）

---

## 历史背景

详见 `THINKING_SANITIZATION_PROPOSAL.md`（注意：该文档描述的是旧的预清洗方案，仅供参考）。
