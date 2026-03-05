"""
服务器配置
从环境变量或 .env 文件加载配置
"""

import os
from pathlib import Path
from typing import Any

# 尝试加载 .env 文件
try:
    from dotenv import load_dotenv

    env_file = Path(".env")
    if env_file.exists():
        load_dotenv(env_file)
except ImportError:
    # 如果没有安装 python-dotenv，仍然可以从环境变量读取
    pass


class Config:
    def __init__(self) -> None:
        # 服务器配置
        self.host = os.getenv("HOST", "0.0.0.0")
        self.port = int(os.getenv("PORT", "8084"))
        self.log_level = os.getenv("LOG_LEVEL", "INFO")
        self.worker_processes = int(
            os.getenv("WEB_CONCURRENCY", os.getenv("GUNICORN_WORKERS", "4"))
        )

        # PostgreSQL 连接池计算相关配置
        # PG_MAX_CONNECTIONS: PostgreSQL 的 max_connections 设置（默认 100）
        # PG_RESERVED_CONNECTIONS: 为其他应用/管理工具预留的连接数（默认 10）
        self.pg_max_connections = int(os.getenv("PG_MAX_CONNECTIONS", "100"))
        self.pg_reserved_connections = int(os.getenv("PG_RESERVED_CONNECTIONS", "10"))

        # 数据库配置 - 延迟验证，支持测试环境覆盖
        self._database_url = os.getenv("DATABASE_URL")

        # JWT配置
        self.jwt_secret_key = os.getenv("JWT_SECRET_KEY", None)
        self.jwt_algorithm = os.getenv("JWT_ALGORITHM", "HS256")
        self.jwt_expiration_hours = int(os.getenv("JWT_EXPIRATION_HOURS", "24"))

        # 加密密钥配置（独立于JWT密钥，用于敏感数据加密）
        self.encryption_key = os.getenv("ENCRYPTION_KEY", None)

        # 环境配置 - 智能检测
        # Docker 部署默认为生产环境，本地开发默认为开发环境
        is_docker = (
            os.path.exists("/.dockerenv")
            or os.environ.get("DOCKER_CONTAINER", "false").lower() == "true"
        )
        default_env = "production" if is_docker else "development"
        self.environment = os.getenv("ENVIRONMENT", default_env)

        # Redis 依赖策略（生产默认必需，开发默认可选，可通过 REDIS_REQUIRED 覆盖）
        redis_required_env = os.getenv("REDIS_REQUIRED")
        if redis_required_env is not None:
            self.require_redis = redis_required_env.lower() == "true"
        else:
            # 保持向后兼容：开发环境可选，生产环境必需
            self.require_redis = self.environment not in {"development", "test", "testing"}

        # CORS配置 - 使用环境变量配置允许的源
        # 格式: 逗号分隔的域名列表,如 "http://localhost:3000,https://example.com"
        cors_origins = os.getenv("CORS_ORIGINS", "")
        if cors_origins:
            self.cors_origins = [
                origin.strip() for origin in cors_origins.split(",") if origin.strip()
            ]
        else:
            # 默认: 开发环境允许本地前端,生产环境不允许任何跨域
            if self.environment == "development":
                self.cors_origins = [
                    "http://localhost:3000",
                    "http://localhost:5173",  # Vite 默认端口
                    "http://127.0.0.1:3000",
                    "http://127.0.0.1:5173",
                ]
            else:
                # 生产环境默认不允许跨域,必须显式配置
                self.cors_origins = []

        # CORS是否允许凭证(Cookie/Authorization header)
        # 注意: allow_credentials=True 时不能使用 allow_origins=["*"]
        self.cors_allow_credentials = os.getenv("CORS_ALLOW_CREDENTIALS", "true").lower() == "true"

        # 管理员账户配置（用于初始化）
        self.admin_email = os.getenv("ADMIN_EMAIL", "admin@localhost")
        self.admin_username = os.getenv("ADMIN_USERNAME", "admin")

        # 管理员密码 - 必须在环境变量中设置
        admin_password_env = os.getenv("ADMIN_PASSWORD")
        if admin_password_env:
            self.admin_password = admin_password_env
        else:
            # 未设置密码，启动时会报错
            self.admin_password = ""
            self._missing_admin_password = True

        # API Key 配置
        self.api_key_prefix = os.getenv("API_KEY_PREFIX", "sk")

        # 支付回调安全配置（公开回调入口必须携带该共享密钥）
        self.payment_callback_secret = os.getenv("PAYMENT_CALLBACK_SECRET", "").strip()

        # LLM API 速率限制配置（每分钟请求数）
        self.llm_api_rate_limit = int(os.getenv("LLM_API_RATE_LIMIT", "100"))
        self.public_api_rate_limit = int(os.getenv("PUBLIC_API_RATE_LIMIT", "60"))

        # 异常处理配置
        # 设置为 True 时，ProxyException 会传播到路由层以便记录 provider_request_headers
        # 设置为 False 时，使用全局异常处理器统一处理
        self.propagate_provider_exceptions = (
            os.getenv("PROPAGATE_PROVIDER_EXCEPTIONS", "true").lower() == "true"
        )

        # 数据库连接池配置 - 智能自动调整
        # 系统会根据 Worker 数量和 PostgreSQL 限制自动计算安全值
        self.db_pool_size = int(os.getenv("DB_POOL_SIZE") or self._auto_pool_size())
        self.db_max_overflow = int(os.getenv("DB_MAX_OVERFLOW") or self._auto_max_overflow())
        self.db_pool_timeout = int(os.getenv("DB_POOL_TIMEOUT", "60"))
        self.db_pool_recycle = int(os.getenv("DB_POOL_RECYCLE", "3600"))
        self.db_pool_warn_threshold = int(os.getenv("DB_POOL_WARN_THRESHOLD", "70"))

        # 并发控制配置
        # CACHE_RESERVATION_RATIO: 缓存用户预留比例（默认 10%，新用户可用 90%）
        self.cache_reservation_ratio = float(os.getenv("CACHE_RESERVATION_RATIO", "0.1"))

        # RPM 计数器时间窗口配置
        from src.config.constants import RPMDefaults

        self.rpm_bucket_seconds = int(
            os.getenv("RPM_BUCKET_SECONDS", str(RPMDefaults.RPM_BUCKET_SECONDS))
        )
        self.rpm_key_ttl_seconds = int(
            os.getenv("RPM_KEY_TTL_SECONDS", str(RPMDefaults.RPM_KEY_TTL_SECONDS))
        )
        self.rpm_cleanup_interval_seconds = int(
            os.getenv("RPM_CLEANUP_INTERVAL_SECONDS", str(RPMDefaults.RPM_CLEANUP_INTERVAL_SECONDS))
        )

        # 限流降级策略配置
        # RATE_LIMIT_FAIL_OPEN: 当限流服务（Redis）异常时的行为
        #
        # True (默认): fail-open - 放行请求（优先可用性）
        #   风险：Redis 故障期间无法限流，可能被滥用
        #   适用：API 网关作为关键基础设施，必须保持高可用
        #
        # False: fail-close - 拒绝所有请求（优先安全性）
        #   风险：Redis 故障会导致 API 网关不可用
        #   适用：有严格速率限制要求的安全敏感场景
        self.rate_limit_fail_open = os.getenv("RATE_LIMIT_FAIL_OPEN", "true").lower() == "true"

        # HTTP 请求超时配置（秒）
        self.http_connect_timeout = float(os.getenv("HTTP_CONNECT_TIMEOUT", "10.0"))
        self.http_read_timeout = float(os.getenv("HTTP_READ_TIMEOUT", "3600.0"))
        self.http_write_timeout = float(os.getenv("HTTP_WRITE_TIMEOUT", "3600.0"))
        self.http_pool_timeout = float(os.getenv("HTTP_POOL_TIMEOUT", "10.0"))
        # HTTP_REQUEST_TIMEOUT: 非流式请求整体超时（秒），默认 300 秒
        self.http_request_timeout = float(os.getenv("HTTP_REQUEST_TIMEOUT", "300.0"))

        # HTTP 连接池配置
        # HTTP_MAX_CONNECTIONS: 最大连接数，影响并发能力
        #   - 每个连接占用一个 socket，过多会耗尽系统资源
        #   - 默认根据 Worker 数量自动计算：单 Worker 200，多 Worker 按比例分配
        # HTTP_KEEPALIVE_CONNECTIONS: 保活连接数，影响连接复用效率
        #   - 高频请求场景应该增大此值
        #   - 默认为 max_connections 的 30%（长连接场景更高效）
        # HTTP_KEEPALIVE_EXPIRY: 保活过期时间（秒）
        #   - 过短会频繁重建连接，过长会占用资源
        #   - 默认 30 秒，生图等长连接场景可适当增大
        self.http_max_connections = int(
            os.getenv("HTTP_MAX_CONNECTIONS") or self._auto_http_max_connections()
        )
        self.http_keepalive_connections = int(
            os.getenv("HTTP_KEEPALIVE_CONNECTIONS") or self._auto_http_keepalive_connections()
        )
        self.http_keepalive_expiry = float(os.getenv("HTTP_KEEPALIVE_EXPIRY", "30.0"))

        # 上游传输优化配置
        # ENABLE_HTTP2: 是否对上游请求启用 HTTP/2（HPACK 头部压缩 + 多路复用）
        #   - 三家上游（Claude/OpenAI/Gemini）均已确认支持 HTTP/2
        #   - 出现兼容性问题时可通过环境变量快速回退到 HTTP/1.1
        self.enable_http2 = os.getenv("ENABLE_HTTP2", "true").lower() == "true"

        # 流式处理配置
        # STREAM_PREFETCH_LINES: 预读行数，用于检测嵌套错误
        # STREAM_STATS_DELAY: 统计记录延迟（秒），等待流完全关闭
        # STREAM_FIRST_BYTE_TIMEOUT: 首字节超时（秒），等待首字节超过此时间触发故障转移
        self.stream_prefetch_lines = int(os.getenv("STREAM_PREFETCH_LINES", "5"))
        self.stream_stats_delay = float(os.getenv("STREAM_STATS_DELAY", "0.1"))
        self.stream_first_byte_timeout = float(os.getenv("STREAM_FIRST_BYTE_TIMEOUT", "30.0"))

        # Usage 队列配置（Redis Streams）
        # 默认启用队列模式，通过 Redis Streams 异步写入 DB，提升响应性能
        self.usage_queue_enabled = os.getenv("USAGE_QUEUE_ENABLED", "true").lower() == "true"
        # 队列事件是否包含 headers/bodies 由系统配置（request_record_level）决定；
        # 最终写入 DB 前仍会按 SystemConfigService 做脱敏与截断。
        self.usage_queue_stream_key = os.getenv("USAGE_QUEUE_STREAM_KEY", "usage:events")
        self.usage_queue_stream_group = os.getenv("USAGE_QUEUE_STREAM_GROUP", "usage_consumers")
        self.usage_queue_stream_maxlen = int(os.getenv("USAGE_QUEUE_STREAM_MAXLEN", "200000"))
        self.usage_queue_dlq_key = os.getenv("USAGE_QUEUE_DLQ_KEY", "usage:events:dlq")
        self.usage_queue_dlq_maxlen = int(os.getenv("USAGE_QUEUE_DLQ_MAXLEN", "5000"))
        self.usage_queue_consumer_batch = int(os.getenv("USAGE_QUEUE_CONSUMER_BATCH", "200"))
        self.usage_queue_consumer_block_ms = int(os.getenv("USAGE_QUEUE_CONSUMER_BLOCK_MS", "500"))
        self.usage_queue_claim_idle_ms = int(os.getenv("USAGE_QUEUE_CLAIM_IDLE_MS", "30000"))
        self.usage_queue_claim_interval_seconds = float(
            os.getenv("USAGE_QUEUE_CLAIM_INTERVAL_SECONDS", "5")
        )
        self.usage_queue_max_retries = int(os.getenv("USAGE_QUEUE_MAX_RETRIES", "2"))
        self.usage_queue_metrics_interval_seconds = float(
            os.getenv("USAGE_QUEUE_METRICS_INTERVAL_SECONDS", "30")
        )

        # Admin analytics query defaults (protect DB from unbounded scans)
        # ADMIN_USAGE_DEFAULT_DAYS:
        # - 0: keep current behavior (no implicit time filter)
        # - >0: when admin usage endpoints omit start_date/end_date, default to "last N days"
        default_admin_usage_default_days = (
            "0" if self.environment in {"development", "test", "testing"} else "30"
        )
        self.admin_usage_default_days = int(
            os.getenv("ADMIN_USAGE_DEFAULT_DAYS", default_admin_usage_default_days)
        )

        # Thinking 整流器配置
        # THINKING_RECTIFIER_ENABLED: 是否启用 Thinking 整流器
        #   当遇到跨 Provider 的 thinking 签名错误时，自动整流请求体后重试
        #   默认启用，设为 false 可禁用此功能
        self.thinking_rectifier_enabled = (
            os.getenv("THINKING_RECTIFIER_ENABLED", "true").lower() == "true"
        )

        # 请求体读取超时（秒）
        # REQUEST_BODY_TIMEOUT: 等待客户端发送完整请求体的超时时间
        #   默认 60 秒，防止客户端发送不完整请求导致连接卡死
        self.request_body_timeout = float(os.getenv("REQUEST_BODY_TIMEOUT", "60.0"))

        # 性能检测配置
        # PERF_METRICS_ENABLED: 是否启用性能指标上报（监控插件）
        # PERF_LOG_SLOW_MS: 慢请求日志阈值（毫秒），0 表示关闭
        # PERF_SAMPLE_RATE: 采样率 (0-1)，降低高频指标开销
        self.perf_metrics_enabled = os.getenv("PERF_METRICS_ENABLED", "false").lower() == "true"
        self.perf_log_slow_ms = int(os.getenv("PERF_LOG_SLOW_MS", "0"))
        self.perf_sample_rate = float(os.getenv("PERF_SAMPLE_RATE", "1.0"))
        # PERF_STORE_ENABLED: 是否将性能指标写入 Usage.request_metadata
        # PERF_STORE_SAMPLE_RATE: 存储采样率 (0-1)，用于降低写入压力
        self.perf_store_enabled = os.getenv("PERF_STORE_ENABLED", "false").lower() == "true"
        self.perf_store_sample_rate = float(os.getenv("PERF_STORE_SAMPLE_RATE", "1.0"))

        # 解密缓存配置（降低高频解密带来的CPU开销）
        # CRYPTO_DECRYPT_CACHE_ENABLED: 是否启用解密结果缓存
        # CRYPTO_DECRYPT_CACHE_SIZE: 最大缓存条目数
        # CRYPTO_DECRYPT_CACHE_TTL_SECONDS: 缓存TTL（秒）
        self.crypto_decrypt_cache_enabled = (
            os.getenv("CRYPTO_DECRYPT_CACHE_ENABLED", "true").lower() == "true"
        )
        self.crypto_decrypt_cache_size = int(os.getenv("CRYPTO_DECRYPT_CACHE_SIZE", "256"))
        self.crypto_decrypt_cache_ttl_seconds = float(
            os.getenv("CRYPTO_DECRYPT_CACHE_TTL_SECONDS", "60.0")
        )

        # 内部请求 User-Agent 配置（用于查询上游模型列表等）
        # 可通过环境变量覆盖默认值，模拟对应 CLI 客户端
        self.internal_user_agent_claude_cli = os.getenv(
            "CLAUDE_CLI_USER_AGENT", "claude-code/1.0.1"
        )
        self.internal_user_agent_openai_cli = os.getenv("OPENAI_CLI_USER_AGENT", "openai-codex/1.0")
        self.internal_user_agent_gemini_cli = os.getenv("GEMINI_CLI_USER_AGENT", "gemini-cli/0.1.0")

        # 邮箱验证配置
        # VERIFICATION_CODE_EXPIRE_MINUTES: 验证码有效期（分钟）
        # VERIFICATION_SEND_COOLDOWN: 发送冷却时间（秒）
        self.verification_code_expire_minutes = int(
            os.getenv("VERIFICATION_CODE_EXPIRE_MINUTES", "5")
        )
        self.verification_send_cooldown = int(os.getenv("VERIFICATION_SEND_COOLDOWN", "60"))

        # 计费系统配置（多维度计费 / 异步任务）
        # BILLING_REQUIRE_RULE: Video/Image/Audio 缺失 billing_rule 时是否拒绝请求（默认 false，缺失则 cost=0 并告警）
        # BILLING_STRICT_MODE: required 维度缺失时是否拒绝请求/标记任务失败（默认 false，缺失则 cost=0 + 标记 incomplete）
        self.billing_require_rule = os.getenv("BILLING_REQUIRE_RULE", "false").lower() == "true"
        self.billing_strict_mode = os.getenv("BILLING_STRICT_MODE", "false").lower() == "true"

        # Usage.request_metadata 体积控制（用于降低 DB/CPU/内存压力）
        # USAGE_METADATA_MAX_BYTES:
        # - 0: unlimited (backward compatible)
        # - >0: best-effort prune large keys when metadata JSON exceeds this size
        default_usage_metadata_max_bytes = (
            "0" if self.environment in {"development", "test", "testing"} else "65536"
        )
        self.usage_metadata_max_bytes = int(
            os.getenv("USAGE_METADATA_MAX_BYTES", default_usage_metadata_max_bytes)
        )

        # 视频任务轮询配置
        # VIDEO_POLL_INTERVAL_SECONDS: 轮询间隔（秒），默认 10 秒
        # VIDEO_MAX_POLL_COUNT: 最大轮询次数，默认 360 次（约 1 小时）
        # VIDEO_POLL_BATCH_SIZE: 每批处理任务数，默认 50
        # VIDEO_POLL_CONCURRENCY: 并发轮询数，默认 10
        self.video_poll_interval_seconds = int(os.getenv("VIDEO_POLL_INTERVAL_SECONDS", "10"))
        self.video_max_poll_count = int(os.getenv("VIDEO_MAX_POLL_COUNT", "360"))
        self.video_poll_batch_size = int(os.getenv("VIDEO_POLL_BATCH_SIZE", "50"))
        self.video_poll_concurrency = int(os.getenv("VIDEO_POLL_CONCURRENCY", "10"))

        # Management Token 速率限制（每分钟每 IP）
        self.management_token_rate_limit = int(os.getenv("MANAGEMENT_TOKEN_RATE_LIMIT", "30"))

        # 每个用户最多可创建的 Management Token 数量
        self.management_token_max_per_user = int(os.getenv("MANAGEMENT_TOKEN_MAX_PER_USER", "20"))

        # API 文档配置
        # DOCS_ENABLED: 是否启用 API 文档（/docs, /redoc, /openapi.json）
        #   - 未设置: 开发环境启用，生产环境禁用
        #   - true: 强制启用
        #   - false: 强制禁用
        docs_enabled_env = os.getenv("DOCS_ENABLED")
        if docs_enabled_env is not None:
            self.docs_enabled = docs_enabled_env.lower() == "true"
        else:
            # 默认：开发环境启用，生产环境禁用
            self.docs_enabled = self.environment == "development"

        # 验证连接池配置
        self._validate_pool_config()

    def _auto_pool_size(self) -> int:
        """
        智能计算连接池大小 - 根据 Worker 数量和 PostgreSQL 限制计算

        公式: (pg_max_connections - reserved) / workers / 2
        除以 2 是因为还要预留 max_overflow 的空间
        """
        available_connections = self.pg_max_connections - self.pg_reserved_connections
        # 每个 Worker 可用的连接数（pool_size + max_overflow）
        per_worker_total = available_connections // max(self.worker_processes, 1)
        # pool_size 取总数的一半，另一半留给 overflow
        pool_size = max(per_worker_total // 2, 5)  # 最小 5 个连接
        return min(pool_size, 30)  # 最大 30 个连接

    def _auto_max_overflow(self) -> int:
        """智能计算最大溢出连接数 - 与 pool_size 相同"""
        return self.db_pool_size

    def _auto_http_max_connections(self) -> int:
        """
        智能计算 HTTP 最大连接数

        计算依据:
        1. 系统 socket 资源有限（Linux 默认 ulimit -n 通常为 1024）
        2. 多 Worker 部署时每个进程独立连接池
        3. 需要为数据库连接、Redis 连接等预留资源

        公式: base_connections / workers
        - 单 Worker: 200 连接（适合开发/低负载）
        - 多 Worker: 按比例分配，确保总数不超过系统限制

        范围: 50 - 500
        """
        # 基础连接数：假设系统可用 socket 约 800 个用于 HTTP
        # （预留给 DB、Redis、内部服务等）
        base_connections = 800
        workers = max(self.worker_processes, 1)

        # 每个 Worker 分配的连接数
        per_worker = base_connections // workers

        # 限制范围：最小 50（保证基本并发），最大 500（避免资源耗尽）
        return max(50, min(per_worker, 500))

    def _auto_http_keepalive_connections(self) -> int:
        """
        智能计算 HTTP 保活连接数

        计算依据:
        1. 保活连接用于复用，减少 TCP 握手开销
        2. 对于 API 网关场景，上游请求频繁，保活比例应较高
        3. 生图等长连接场景，连接会被长时间占用

        公式: max_connections * 0.3
        - 30% 的比例在复用效率和资源占用间取得平衡
        - 长连接场景建议手动调高到 50-70%

        范围: 10 - max_connections
        """
        # 保活连接数为最大连接数的 30%
        keepalive = int(self.http_max_connections * 0.3)

        # 最小 10 个保活连接，最大不超过 max_connections
        return max(10, min(keepalive, self.http_max_connections))

    def _validate_pool_config(self) -> None:
        """验证连接池配置是否安全"""
        total_per_worker = self.db_pool_size + self.db_max_overflow
        total_all_workers = total_per_worker * self.worker_processes
        safe_limit = self.pg_max_connections - self.pg_reserved_connections

        if total_all_workers > safe_limit:
            # 记录警告（不抛出异常，避免阻止启动）
            self._pool_config_warning = (
                f"[WARN] 数据库连接池配置可能超过 PostgreSQL 限制: "
                f"{self.worker_processes} workers x {total_per_worker} connections = "
                f"{total_all_workers} > {safe_limit} (pg_max_connections - reserved). "
                f"建议调整 DB_POOL_SIZE 或 PG_MAX_CONNECTIONS 环境变量。"
            )
        else:
            self._pool_config_warning = None

    @property
    def database_url(self) -> str:
        """
        数据库 URL（延迟验证）

        在测试环境中可以通过依赖注入覆盖，而不会在导入时崩溃
        """
        if not self._database_url:
            raise ValueError(
                "DATABASE_URL environment variable is required. "
                "Example: postgresql://username:password@localhost:5432/dbname"
            )
        return self._database_url

    @database_url.setter
    def database_url(self, value: str) -> Any:
        """允许在测试中设置数据库 URL"""
        self._database_url = value

    def log_startup_warnings(self) -> None:
        """
        记录启动时的安全警告
        这个方法应该在 logger 初始化后调用
        """
        from src.core.logger import logger

        # 连接池配置警告
        if hasattr(self, "_pool_config_warning") and self._pool_config_warning:
            logger.warning(self._pool_config_warning)

        # 管理员密码检查（必须在环境变量中设置）
        if hasattr(self, "_missing_admin_password") and self._missing_admin_password:
            logger.error("必须设置 ADMIN_PASSWORD 环境变量！")
            raise ValueError("ADMIN_PASSWORD environment variable must be set!")

        # JWT 密钥警告
        if not self.jwt_secret_key:
            if self.environment == "production":
                logger.error(
                    "生产环境未设置 JWT_SECRET_KEY! 这是严重的安全漏洞。"
                    "使用 'python generate_keys.py' 生成安全密钥。"
                )
            else:
                logger.warning("JWT_SECRET_KEY 未设置，将使用默认密钥（仅限开发环境）")

        # 加密密钥警告
        if not self.encryption_key and self.environment != "production":
            logger.warning("ENCRYPTION_KEY 未设置，使用开发环境默认密钥。生产环境必须设置。")

        # CORS 配置警告（生产环境）
        if self.environment == "production" and not self.cors_origins:
            logger.warning("生产环境 CORS 未配置，前端将无法访问 API。请设置 CORS_ORIGINS。")
        if self.environment == "production" and not self.payment_callback_secret:
            logger.warning(
                "生产环境未设置 PAYMENT_CALLBACK_SECRET，支付回调将被拒绝。"
                "如需启用支付回调，请配置共享密钥。"
            )

    def validate_security_config(self) -> list[str]:
        """
        验证安全配置，返回错误列表
        生产环境会阻止启动，开发环境仅警告

        Returns:
            错误消息列表（空列表表示验证通过）
        """
        errors: list[str] = []

        if self.environment == "production":
            # 生产环境必须设置 JWT 密钥
            if not self.jwt_secret_key:
                errors.append(
                    "JWT_SECRET_KEY must be set in production. "
                    "Use 'python generate_keys.py' to generate a secure key."
                )
            elif len(self.jwt_secret_key) < 32:
                errors.append("JWT_SECRET_KEY must be at least 32 characters in production.")

            # 生产环境必须设置加密密钥
            if not self.encryption_key:
                errors.append(
                    "ENCRYPTION_KEY must be set in production. "
                    "Use 'python generate_keys.py' to generate a secure key."
                )

        return errors

    def __repr__(self) -> None:
        """配置信息字符串表示"""
        return f"""
Configuration:
  Server: {self.host}:{self.port}
  Log Level: {self.log_level}
  Environment: {self.environment}
"""


# 创建全局配置实例
config = Config()

# 在调试模式下记录配置（延迟到日志系统初始化后）
# 这个配置信息会在应用启动时通过日志系统输出
