# 实施计划：按 Provider 精细限制可用渠道 Key

## 1. 需求理解与现状

用户希望在现有“允许哪些 Provider”的限制之下继续限制“该 Provider 允许使用哪些 `ProviderAPIKey`”。目标对象包括：

- 独立余额 API Key（`ApiKey.is_standalone=True`）；
- 用户/用户分组级限制（当前代码没有独立 `UserGroup` 表，管理页所谓用户分组实际落在 `User` 的访问限制字段上）；
- 运行时所有请求候选，包括普通候选和 Provider 号池候选。

当前相关链路：

- `User` 和 `ApiKey` 只有 `allowed_providers`、`allowed_models`、`allowed_api_formats`；
- `src/services/scheduling/restriction_checker.py` 在调度时将 API Key 与 User 的限制取交集；
- `CacheAwareScheduler.list_all_candidates()` 先按 Provider 限制筛选；
- `CandidateBuilder._build_candidates()` 从 `provider.api_keys` 生成候选，号池模式会把多个 key 放入 `PoolCandidate.pool_keys`，之后由池调度器选择；
- `/v1/models` 及用户模型列表使用另一套 `AccessRestrictions`/`ModelAvailability` 逻辑；
- `ApiKeyProviderMapping` 是旧的 API Key→Provider 映射/优先级表，不能直接表达 Provider 内部 key 白名单，也不应把它与本需求混用。

## 2. 明确的数据语义

新增字段建议命名为 `allowed_provider_keys`，类型为 nullable JSON object，写入 `users` 和 `api_keys` 两张表：

```json
{
  "provider-id-a": ["provider-key-id-1", "provider-key-id-2"],
  "provider-id-b": []
}
```

语义固定如下：

- `null`：不启用渠道 Key 限制，所有允许 Provider 下的活跃 key 都可用；
- 缺少某个 Provider 的映射：该 Provider 下不限制具体 key；
- `provider_id: []`：该 Provider 下的所有 key 都禁止；
- `provider_id: [key_id...]`：该 Provider 下仅允许列出的 key；
- `allowed_providers` 仍是 Provider 层白名单，二者同时存在时必须同时满足；key 映射不能扩大 Provider 白名单；
- User 与 API Key 都有配置时按“限制只能收窄、不能放宽”处理：同一 Provider 两边都限制时取 key ID 交集，一边未配置该 Provider 时使用另一边配置；顶层 `null` 表示该层没有 key 限制。

只保存 Provider Key ID，不保存明文凭据，也不使用 Provider 名称作为映射键。配置保存时应去重、规范化字符串并校验 key 确实存在且属于映射中的 Provider；key 暂停使用仍允许保留配置，以便重新启用后生效。

## 3. 后端数据模型与迁移

1. 修改 `src/models/database.py`：
   - 在 `User` 和 `ApiKey` 的访问限制字段旁增加 `allowed_provider_keys = Column(JSON, nullable=True)`；
   - 注释说明 `null`、缺失 Provider、空数组三态语义；
   - 保持 `ApiKey.provider_mappings` 旧关系不变。
2. 新增最新 head 的 Alembic 增量迁移（`alembic/versions/`）：
   - 为 `users`、`api_keys` 增加 nullable JSON 列；
   - `downgrade()` 删除两列；
   - 遵循当前迁移链最新 revision，不能另起分支；
   - 不回填历史数据，旧记录 `NULL` 自动保持无限制行为。
3. 更新 `src/models/api.py`、`src/models/admin_requests.py` 的创建/更新/响应 schema，增加字段类型、描述和统一校验器；必要时保留 `ApiKeyResponse` 对已有字段兼容。

建议在 `src/core/access_restrictions.py` 或相邻的共享访问控制模块中集中实现：

- mapping 的 normalize/validate 辅助函数；
- User/API Key mapping 合并函数；
- `is_provider_key_allowed(provider_id, provider_key_id)`；
- 删除 Provider/Key 时的 prune 函数。

避免在路由、调度器和前端各自实现一套空值语义。

## 4. 管理 API、服务和导入导出

### 4.1 独立密钥与普通 API Key

修改：

