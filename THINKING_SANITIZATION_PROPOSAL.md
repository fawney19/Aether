# Thinking Blocks 多供应商兼容性优化方案

> **注意**：本文档描述的是**旧的预清洗方案**，该方案已被**整流器模式**取代。
> 当前实现请参考 `PROGRESS.md` 和 `src/services/message/thinking_rectifier.py`。

## 当前实现（2026-01）

**整流器模式（ThinkingRectifier）**：
- 无需配置，自动工作
- 遇到签名/结构错误时自动整流（移除 thinking 块）
- 整流后重试一次
- 环境变量 `THINKING_SANITIZATION_*` 已移除

---

## 以下为历史方案文档（仅供参考）

## 问题背景

### 现象描述

在使用多个 Claude API 供应商（特别是三方中转服务）时，会出现以下错误：

```
messages.23.content.4.thinking.signature: Field required
```

或

```
Invalid `signature` in `thinking` block
```

### 根本原因

Claude Extended Thinking 功能返回的 `thinking` 块包含加密的 `signature` 字段，用于验证 thinking 内容确实由 Claude 生成：

```json
{
  "type": "thinking",
  "thinking": "Let me analyze this problem...",
  "signature": "WaUjzkypQ2mUE..." // 加密签名
}
```

**问题核心**：
1. `signature` 是供应商私钥签名，不同供应商使用不同的签名密钥
2. 在多轮对话中，历史消息包含之前供应商签名的 thinking 块
3. 当请求被路由到**不同供应商**时，该供应商无法验证其他供应商的签名
4. 结果：签名验证失败，请求被拒绝

### 实际场景

用户配置了 4 个渠道：
- **a、b、c**：三方中转（Anthropic 兼容，严格验证签名）
- **d**：GLM-4.7（宽松兼容，不验证签名）

**问题流程**：
```
第1轮 → Provider a → 正常工作，返回 thinking + signature_a
第2轮 → Provider a 故障（500）
      → 切换到 b → 收到 signature_a → 拒绝 ❌
      → 切换到 c → 收到 signature_a → 拒绝 ❌
      → 切换到 d → 不验证签名 → 成功 ✅
第3轮 → 又从 a 开始 → 收到 signature_a → 又拒绝 ❌
```

**循环原因**：历史消息中一直包含 a 签名的 thinking 块，导致 a/b/c 持续失败，只有 d 能兜底。

### 问题影响

| 影响 | 说明 |
|------|------|
| **功能受损** | 优质供应商（a/b/c）无法使用，被迫降级到兜底供应商（d） |
| **体验变差** | 用户需要手动回退历史或重新开始对话 |
| **资源浪费** | 系统反复尝试注定失败的供应商 |

---

## 优化方案

### 核心思路

**在下一次请求时清洗 thinking 块，使其兼容所有供应商**

```
正常流程 → a/b/c 正常工作
    ↓
签名错误 → d 兜底 → 检测到签名问题
    ↓
下次请求 → 自动清洗 thinking → 转换为通用格式
    ↓
a/b/c 恢复 → 继续按用户规则工作
```

### 设计原则

1. **向后兼容**：默认关闭，用户主动开启
2. **无感切换**：清洗后自动恢复 a/b/c 优先级
3. **持久有效**：清洗一次，持续有效
4. **可配置**：提供灵活的清洗策略选项

---

## 技术方案

### 方案选择

**时机选择**：下次请求时清洗（而非当前请求重试）

| 对比 | 当前请求重试 | 下次请求清洗 |
|------|-------------|-------------|
| 复杂度 | 需要重试逻辑 | 简单，标记即可 |
| 延迟 | 增加重试延迟 | 无额外延迟 |
| 实现难度 | 中 | 低 |
| **推荐** | | ✅ |

### 清洗规则

**默认方案**：`thinking` → `text`（保留内容）

```json
// 清洗前
{
  "type": "thinking",
  "thinking": "Let me analyze...",
  "signature": "WaUjzkypQ2..."
}

// 清洗后
{
  "type": "text",
  "text": "[Thinking] Let me analyze..."
}
```

**可选方案**：
- 模式 A：`convert_to_text` - 转换为 text（默认，推荐）
- 模式 B：`remove` - 直接删除（会丢失上下文）

