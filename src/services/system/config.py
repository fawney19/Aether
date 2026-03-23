"""
系统配置服务
"""

from __future__ import annotations

import json
import time
from enum import Enum
from typing import Any

from sqlalchemy.orm import Session

from src.core.logger import logger
from src.core.validators import PasswordPolicyLevel
from src.models.database import Provider, SystemConfig

REQUEST_RECORD_LEVEL_KEY = "request_record_level"
_LEGACY_REQUEST_LOG_LEVEL_KEY = "request_log_level"


class RequestRecordLevel(str, Enum):
    """请求记录级别（控制请求/响应详情入库）"""

    BASIC = "basic"  # 仅记录基本信息（tokens、成本等）
    HEADERS = "headers"  # 记录基本信息+请求/响应头（敏感信息会脱敏）
    FULL = "full"  # 记录完整请求和响应（包含body，敏感信息会脱敏）


# 进程内缓存 TTL（秒）- 系统配置变化不频繁，使用较长的 TTL
_CONFIG_CACHE_TTL = 60  # 1 分钟

# 调度相关配置使用更短的缓存 TTL，确保多 Worker 部署时快速收敛
# 当管理员切换调度模式时，其他 Worker 最多延迟 5 秒即可感知变更
_SCHEDULING_CONFIG_CACHE_TTL = 5  # 5 秒

# 需要跨 Worker 快速同步的调度相关配置 key
SCHEDULING_CONFIG_KEYS = frozenset({"scheduling_mode", "provider_priority_mode"})

# 进程内缓存存储: {key: (value, expire_time)}
_config_cache: dict[str, tuple[Any, float]] = {}


def _get_cached_config(key: str) -> tuple[bool, Any]:
    """从进程内缓存获取配置值

    Returns:
        (hit, value): hit=True 表示缓存命中，value 为缓存的值
    """
    if key in _config_cache:
        value, expire_time = _config_cache[key]
        if time.time() < expire_time:
            return True, value
        # 缓存过期，安全删除（避免并发时 KeyError）
        _config_cache.pop(key, None)
    return False, None


def _set_cached_config(key: str, value: Any) -> None:
    """设置进程内缓存

    调度相关配置使用更短的 TTL（5秒），确保多 Worker 部署时快速收敛。
    """
    ttl = _SCHEDULING_CONFIG_CACHE_TTL if key in SCHEDULING_CONFIG_KEYS else _CONFIG_CACHE_TTL
    _config_cache[key] = (value, time.time() + ttl)


def invalidate_config_cache(key: str | None = None) -> None:
    """清除配置缓存

    Args:
        key: 配置键，如果为 None 则清除所有缓存
    """
    global _config_cache
    if key is None:
        _config_cache = {}
        logger.debug("已清除所有系统配置缓存")
    else:
        # 使用 pop 安全删除，避免并发时 KeyError
        if _config_cache.pop(key, None) is not None:
            logger.debug(f"已清除系统配置缓存: {key}")