- `src/services/user/apikey.py`：`create_api_key()` 接收并写入新字段；`update_api_key()` 将其加入可更新字段及 nullable 字段集合；
- `src/api/admin/api_keys/routes.py`：独立 Key 的创建、更新、列表/详情序列化返回 `allowed_provider_keys`；adapter 透传空对象、空数组和 `null`，不能用 truthiness 丢失 `[]`；
- `src/api/admin/users/routes.py`：用户 API Key 的创建/更新 adapter 同样支持字段，响应序列化补齐字段；
- `src/models/api.py` 的 `CreateApiKeyRequest` 复用共享校验。

如果普通用户自己的 API Key 页面暂时不提供该编辑能力，至少保证后端字段可导入、导出和管理员 API 完整支持，避免接口契约不一致。

### 4.2 User/“用户分组”管理

修改：

- `src/services/user/service.py` 的 `create_user()`、`update_user()` 和允许更新字段；
- `src/api/admin/users/routes.py` 的用户创建、更新、序列化；
- 所有系统用户导入/导出路径（`src/api/admin/system.py`）：`_serialize_api_key`、用户导出对象、API Key 导入构造、用户创建/覆盖更新都传递新字段；导出版本按现有兼容策略递增或保持向后兼容；
- 前端 API 类型 `frontend/src/api/users.ts`、`frontend/src/api/admin.ts` 以及相关响应类型。

### 4.3 渠道 Key 选项接口

现有 `getKeysGroupedByFormat()` 返回按 API 格式重复的 key 列表，不适合作为权限编辑器的唯一数据源。建议在 `src/api/admin/endpoints/keys.py` 和 `src/services/provider_keys/key_query_service.py` 增加一个面向权限配置的查询接口，例如：

`GET /api/admin/endpoints/keys/access-options`

返回活跃/停用 Provider 及其全部 ProviderAPIKey 的非敏感选项：`provider_id`、`provider_name`、`key_id`、`key_name`、`is_active`、`api_formats`。不返回凭据或可逆密文。前端用它构造 Provider→Key 的级联选择，并在编辑已有配置时保留已停用 key 的显示。

保存时后端仍必须重新查询校验，不能信任前端选项。

## 5. 访问限制合并与实际调度

### 5.1 统一有效限制

修改 `src/services/scheduling/restriction_checker.py`：

- 返回 `allowed_provider_keys`；
- 实现按 Provider 的 mapping 合并；
- 明确 `null` 与 `[]` 不可混淆；
- 继续与 `allowed_providers`、`allowed_api_formats`、`allowed_models` 取交集。

同步修改 `src/core/access_restrictions.py`，使 `/v1/models` 的限制语义与调度热路径一致，新增 provider-key 检查方法。现有测试中“API Key 限制优先于 User 限制”的断言需要重新核对并改为不会放宽用户限制的统一规则，避免模型列表显示可用但实际请求被调度器过滤，或相反。

### 5.2 CandidateBuilder 普通 key 和号池 key

修改 `src/services/scheduling/aware_scheduler.py`、`src/services/scheduling/candidate_builder.py`：

1. Scheduler 从有效限制中取 `allowed_provider_keys`，仍先做 Provider 过滤；
2. CandidateBuilder 在按 `is_active`、`api_formats` 筛选后，按当前 Provider ID应用 key 白名单；
3. 普通模式只为允许的 key 创建 `ProviderCandidate`；
4. 号池模式在创建 `pool_keys` 之前先过滤，不能把禁止 key 传给 `PoolManager`；全部被过滤时跳过该 Provider/Endpoint；
5. 过滤必须发生在 shuffle、缓存亲和、LRU、额度检查之前，确保这些排序/选择机制无法重新选回禁止 key；
6. `preferred_key_ids` 只允许在已过滤候选中置顶，不得成为权限绕过；
7. 日志只记录 key ID 前缀/名称等非敏感信息，避免凭据泄露。

检查 `src/services/orchestration/candidate_resolver.py`、`src/services/orchestration/request_dispatcher.py`、`src/services/provider/pool/` 的候选后续流程，确保 `PoolCandidate.pool_keys` 的所有入口都已过滤。

## 6. 模型列表、可用性和旁路路径

### 6.1 `/v1/models` 与用户模型列表

现有 `src/api/base/models_service.py`、`src/services/model/availability.py` 的可用 Provider/模型计算只知道 Provider 和 key 的模型/格式权限，不知道用户级 key 白名单。应：