### 实现架构

```
┌─────────────────────────────────────────────────────────┐
│                    配置层 (Settings)                      │
│  THINKING_SANITIZATION_ENABLED                          │
│  THINKING_SANITIZATION_MODE                             │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────┐
│               请求处理层 (ChatHandlerBase)                │
│  1. 检测是否需要清洗                                     │
│     - 历史消息中包含 thinking 块                         │
│     - 上次请求遇到签名错误                               │
│  2. 执行清洗转换                                         │
│     - thinking → text                                   │
│     - 移除 signature                                    │
│  3. 标记清洗状态                                         │
│     - metadata.sanitized = true                         │
└─────────────────────────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────┐
│            错误识别层 (ErrorClassifier)                   │
│  1. 识别签名相关错误                                     │
│     - "Invalid signature in thinking block"             │
│     - "thinking.signature: Field required"              │
│  2. 标记需要清洗                                         │
│     - 设置会话标记                                       │
└─────────────────────────────────────────────────────────┘
```

---

## 配置说明

### 环境变量配置

```bash
# 是否启用 Thinking 清洗兼容模式
THINKING_SANITIZATION_ENABLED=true

# 清洗模式：convert_to_text（转换为文本）| remove（直接删除）
THINKING_SANITIZATION_MODE=convert_to_text

# 转换后的文本前缀
THINKING_SANITIZATION_TEXT_PREFIX=[Thinking]

# 转换后的文本后缀（可选）
THINKING_SANITIZATION_TEXT_SUFFIX=

# 是否静默模式（true=不提示用户，false=在响应头添加标记）
THINKING_SANITIZATION_SILENT=true

# 是否禁用请求中的 thinking 参数（避免生成新的 thinking）
THINKING_SANITIZATION_DISABLE_THINKING_PARAM=false
```

### 前端配置说明

在管理后台添加"供应商兼容模式"开关：

```
设置 → 供应商配置 → 兼容模式

☐ 启用多供应商兼容模式
   开启后，将自动清理历史消息中的 thinking 块，
   以兼容使用不同签名密钥的供应商。

   清洗模式：○ 转换为文本（保留内容）  ○ 直接删除
```

---

## 实现清单

### 文件改动

| 文件 | 改动内容 | 代码量 |
|------|----------|--------|
| `src/core/exceptions.py` | 新增异常类型 | +20 行 |
| `src/services/orchestration/error_classifier.py` | 签名错误识别 | +40 行 |
| `src/api/handlers/base/chat_handler_base.py` | 清洗逻辑 | +70 行 |
| `src/config/settings.py` | 配置项 | +20 行 |
| `frontend/src/features/settings/` | 前端配置界面 | +100 行 |
| **总计** | | **~250 行** |

### 核心代码结构

```python
# 1. 新增异常类型
class ThinkingSignatureException(UpstreamClientException):
    """Thinking 块签名验证失败异常"""

# 2. 签名错误识别
SIGNATURE_ERROR_PATTERNS = [
    "Invalid `signature` in `thinking` block",
    "thinking.signature: Field required",
    "messages.*.content.*.thinking.signature",
]

def _is_signature_error(error_text: str) -> bool:
    """检测是否为签名相关错误"""

# 3. 清洗函数
def _sanitize_thinking_blocks(
    messages: List[Dict],
    mode: str = "convert_to_text",
    prefix: str = "[Thinking] "
) -> Tuple[List[Dict], bool]:
    """
    清洗消息中的 thinking 块

    Returns:
        (清洗后的消息列表, 是否有改动)
    """

# 4. Handler 集成
async def process_sync(...):
    # 检测会话标记
    needs_sanitization = self._check_sanitization_needed()

    if needs_sanitization:
        messages = _sanitize_thinking_messages(messages)
        # 继续正常请求流程
```

---

## 使用场景

### 场景 1：多供应商故障转移

**配置**：a、b、c（三方中转），d（GLM-4.7 兜底）

```
正常：a → b → c → d
签名错误：a(500) → b(签名错) → c(签名错) → d(成功)
    ↓
标记需要清洗
    ↓
下次请求：自动清洗 thinking → a/b/c 恢复工作
```