class SystemConfigService:
    """系统配置服务类"""

    # 默认配置
    DEFAULT_CONFIGS = {
        # 站点信息
        "site_name": {
            "value": "Aether",
            "description": "站点名称，显示在页面标题、导航栏、登录页面和邮件中",
        },
        "site_subtitle": {
            "value": "AI Gateway",
            "description": "站点副标题，显示在导航栏品牌名称下方",
        },
        "default_user_initial_gift_usd": {
            "value": 10.0,
            "description": "新用户默认初始赠款（美元）",
        },
        "password_policy_level": {
            "value": PasswordPolicyLevel.WEAK.value,
            "description": "密码策略等级：weak(弱密码), medium(中等强度), strong(强密码)",
        },
        REQUEST_RECORD_LEVEL_KEY: {
            "value": RequestRecordLevel.BASIC.value,
            "description": "请求记录级别：basic(基本信息), headers(含请求/响应头), full(完整请求/响应)",
        },
        "max_request_body_size": {
            "value": 5242880,  # 5MB
            "description": "最大请求体记录大小（字节），超过此大小的请求体将被截断（仅影响数据库记录，不影响真实API请求）",
        },
        "max_response_body_size": {
            "value": 5242880,  # 5MB
            "description": "最大响应体记录大小（字节），超过此大小的响应体将被截断（仅影响数据库记录，不影响真实API响应）",
        },
        "sensitive_headers": {
            "value": ["authorization", "x-api-key", "api-key", "cookie", "set-cookie"],
            "description": "敏感请求头列表，这些请求头会被脱敏处理",
        },
        # 分级清理策略
        "detail_log_retention_days": {
            "value": 7,
            "description": "详细日志保留天数，超过此天数后压缩 request_body 和 response_body 到压缩字段",
        },
        "compressed_log_retention_days": {
            "value": 30,
            "description": "压缩记录保留天数，超过此天数后删除压缩的 body 字段（保留headers和统计）",
        },
        "header_retention_days": {
            "value": 90,
            "description": "请求头保留天数，超过此天数后清空 request_headers 和 response_headers 字段",
        },
        "log_retention_days": {
            "value": 365,
            "description": "请求记录保存天数，超过此天数后删除整条使用记录",
        },
        "enable_auto_cleanup": {
            "value": True,
            "description": "是否启用自动清理任务，每天凌晨执行分级清理",
        },
        "cleanup_batch_size": {
            "value": 1000,
            "description": "每批次清理的记录数，避免单次操作过大影响数据库性能",
        },
        "request_candidates_retention_days": {
            "value": 30,
            "description": "请求候选记录保留天数，超过此天数的 request_candidates 审计记录将被自动清理",
        },
        "request_candidates_cleanup_batch_size": {
            "value": 5000,
            "description": "请求候选记录每批次清理条数，使用独立批次控制大表删除压力",
        },
        "enable_provider_checkin": {
            "value": True,
            "description": "是否启用 Provider 自动签到任务",
        },
        "provider_checkin_time": {
            "value": "01:05",
            "description": "Provider 自动签到执行时间（HH:MM 格式，24小时制）",
        },
        "provider_priority_mode": {
            "value": "provider",
            "description": "优先级策略：provider(提供商优先模式) 或 global_key(全局Key优先模式)",
        },
        "scheduling_mode": {
            "value": "cache_affinity",
            "description": "调度模式：fixed_order(固定顺序模式，严格按优先级顺序) 或 cache_affinity(缓存亲和模式，优先使用已缓存的Provider)",
        },
        "auto_delete_expired_keys": {
            "value": False,
            "description": "是否自动删除过期的API Key（True=物理删除，False=仅禁用），仅管理员可配置",
        },
        "email_suffix_mode": {
            "value": "none",
            "description": "邮箱后缀限制模式：none(不限制), whitelist(白名单), blacklist(黑名单)",
        },
        "email_suffix_list": {
            "value": [],
            "description": "邮箱后缀列表，配合 email_suffix_mode 使用",
        },
        # 格式转换配置
        "enable_format_conversion": {
            "value": True,
            "description": "格式转换总开关：开启时允许跨格式转换；关闭时禁止任何跨格式转换",
        },
        "keep_priority_on_conversion": {
            "value": False,
            "description": "格式转换时保持优先级：开启时需要转换的候选保持原优先级；关闭时降级到不需要转换的候选之后",
        },
        "audit_log_retention_days": {
            "value": 30,
            "description": "审计日志保留天数，超过此天数的审计日志将被自动清理",
        },
        "enable_db_maintenance": {
            "value": True,
            "description": "是否启用数据库表维护任务（定期 VACUUM ANALYZE 防止表和索引膨胀）",
        },
        "redis_memory_total_bytes": {
            "value": 0,
            "description": "Redis 总内存容量（字节）。适用于远程或托管 Redis 无法完整上报容量时手动填写，0 表示自动探测",
        },
        "postgres_storage_total_bytes": {
            "value": 0,
            "description": "PostgreSQL 总存储空间（字节）。适用于远程或托管 PostgreSQL 无法直接获取总空间时手动填写，0 表示未设置",
        },
        # 系统代理
        "system_proxy_node_id": {
            "value": None,
            "description": "系统默认代理节点 ID，为空时直连。仅影响提供商出站请求（大模型API/余额查询/OAuth），不影响系统内部接口",
        },
        # SMTP 邮件配置
        "smtp_host": {
            "value": None,
            "description": "SMTP 服务器地址",
        },
        "smtp_port": {
            "value": 587,
            "description": "SMTP 服务器端口",
        },
        "smtp_user": {
            "value": None,
            "description": "SMTP 用户名",
        },
        "smtp_password": {
            "value": None,
            "description": "SMTP 密码（加密存储）",
        },
        "smtp_use_tls": {
            "value": True,
            "description": "是否使用 STARTTLS",
        },
        "smtp_use_ssl": {
            "value": False,
            "description": "是否使用 SSL/TLS",
        },
        "smtp_from_email": {
            "value": None,
            "description": "发件人邮箱地址",
        },
        "smtp_from_name": {
            "value": "Aether",
            "description": "发件人名称",
        },
        # OAuth Token 刷新配置
        "enable_oauth_token_refresh": {
            "value": True,
            "description": "是否启用 OAuth Token 自动刷新任务，主动刷新即将过期的 OAuth token",
        },
    }

    @classmethod
    def get_config(cls, db: Session, key: str, default: Any | None = None) -> Any | None:
        """获取系统配置值（带进程内缓存）"""
        # Backward-compatible alias: request_log_level -> request_record_level
        if key in {REQUEST_RECORD_LEVEL_KEY, _LEGACY_REQUEST_LOG_LEVEL_KEY}:
            value = cls._get_request_record_level_raw(db)
            if value is not None:
                return value
            if REQUEST_RECORD_LEVEL_KEY in cls.DEFAULT_CONFIGS:
                value = cls.DEFAULT_CONFIGS[REQUEST_RECORD_LEVEL_KEY]["value"]
                _set_cached_config(REQUEST_RECORD_LEVEL_KEY, value)
                return value
            return default

        # 1. 检查进程内缓存
        hit, cached_value = _get_cached_config(key)
        if hit:
            return cached_value

        # 2. 查询数据库
        config = db.query(SystemConfig).filter(SystemConfig.key == key).first()
        if config:
            _set_cached_config(key, config.value)
            return config.value

        # 3. 如果配置不存在，使用默认值
        if key in cls.DEFAULT_CONFIGS:
            value = cls.DEFAULT_CONFIGS[key]["value"]
            _set_cached_config(key, value)
            return value

        return default

    @classmethod
    def get_configs(cls, db: Session, keys: list[str]) -> dict[str, Any]:
        """
        批量获取系统配置值

        Args:
            db: 数据库会话
            keys: 配置键列表

        Returns:
            配置键值字典
        """
        result = {}

        # 一次查询获取所有配置
        configs = db.query(SystemConfig).filter(SystemConfig.key.in_(keys)).all()
        config_map = {c.key: c.value for c in configs}

        # 填充结果，不存在的使用默认值
        for key in keys:
            if key in config_map:
                result[key] = config_map[key]
            elif key in cls.DEFAULT_CONFIGS:
                result[key] = cls.DEFAULT_CONFIGS[key]["value"]
            else:
                result[key] = None

        return result

    @staticmethod
    def set_config(
        db: Session, key: str, value: Any, description: str | None = None
    ) -> SystemConfig:
        """设置系统配置值"""
        if key == "password_policy_level":
            normalized = str(value).strip().lower() if value is not None else ""
            value = (
                PasswordPolicyLevel(normalized).value
                if normalized
                else PasswordPolicyLevel.WEAK.value
            )
        # Backward-compatible alias: request_log_level -> request_record_level
        if key in {REQUEST_RECORD_LEVEL_KEY, _LEGACY_REQUEST_LOG_LEVEL_KEY}:
            config = (
                db.query(SystemConfig).filter(SystemConfig.key == REQUEST_RECORD_LEVEL_KEY).first()
            )
            legacy = (
                db.query(SystemConfig)
                .filter(SystemConfig.key == _LEGACY_REQUEST_LOG_LEVEL_KEY)
                .first()
            )

            if config:
                config.value = value
                if description:
                    config.description = description
                # 如果同时存在旧 key，删除它避免混乱
                if legacy:
                    db.delete(legacy)
            elif legacy:
                # 原地迁移旧 key -> 新 key
                legacy.key = REQUEST_RECORD_LEVEL_KEY
                legacy.value = value
                if description:
                    legacy.description = description
                config = legacy
            else:
                config = SystemConfig(
                    key=REQUEST_RECORD_LEVEL_KEY, value=value, description=description
                )
                db.add(config)

            db.commit()
            db.refresh(config)

            invalidate_config_cache(REQUEST_RECORD_LEVEL_KEY)
            invalidate_config_cache(_LEGACY_REQUEST_LOG_LEVEL_KEY)
            return config

        config = db.query(SystemConfig).filter(SystemConfig.key == key).first()

        if config:
            # 更新现有配置
            config.value = value
            if description:
                config.description = description
        else:
            # 创建新配置
            config = SystemConfig(key=key, value=value, description=description)
            db.add(config)

        db.commit()
        db.refresh(config)

        # 清除缓存
        invalidate_config_cache(key)

        return config

    @staticmethod
    def get_password_policy_level(db: Session) -> str:
        """获取密码策略等级，异常值自动回退为弱策略。"""
        value = SystemConfigService.get_config(
            db, "password_policy_level", PasswordPolicyLevel.WEAK.value
        )
        return (
            PasswordPolicyLevel(value).value
            if value in PasswordPolicyLevel._value2member_map_
            else PasswordPolicyLevel.WEAK.value
        )

    @staticmethod
    def get_default_provider(db: Session) -> str | None:
        """
        获取系统默认提供商
        优先级：1. 管理员设置的默认提供商 2. 数据库中第一个可用提供商
        """
        # 首先尝试获取管理员设置的默认提供商
        default_provider = SystemConfigService.get_config(db, "default_provider")
        if default_provider:
            return default_provider

        # 如果没有设置，fallback到数据库中第一个可用提供商
        first_provider = db.query(Provider).filter(Provider.is_active == True).first()

        if first_provider:
            return first_provider.name

        return None

    @staticmethod
    def set_default_provider(db: Session, provider_name: str) -> SystemConfig:
        """设置系统默认提供商"""
        return SystemConfigService.set_config(
            db, "default_provider", provider_name, "系统默认提供商，当用户未设置个人提供商时使用"
        )

    # 敏感配置项，不返回实际值
    SENSITIVE_KEYS = {"smtp_password"}

    @classmethod
    def get_all_configs(cls, db: Session) -> list:
        """获取所有系统配置"""
        configs = db.query(SystemConfig).all()
        by_key = {c.key: c for c in configs}
        result = []
        for config in configs:
            # Hide legacy key in list; present as canonical key instead.
            if config.key == _LEGACY_REQUEST_LOG_LEVEL_KEY:
                if REQUEST_RECORD_LEVEL_KEY in by_key:
                    continue
                # Expose as canonical key name
                config_key = REQUEST_RECORD_LEVEL_KEY
            else:
                config_key = config.key

            item = {
                "key": config_key,
                "description": config.description,
                "updated_at": config.updated_at.isoformat(),
            }
            # 对敏感配置，只返回是否已设置的标志，不返回实际值
            if config.key in cls.SENSITIVE_KEYS:
                item["value"] = None
                item["is_set"] = bool(config.value)
            else:
                item["value"] = config.value
            result.append(item)
        return result

    @classmethod
    def delete_config(cls, db: Session, key: str) -> bool:
        """删除系统配置"""
        # Backward-compatible alias: request_log_level -> request_record_level
        if key in {REQUEST_RECORD_LEVEL_KEY, _LEGACY_REQUEST_LOG_LEVEL_KEY}:
            configs = (
                db.query(SystemConfig)
                .filter(
                    SystemConfig.key.in_([REQUEST_RECORD_LEVEL_KEY, _LEGACY_REQUEST_LOG_LEVEL_KEY])
                )
                .all()
            )
            if not configs:
                return False
            for c in configs:
                db.delete(c)
            db.commit()
            invalidate_config_cache(REQUEST_RECORD_LEVEL_KEY)
            invalidate_config_cache(_LEGACY_REQUEST_LOG_LEVEL_KEY)
            return True

        config = db.query(SystemConfig).filter(SystemConfig.key == key).first()
        if config:
            db.delete(config)
            db.commit()
            # 清除缓存
            invalidate_config_cache(key)
            return True
        return False

    @classmethod
    def init_default_configs(cls, db: Session) -> None:
        """初始化默认配置"""
        for key, default_config in cls.DEFAULT_CONFIGS.items():
            if not db.query(SystemConfig).filter(SystemConfig.key == key).first():
                config = SystemConfig(
                    key=key,
                    value=default_config["value"],
                    description=default_config["description"],
                )
                db.add(config)

        db.commit()
        logger.info("初始化默认系统配置完成")

    @classmethod
    def _get_request_record_level_raw(cls, db: Session) -> Any | None:
        """Raw value from DB/cache for request record level (supports legacy key)."""
        hit, cached_value = _get_cached_config(REQUEST_RECORD_LEVEL_KEY)
        if hit:
            return cached_value

        config = db.query(SystemConfig).filter(SystemConfig.key == REQUEST_RECORD_LEVEL_KEY).first()
        if config:
            _set_cached_config(REQUEST_RECORD_LEVEL_KEY, config.value)
            return config.value

        hit, cached_value = _get_cached_config(_LEGACY_REQUEST_LOG_LEVEL_KEY)
        if hit:
            _set_cached_config(REQUEST_RECORD_LEVEL_KEY, cached_value)
            return cached_value

        legacy = (
            db.query(SystemConfig).filter(SystemConfig.key == _LEGACY_REQUEST_LOG_LEVEL_KEY).first()
        )
        if legacy:
            _set_cached_config(_LEGACY_REQUEST_LOG_LEVEL_KEY, legacy.value)
            _set_cached_config(REQUEST_RECORD_LEVEL_KEY, legacy.value)
            return legacy.value

        return None

    @classmethod
    def get_request_record_level(cls, db: Session) -> RequestRecordLevel:
        """获取请求记录级别（控制请求/响应详情入库）"""
        level = cls.get_config(db, REQUEST_RECORD_LEVEL_KEY, RequestRecordLevel.BASIC.value)
        if isinstance(level, str):
            return RequestRecordLevel(level)
        return level

    @classmethod
    def get_log_level(cls, db: Session) -> RequestRecordLevel:
        """Deprecated: use get_request_record_level."""
        return cls.get_request_record_level(db)

    @classmethod
    def should_log_headers(cls, db: Session) -> bool:
        """是否应该记录请求头"""
        level = cls.get_request_record_level(db)
        return level in [RequestRecordLevel.HEADERS, RequestRecordLevel.FULL]

    @classmethod
    def should_log_body(cls, db: Session) -> bool:
        """是否应该记录请求体和响应体"""
        level = cls.get_request_record_level(db)
        return level == RequestRecordLevel.FULL

    @classmethod
    def should_mask_sensitive_data(cls, db: Session) -> bool:
        """是否应该脱敏敏感数据（始终脱敏）"""
        _ = db  # 保持接口一致性
        return True

    @classmethod
    def get_sensitive_headers(cls, db: Session) -> list:
        """获取敏感请求头列表"""
        return cls.get_config(db, "sensitive_headers", [])

    @classmethod
    def is_format_conversion_enabled(cls, db: Session) -> bool:
        """检查全局格式转换是否启用"""
        return bool(cls.get_config(db, "enable_format_conversion", True))

    @classmethod
    def is_keep_priority_on_conversion(cls, db: Session) -> bool:
        """检查格式转换时是否保持优先级"""
        return bool(cls.get_config(db, "keep_priority_on_conversion", False))

    @classmethod
    def mask_sensitive_headers(cls, db: Session, headers: dict[str, Any]) -> dict[str, Any]:
        """脱敏敏感请求头"""
        if not cls.should_mask_sensitive_data(db):
            return headers

        sensitive_headers = cls.get_sensitive_headers(db)
        sensitive_lower = {h.lower() for h in sensitive_headers if isinstance(h, str) and h}
        masked_headers = {}

        for key, value in headers.items():
            if key.lower() in sensitive_lower:
                # 保留前后各4个字符，中间用星号替换
                if len(str(value)) > 8:
                    masked_value = str(value)[:4] + "****" + str(value)[-4:]
                else:
                    masked_value = "****"
                masked_headers[key] = masked_value
            else:
                masked_headers[key] = value

        return masked_headers

    @classmethod
    def truncate_body(cls, db: Session, body: Any, is_request: bool = True) -> Any:
        """截断过大的请求体或响应体"""
        max_size_key = "max_request_body_size" if is_request else "max_response_body_size"
        max_size = cls.get_config(db, max_size_key, 5242880)  # 5MB

        if not body:
            return body

        # 转换为字符串以计算大小
        body_str = json.dumps(body) if isinstance(body, (dict, list)) else str(body)

        if len(body_str) > max_size:
            # 截断并添加提示
            truncated_str = body_str[:max_size]
            if isinstance(body, (dict, list)):
                try:
                    # 尝试保持JSON格式
                    return {
                        "_truncated": True,
                        "_original_size": len(body_str),
                        "_content": truncated_str,
                    }
                except:
                    pass
            return truncated_str + f"\n... (truncated, original size: {len(body_str)} bytes)"

        return body