- 让 `get_available_provider_ids()`、`_get_available_model_ids_for_format()`、`ModelAvailabilityQuery.get_provider_key_rules()` 接收有效 key 限制或 `AccessRestrictions`；
- `get_provider_key_rules()` 返回/过滤 key ID，使“Provider 仅剩禁止 key”时不会被认为可提供模型；
- `src/api/public/models.py` 的所有模型列表/详情入口传入限制；
- `src/services/user/service.py::get_user_available_models()` 使用同一 helper，避免管理/用户模型页与公开接口不一致；
- 受限请求不要命中不带权限维度的全局缓存，必要时扩展缓存 key 或继续禁用受限场景缓存。

### 6.2 直接使用历史 Provider Key 的路径

审计并补权限检查：

- `src/api/public/gemini_files.py` 视频任务下载 `_find_video_task_by_id()` 会根据历史 `VideoTask.key_id` 直接解密 key；调用处必须携带当前认证的 User/API Key 限制，在返回凭据前确认 Provider/key 仍被允许；
- `src/services/task/video/poller_adapter.py` 等后台任务是服务内部执行，不应把用户请求权限扩大，但要确认任务创建时已由过滤后的候选产生，必要时记录权限快照/在执行前再次校验；
- 普通 Gemini 文件上下文已经走 CandidateResolver，确认新过滤链覆盖该入口；
- `src/services/orchestration/request_dispatcher.py` 只接受已经过滤的候选，不新增可绕过白名单的直接 key 查询。

## 7. 删除、缓存和一致性清理

1. 修改 `src/services/provider/delete_cleanup.py`：
   - Provider 删除时从 `User.allowed_provider_keys`、`ApiKey.allowed_provider_keys` 删除整个 Provider 映射；
   - 保留其他 Provider 的配置；
   - 与现有 `allowed_providers` 清理一起执行并纳入统计/日志。
2. 修改 `src/services/provider_keys/key_side_effects.py` 或共享清理模块：
   - 删除单个 `ProviderAPIKey` 时，从所有 User/API Key 对应 Provider 的白名单数组移除 key ID；
   - 若数组只剩空数组，保留空数组表示该 Provider 仍然全部禁用；若整个 mapping 无条目可删除该 Provider 项；
   - 批量删除路径 `batch_delete_task.py` 必须批量处理，避免逐条 N+1。
3. 修改 `src/services/cache/user_cache.py`：当前 `_user_to_dict()` / `_dict_to_user()` 没有缓存 `allowed_providers`、`allowed_api_formats`、`allowed_models`，会导致缓存命中时丢失限制；应一并补上新旧全部访问限制字段，并验证更新用户后的失效行为。
4. 更新 Provider/用户/模型相关缓存失效，确保白名单变更立即影响下一次请求和模型列表。

## 8. 前端交互

修改：

- `frontend/src/features/api-keys/components/StandaloneKeyFormDialog.vue`；
- `frontend/src/features/users/components/UserFormDialog.vue`；
- `frontend/src/views/admin/ApiKeys.vue`、`frontend/src/views/admin/Users.vue` 的表单装载、编辑回填、提交 payload；
- `frontend/src/api/admin.ts`、`frontend/src/api/users.ts` 和新增 access-options API 类型/请求函数。

交互建议：

- 保留现有 Provider unrestricted 开关；
- Provider 选择后显示按 Provider 分组的 key 多选控件；
- 每个 Provider 提供“不限制该 Provider 的 Key”状态、允许选择一个或多个 key、以及“全部禁用”对应的空数组状态；
- Provider 未被允许时禁用/清空对应 key 选择，提交前再次规整；
- 编辑时加载 `allowed_provider_keys`，即便某 key 已停用也显示“已停用”标签，避免无意丢配置；
- `null`、`{}`、`provider: []` 分别保持各自语义，不能统一转成空数组；
- key 选项按 Provider 过滤，不让用户看到其他 Provider 的 key；
- 遵循已有 `MultiSelect`、`Switch`、Provider/模型选项加载模式，桌面双栏布局在窄屏下保持可用。

如不新增专用树形控件，可使用现有 MultiSelect 按 Provider 分段渲染；重点是提交结构清晰且可表达三态，而不是引入新的 UI 依赖。

## 9. 测试计划

### 后端单元/契约测试

新增或修改：