### 场景 2：供应商切换

用户在不同供应商之间切换时，自动适配签名差异。

### 场景 3：健康恢复

原供应商恢复后，清洗的请求可以无缝切回，继续使用。

---

## 效果预期

| 指标 | 优化前 | 优化后 |
|------|--------|--------|
| **多供应商兼容性** | ❌ 签名冲突 | ✅ 自动适配 |
| **故障转移** | ⚠️ 只能用 d | ✅ a/b/c 都可用 |
| **用户体验** | ❌ 需手动处理 | ✅ 无感切换 |
| **上下文保留** | - | ✅ 保留内容 |
| **性能影响** | - | ✅ 几乎无影响 |

---

## 风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| **语义变化** | thinking 从隐藏推理变为显式文本 | 默认关闭，用户自主选择 |
| **模型行为** | 可能影响输出风格 | 提示用户并保留配置选项 |
| **Token 成本** | thinking 内容计入输入 | 提示用户，可配置删除模式 |
| **合规问题** | 内部推理暴露 | 静默模式，不显示给用户 |

---

## 测试要点

### 功能测试

- [ ] 正常请求不触发清洗
- [ ] 签名错误后标记会话
- [ ] 下次请求正确清洗
- [ ] 清洗后 a/b/c 恢复工作
- [ ] 配置开关正常工作

### 兼容性测试

- [ ] 不同供应商组合
- [ ] 多轮对话场景
- [ ] 流式/非流式请求
- [ ] CLI/Web 请求

### 性能测试

- [ ] 清洗开销 < 10ms
- [ ] 不影响正常请求延迟
- [ ] 内存占用可忽略

---

## 实施计划

### Phase 1: 核心功能（1-2 天）
- [ ] 异常类型定义
- [ ] 签名错误识别
- [ ] 清洗逻辑实现
- [ ] 配置项添加

### Phase 2: 集成测试（1 天）
- [ ] 单元测试
- [ ] 集成测试
- [ ] 边界情况测试

### Phase 3: 前端界面（1 天）
- [ ] 配置界面
- [ ] 状态显示
- [ ] 帮助文档

### Phase 4: 文档和发布（1 天）
- [ ] 用户文档
- [ ] 发布说明
- [ ] 监控指标

**总计**：4-5 个工作日

---

## 总结

这个优化方案通过**智能清洗 thinking 块**，解决了多供应商环境下的签名兼容性问题：

✅ **自动适配**：无需用户手动干预
✅ **向后兼容**：默认关闭，按需开启
✅ **无感切换**：清洗后自动恢复优先级
✅ **灵活配置**：支持多种清洗策略

**最终效果**：用户可以放心使用多个供应商，享受故障转移和负载均衡的便利，而不用担心签名冲突问题。

---

## 附录

### A. 相关代码文件

- `src/core/exceptions.py`
- `src/services/orchestration/error_classifier.py`
- `src/api/handlers/base/chat_handler_base.py`
- `src/config/settings.py`
- `src/models/claude.py`

### B. 参考文档

- [Claude Extended Thinking Documentation](https://docs.anthropic.com/claude/docs/extended-thinking)
- [Claude API Message Format](https://docs.anthropic.com/claude/docs/messages-overview)

### C. 常见问题

**Q: 清洗后 thinking 内容会暴露给用户吗？**
A: 不会。清洗只在服务端进行，用户不可见。如果使用聊天界面，thinking 块原本就不会显示。

**Q: 会增加 Token 成本吗？**
A: 会。因为 thinking 从"隐藏推理"变成了"显式文本"，会计入输入 Token。如果担心成本，可以使用 `remove` 模式直接删除。

**Q: 会影响模型输出质量吗？**
A: 可能会有轻微影响。thinking 从隐藏变为显式后，模型可能被引导。但通常影响很小。

**Q: 可以只对特定供应商清洗吗？**
A: 当前版本是全局清洗。如果需要更细粒度的控制，可以在配置中添加供应商白名单。

---

**文档版本**：v1.0
**创建日期**：2025-01-20
**最后更新**：2025-01-20
**维护者**：Aether Team