- `tests/services/test_model_availability.py`：mapping 规范化、有效 key 过滤、Provider 只有被禁止 key 时不可用；
- `tests/services/test_aware_scheduler_pool_candidate_enumeration.py`：普通 Provider 候选、号池 `pool_keys`、空数组阻断、多个 Provider 不互相污染；
- `tests/contracts/test_scheduler_provider_prefilter_contract.py` 及 `test_scheduler_list_all_candidates_contract.py`：Provider + key 双层过滤；
- `tests/services/test_model_availability.py` 或新建访问限制测试：User/API Key 交集、同 Provider key ID 交集、`null`/缺失/`[]` 三态；
- `tests/api/test_admin_api_key_scope_routes.py`、`tests/api/test_admin_user_routes.py`：创建/更新/序列化保留非空、空对象、空数组、null，非法 Provider/key 归属返回 4xx；
- `tests/services/test_provider_delete_cleanup.py`：删除 Provider 清理映射、删除单个/批量 ProviderAPIKey 清理 key ID 且保留空数组语义；
- `tests/unit/test_admin_system_users_export_import.py`：导入导出新字段并保持旧版本数据兼容；
- 新增 `tests/unit/test_user_cache_restrictions.py`：用户缓存序列化往返不丢访问限制；
- `tests/api` 或 `tests/e2e`：Gemini 视频任务下载在 key 被用户/独立 Key 限制拒绝后不能解密/转发；
- 更新 `tests/services/test_model_availability.py` 中现有 `AccessRestrictions` 期望，使公开模型列表与 scheduler 的限制合并规则一致。

### 前端验证

- 为提取出的纯 mapping 转换/三态序列化 helper 增加 Vitest（如放在 `frontend/src/features/api-keys` 或 `frontend/src/features/users` 的 `__tests__`）；
- 至少覆盖 Provider 选择、多个 key、空数组、null、编辑回填及停用 key 保留；
- 执行 `cd frontend && npm run type-check && npm run test:run`，按仓库指南尽量执行 `npm run lint`。

### 命令验收

- `uv run pytest <相关测试文件>`；
- 依赖可用时执行完整 `uv run pytest`；
- `uv run alembic heads`/`uv run alembic upgrade head`（当前环境中 `uv run alembic heads` 已因环境缺少 `alembic` 可执行文件失败，实施阶段需先确认依赖安装）；
- 前端 type-check/test/lint；
- 检查 `git diff`，确认没有提交凭据、`.env`、本地导出数据或无关文件。

## 10. 实施顺序与验收标准

1. 先落地共享类型、字段、迁移和校验函数；
2. 接入 User/API Key 服务、管理员 API、导入导出和 options API；
3. 接入有效限制合并及 CandidateBuilder 普通/号池候选过滤；
4. 接入模型列表/可用性和 Gemini/视频等旁路；
5. 接入删除清理、缓存字段和失效；
6. 完成前端配置 UI；
7. 运行针对性测试，再运行前端验证和可行的全量测试。

验收必须满足：

- Provider 允许但某个 key 不在白名单时，普通请求和号池请求都不会使用该 key；
- 多个 Provider 可以分别配置不同 key 白名单；
- `null`、缺少 Provider、空数组和非空数组行为可预测并在 UI/API/调度/模型列表一致；
- User 限制与独立 API Key 限制不会互相放宽；
- 删除 Provider 或 Provider key 后不会留下失效 ID 导致异常，也不会误删其他 Provider 配置；
- `/v1/models` 显示结果与实际可调用候选一致；
- 现有未配置该字段的记录行为完全保持为“不限制具体 key”。

## 11. 主要风险与待实现时确认项

- 现有 `AccessRestrictions.from_api_key_and_user()` 与 scheduler 的限制合并逻辑历史上并不完全一致，实施时必须以“限制只能收窄”为统一安全规则，并同步更新回归测试；
- 号池有延迟可用性检查，白名单过滤必须早于池对象构建，否则后续 Redis/LRU 选择仍可能命中被禁 key；
- ProviderAPIKey 删除和 Provider 删除是两套清理路径，必须分别覆盖同步、批量和异步任务；
- 用户缓存当前遗漏旧访问限制字段，若只添加新字段而不修复旧字段，缓存命中仍可能产生权限回退；
- 管理 options 接口必须返回停用 key 的元数据供编辑回填，但运行时只能使用 active key；
- 当前工作树已有 pi-herd 相关提交/分支状态，实施者应只修改本功能涉及文件，不回退既有用户或代理改动。

pi-herd-verdict: done pass=1 已完成源码调研并给出涵盖数据、API、调度号池、模型列表、旁路、前端、清理和测试的按渠道 Key 限制实施计划
