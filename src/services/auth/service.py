"""
认证服务
"""

from __future__ import annotations

import hashlib
import secrets
import time
import uuid
from collections import OrderedDict
from datetime import datetime, timedelta, timezone
from threading import Lock
from typing import TYPE_CHECKING, Any

import jwt
from fastapi import HTTPException, status
from fastapi.concurrency import run_in_threadpool
from sqlalchemy.exc import IntegrityError
from sqlalchemy.orm import Session, joinedload

from src.config import config
from src.core.enums import AuthSource
from src.core.exceptions import ForbiddenException
from src.core.logger import logger
from src.services.system.config import SystemConfigService

if TYPE_CHECKING:
    from src.models.database import ManagementToken

from src.models.database import ApiKey, User, UserRole
from src.services.auth.jwt_blacklist import JWTBlacklistService
from src.services.auth.ldap import LDAPService
from src.services.cache.user_cache import UserCacheService
from src.services.user.apikey import ApiKeyService

# API Key last_used_at 更新节流配置
# 同一个 API Key 在此时间间隔内只会更新一次 last_used_at
_LAST_USED_UPDATE_INTERVAL = 60  # 秒
_LAST_USED_CACHE_MAX_SIZE = 10000  # LRU 缓存最大条目数

# 进程内缓存：记录每个 API Key 最后一次更新 last_used_at 的时间
# 使用 OrderedDict 实现 LRU，避免内存无限增长
_api_key_last_update_times: OrderedDict[str, float] = OrderedDict()
_last_update_lock = Lock()


def _should_update_last_used(api_key_id: str) -> bool:
    """判断是否应该更新 API Key 的 last_used_at

    使用节流策略，同一个 Key 在指定间隔内只更新一次。
    线程安全，使用 LRU 策略限制缓存大小。

    Returns:
        True 表示应该更新，False 表示跳过
    """
    now = time.time()

    with _last_update_lock:
        last_update = _api_key_last_update_times.get(api_key_id, 0)

        if now - last_update >= _LAST_USED_UPDATE_INTERVAL:
            _api_key_last_update_times[api_key_id] = now
            # LRU: 移到末尾（最近使用）
            _api_key_last_update_times.move_to_end(api_key_id)

            # 超过最大容量时，移除最旧的条目
            while len(_api_key_last_update_times) > _LAST_USED_CACHE_MAX_SIZE:
                _api_key_last_update_times.popitem(last=False)

            return True
        return False


# JWT配置从config读取
if not config.jwt_secret_key:
    # 如果没有配置，生成一个随机密钥并警告
    if config.environment == "production":
        raise ValueError("JWT_SECRET_KEY must be set in production environment!")
    config.jwt_secret_key = secrets.token_urlsafe(32)
    logger.warning("JWT_SECRET_KEY未在环境变量中找到，已生成随机密钥用于开发")
    logger.warning("生产环境请设置JWT_SECRET_KEY环境变量!")

JWT_SECRET_KEY = config.jwt_secret_key
JWT_ALGORITHM = config.jwt_algorithm
JWT_EXPIRATION_HOURS = config.jwt_expiration_hours
# Refresh token 有效期设为7天
REFRESH_TOKEN_EXPIRATION_DAYS = 7


class AuthService:
    """认证服务"""

    @staticmethod
    def token_identity_matches_user(payload: dict[str, Any], user: User) -> bool:
        """
        校验 token 的身份字段是否与用户一致。

        兼容策略：
        - email：旧 token 可能包含；新 token 允许不包含（支持无邮箱用户）
        - created_at：用于替代 email 作为"防止身份混淆"的校验字段；旧 token 可能没有

        时区处理说明：
        - 本项目所有 created_at 统一使用 UTC 时区存储（PostgreSQL TIMESTAMPTZ）
        - 对于 naive datetime（无时区信息），假定为 UTC
        - 若历史数据使用了非 UTC 本地时区的 naive datetime，可能导致校验失败
        """
        token_email = payload.get("email")
        if token_email is not None and user.email is not None and user.email != token_email:
            return False

        token_created_at = payload.get("created_at")
        if not token_created_at or not user.created_at:
            return True

        try:
            token_created = datetime.fromisoformat(str(token_created_at).replace("Z", "+00:00"))
        except ValueError:
            return False

        # 统一时区：若是 naive datetime，按 UTC 处理
        # 注意：本项目约定所有时间戳使用 UTC，若旧数据不符合此约定可能导致校验失败
        user_created = user.created_at
        if user_created.tzinfo is None:
            user_created = user_created.replace(tzinfo=timezone.utc)
        if token_created.tzinfo is None:
            token_created = token_created.replace(tzinfo=timezone.utc)

        return abs((user_created - token_created).total_seconds()) <= 1

    @staticmethod
    def create_access_token(data: dict) -> str:
        """创建JWT访问令牌"""
        to_encode = data.copy()
        expire = datetime.now(timezone.utc) + timedelta(hours=JWT_EXPIRATION_HOURS)
        to_encode.update({"exp": expire, "type": "access"})
        encoded_jwt = jwt.encode(to_encode, JWT_SECRET_KEY, algorithm=JWT_ALGORITHM)
        return encoded_jwt

    @staticmethod
    def create_refresh_token(data: dict) -> str:
        """创建JWT刷新令牌"""
        to_encode = data.copy()
        expire = datetime.now(timezone.utc) + timedelta(days=REFRESH_TOKEN_EXPIRATION_DAYS)
        to_encode.update({"exp": expire, "type": "refresh"})
        encoded_jwt = jwt.encode(to_encode, JWT_SECRET_KEY, algorithm=JWT_ALGORITHM)
        return encoded_jwt

    @staticmethod
    async def verify_token(token: str, token_type: str | None = None) -> dict[str, Any]:
        """验证JWT令牌

        Args:
            token: JWT token字符串
            token_type: 期望的token类型 ('access' 或 'refresh')，None表示不验证类型
        """
        try:
            payload = jwt.decode(token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])

            # 验证token类型（如果指定）
            if token_type:
                actual_type = payload.get("type")
                if actual_type != token_type:
                    raise HTTPException(
                        status_code=status.HTTP_401_UNAUTHORIZED,
                        detail=f"Token类型错误: 期望 {token_type}, 实际 {actual_type}",
                    )

            # 检查 Token 是否在黑名单中
            is_blacklisted = await JWTBlacklistService.is_blacklisted(token)
            if is_blacklisted:
                raise HTTPException(
                    status_code=status.HTTP_401_UNAUTHORIZED, detail="Token已被撤销"
                )

            return payload
        except jwt.ExpiredSignatureError:
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="Token已过期")
        except jwt.InvalidTokenError:
            raise HTTPException(status_code=status.HTTP_401_UNAUTHORIZED, detail="无效的Token")

    @staticmethod
    async def authenticate_user(
        db: Session, email: str, password: str, auth_type: str = "local"
    ) -> User | None:
        """用户登录认证

        Args:
            db: 数据库会话
            email: 邮箱/用户名
            password: 密码
            auth_type: 认证类型 ("local" 或 "ldap")
        """
        if auth_type == "ldap":
            # LDAP 认证
            # 预取配置，避免将 Session 传递到线程池
            config_data = LDAPService.get_config_data(db)
            if not config_data:
                logger.warning("登录失败 - LDAP 未启用或配置无效")
                return None

            # 计算总体超时：LDAP 认证包含多次网络操作（连接、管理员绑定、搜索、用户绑定）
            # 超时策略：
            # - 单次操作超时(connect_timeout)：控制每次网络操作的最大等待时间
            # - 总体超时：防止异常场景（如服务器响应缓慢但未超时）导致请求堆积
            # - 公式：单次超时 × 4（覆盖 4 次主要网络操作）+ 10% 缓冲
            # - 最小 20 秒（保证基本操作），最大 60 秒（避免用户等待过长）
            single_timeout = config_data.get("connect_timeout", 10)
            total_timeout = max(20, min(int(single_timeout * 4 * 1.1), 60))

            # 在线程池中执行阻塞的 LDAP 网络请求，避免阻塞事件循环
            # 添加总体超时保护，防止异常场景下请求堆积
            import asyncio

            try:
                ldap_user = await asyncio.wait_for(
                    run_in_threadpool(
                        LDAPService.authenticate_with_config, config_data, email, password
                    ),
                    timeout=total_timeout,
                )
            except TimeoutError:
                logger.error(f"LDAP 认证总体超时({total_timeout}秒): {email}")
                return None

            if not ldap_user:
                return None

            # 获取或创建本地用户
            user = await AuthService._get_or_create_ldap_user(db, ldap_user)
            if not user:
                # 已有本地账号但来源不匹配等情况
                return None
            if user.is_deleted:
                logger.warning(f"登录失败 - 用户已删除: {email}")
                return None
            if not user.is_active:
                logger.warning(f"登录失败 - 用户已禁用: {email}")
                return None
            return user

        # 本地认证
        # 登录校验必须读取密码哈希，不能使用不包含 password_hash 的缓存对象
        # 支持邮箱或用户名登录
        from sqlalchemy import or_

        user = db.query(User).filter(or_(User.email == email, User.username == email)).first()

        if not user:
            logger.warning(f"登录失败 - 用户不存在: {email}")
            return None

        if user.is_deleted:
            logger.warning(f"登录失败 - 用户已删除: {email}")
            return None

        # 检查 LDAP exclusive 模式：仅允许本地管理员登录（紧急恢复通道）
        if LDAPService.is_ldap_exclusive(db):
            if user.role != UserRole.ADMIN or user.auth_source != AuthSource.LOCAL:
                logger.warning(f"登录失败 - 仅允许 LDAP 登录（管理员除外）: {email}")
                return None
            logger.warning(f"[LDAP-EXCLUSIVE] 紧急恢复通道：本地管理员登录: {email}")

        # 检查用户认证来源
        if user.auth_source == AuthSource.LDAP:
            logger.warning(f"登录失败 - 该用户使用 LDAP 认证: {email}")
            return None

        # 在线程池中执行 bcrypt 密码验证，避免阻塞事件循环
        if not await run_in_threadpool(user.verify_password, password):
            logger.warning(f"登录失败 - 密码错误: {email}")
            return None

        if not user.is_active:
            logger.warning(f"登录失败 - 用户已禁用: {email}")
            return None

        # 更新最后登录时间
        user.last_login_at = datetime.now(timezone.utc)
        db.commit()  # 立即提交事务,释放数据库锁
        # 清除缓存，因为用户信息已更新
        await UserCacheService.invalidate_user_cache(user.id, user.email)

        logger.info(f"用户登录成功: {email} (ID: {user.id})")
        return user

    @staticmethod
    async def _get_or_create_ldap_user(db: Session, ldap_user: dict) -> User | None:
        """获取或创建 LDAP 用户

        Args:
            ldap_user: LDAP 用户信息 {username, email, display_name, ldap_dn, ldap_username}

        注意：使用 with_for_update() 防止并发首次登录创建重复用户
        """
        ldap_dn = (ldap_user.get("ldap_dn") or "").strip() or None
        ldap_username = (
            ldap_user.get("ldap_username") or ldap_user.get("username") or ""
        ).strip() or None
        email = ldap_user["email"]

        # 优先用稳定标识查找，避免邮箱变更/用户名冲突导致重复建号
        # 使用 with_for_update() 锁定行，防止并发创建
        user: User | None = None
        if ldap_dn:
            user = (
                db.query(User)
                .filter(User.auth_source == AuthSource.LDAP, User.ldap_dn == ldap_dn)
                .with_for_update()
                .first()
            )
        if not user and ldap_username:
            user = (
                db.query(User)
                .filter(User.auth_source == AuthSource.LDAP, User.ldap_username == ldap_username)
                .with_for_update()
                .first()
            )
        if not user:
            # 最后回退按 email 查找：如果存在同邮箱的本地账号，需要拒绝以避免接管
            user = db.query(User).filter(User.email == email).with_for_update().first()

        if user:
            if user.is_deleted:
                logger.warning(f"LDAP 登录失败 - 用户已删除: {email}")
                return None

            if user.auth_source != AuthSource.LDAP:
                # 避免覆盖已有本地账户（不同来源时拒绝登录）
                logger.warning(
                    f"LDAP 登录拒绝 - 账户来源不匹配(现有:{user.auth_source}, 请求:LDAP): {email}"
                )
                return None

            # 同步邮箱（LDAP 侧邮箱变更时更新；若新邮箱已被占用则拒绝）
            if user.email != email:
                email_taken = db.query(User).filter(User.email == email, User.id != user.id).first()
                if email_taken:
                    logger.warning(f"LDAP 登录拒绝 - 新邮箱已被占用: {email}")
                    return None
                user.email = email
                user.email_verified = True

            # 同步 LDAP 标识（首次填充或 LDAP 侧发生变化）
            if ldap_dn and user.ldap_dn != ldap_dn:
                user.ldap_dn = ldap_dn
            if ldap_username and user.ldap_username != ldap_username:
                user.ldap_username = ldap_username

            user.last_login_at = datetime.now(timezone.utc)
            db.commit()
            await UserCacheService.invalidate_user_cache(user.id, user.email)
            logger.info(f"LDAP 用户登录成功: {ldap_user['email']} (ID: {user.id})")
            return user

        # 检查 username 是否已被占用，使用时间戳+随机数确保唯一性
        base_username = ldap_username or ldap_user["username"]
        username = base_username
        max_retries = 3

        for attempt in range(max_retries):
            # 检查用户名是否已存在
            existing_user_with_username = db.query(User).filter(User.username == username).first()
            if existing_user_with_username:
                # 如果 username 已存在，使用时间戳+随机数确保唯一性
                username = f"{base_username}_ldap_{int(time.time())}{uuid.uuid4().hex[:4]}"
                logger.info(f"LDAP 用户名冲突，使用新用户名: {ldap_user['username']} -> {username}")

            # 读取系统配置的默认配额
            default_quota = SystemConfigService.get_config(
                db, "default_user_quota_usd", default=10.0
            )

            # 创建新用户
            user = User(
                email=email,
                email_verified=True,
                username=username,
                password_hash=None,  # LDAP 用户无本地密码
                auth_source=AuthSource.LDAP,
                ldap_dn=ldap_dn,
                ldap_username=ldap_username,
                role=UserRole.USER,
                is_active=True,
                last_login_at=datetime.now(timezone.utc),
                quota_usd=default_quota,
            )

            try:
                db.add(user)
                db.commit()
                db.refresh(user)
                logger.info(f"LDAP 用户创建成功: {ldap_user['email']} (ID: {user.id})")
                return user
            except IntegrityError as e:
                db.rollback()
                error_str = str(e.orig).lower() if e.orig else str(e).lower()

                # 解析具体冲突类型
                if "email" in error_str or "ix_users_email" in error_str:
                    # 邮箱冲突不应重试（前面已检查过，说明是并发创建）
                    logger.error(f"LDAP 用户创建失败 - 邮箱并发冲突: {email}")
                    return None
                elif "username" in error_str or "ix_users_username" in error_str:
                    # 用户名冲突，重试时会生成新用户名
                    if attempt == max_retries - 1:
                        logger.error(f"LDAP 用户创建失败（用户名冲突重试耗尽）: {username}")
                        return None
                    username = f"{base_username}_ldap_{int(time.time())}{uuid.uuid4().hex[:4]}"
                    logger.warning(
                        f"LDAP 用户创建用户名冲突，重试 ({attempt + 1}/{max_retries}): {username}"
                    )
                else:
                    # 其他约束冲突，不重试
                    logger.error(f"LDAP 用户创建失败 - 未知数据库约束冲突: {e}")
                    return None

        return None

    @staticmethod
    def authenticate_api_key(db: Session, api_key: str) -> tuple[User, ApiKey] | None:
        """API密钥认证"""
        # 对API密钥进行哈希查找，预加载 user 关系以支持后续访问限制检查
        key_hash = ApiKey.hash_key(api_key)
        key_record = (
            db.query(ApiKey)
            .options(joinedload(ApiKey.user))
            .filter(ApiKey.key_hash == key_hash)
            .first()
        )

        if not key_record:
            # 只记录认证失败事件，不记录任何 key 信息以防止信息泄露
            logger.warning("API认证失败 - 密钥不存在或无效")
            return None

        if not key_record.is_active:
            logger.warning("API认证失败 - 密钥已禁用")
            return None

        if key_record.is_locked:
            logger.warning("API认证失败 - 密钥已被管理员锁定")
            raise ForbiddenException("该密钥已被管理员锁定，请联系管理员")

        # 检查过期时间
        if key_record.expires_at:
            # 确保 expires_at 是 aware datetime
            expires_at = key_record.expires_at
            if expires_at.tzinfo is None:
                # 如果没有时区信息，假定为 UTC
                expires_at = expires_at.replace(tzinfo=timezone.utc)

            if expires_at < datetime.now(timezone.utc):
                logger.warning("API认证失败 - 密钥已过期")
                return None

        # 检查余额限制（仅独立Key）
        is_balance_ok, remaining = ApiKeyService.check_balance(key_record)
        if not is_balance_ok:
            # 获取剩余余额用于日志
            remaining_balance = ApiKeyService.get_remaining_balance(key_record)
            logger.warning(
                f"API认证失败 - 余额不足 "
                f"(已用: ${key_record.balance_used_usd:.4f}, 剩余: ${remaining_balance:.4f})"
            )
            return None

        # 获取用户
        user = key_record.user
        if not user.is_active:
            logger.warning(f"API认证失败 - 用户已禁用: {user.email}")
            return None
        if user.is_deleted:
            logger.warning(f"API认证失败 - 用户已删除: {user.email}")
            return None

        # 更新最后使用时间（使用节流策略，减少数据库写入）
        if _should_update_last_used(key_record.id):
            key_record.last_used_at = datetime.now(timezone.utc)

            # 这里需要 commit 来尽快释放锁，但默认 expire_on_commit=True 会让已加载对象过期，
            # 导致同一请求后续访问 user/api_key 字段时触发额外 SELECT。
            original_expire_on_commit = getattr(db, "expire_on_commit", None)
            try:
                if original_expire_on_commit is not None:
                    db.expire_on_commit = False
                db.commit()  # 立即提交事务,释放数据库锁,避免阻塞后续请求
            except Exception:
                db.rollback()
                raise
            finally:
                if original_expire_on_commit is not None:
                    db.expire_on_commit = original_expire_on_commit

        api_key_fp = hashlib.sha256(api_key.encode()).hexdigest()[:12]
        logger.debug("API认证成功: 用户 {} (api_key_fp={})", user.email, api_key_fp)
        return user, key_record

    @staticmethod
    def check_user_quota(user: User, estimated_cost: float = 0) -> bool:
        """检查用户配额"""
        if user.role == UserRole.ADMIN:
            return True  # 管理员无限制

        # NULL 表示无限制
        if user.quota_usd is None:
            return True

        # 检查美元配额
        if user.used_usd + estimated_cost > user.quota_usd:
            logger.warning(
                f"用户配额不足: {user.email} (已用: ${user.used_usd:.2f}, 配额: ${user.quota_usd:.2f})"
            )
            return False

        return True

    @staticmethod
    def check_permission(user: User, required_role: UserRole = UserRole.USER) -> bool:
        """检查用户权限"""
        if user.role == UserRole.ADMIN:
            return True

        # 避免使用字符串比较导致权限判断错误（例如 'user' >= 'admin'）
        role_rank = {UserRole.USER: 0, UserRole.ADMIN: 1}
        # 未知用户角色默认 -1（拒绝），未知要求角色默认 999（拒绝）
        if role_rank.get(user.role, -1) >= role_rank.get(required_role, 999):
            return True

        logger.warning(
            f"权限不足: 用户 {user.email} 角色 {user.role.value} < 需要 {required_role.value}"
        )
        return False

    @staticmethod
    async def logout(token: str) -> bool:
        """
        用户登出，将 Token 加入黑名单

        Args:
            token: JWT token字符串

        Returns:
            是否成功登出
        """
        try:
            # 解码 Token 获取过期时间（不验证黑名单）
            payload = jwt.decode(token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])
            exp_timestamp = payload.get("exp")

            if not exp_timestamp:
                logger.warning("Token 缺少过期时间，无法加入黑名单")
                return False

            # 将 Token 加入黑名单
            success = await JWTBlacklistService.add_to_blacklist(
                token=token, exp_timestamp=exp_timestamp, reason="logout"
            )

            if success:
                user_id = payload.get("user_id")
                logger.info(f"用户登出成功: user_id={user_id}")

            return success

        except jwt.InvalidTokenError as e:
            logger.warning(f"登出失败 - 无效的 Token: {e}")
            return False
        except Exception as e:
            logger.error(f"登出失败: {e}")
            return False

    @staticmethod
    async def revoke_token(token: str, reason: str = "revoked") -> bool:
        """
        撤销 Token（管理员操作）

        Args:
            token: JWT token字符串
            reason: 撤销原因

        Returns:
            是否成功撤销
        """
        try:
            # 解码 Token 获取过期时间
            payload = jwt.decode(token, JWT_SECRET_KEY, algorithms=[JWT_ALGORITHM])
            exp_timestamp = payload.get("exp")

            if not exp_timestamp:
                logger.warning("Token 缺少过期时间，无法撤销")
                return False

            # 将 Token 加入黑名单
            success = await JWTBlacklistService.add_to_blacklist(
                token=token, exp_timestamp=exp_timestamp, reason=reason
            )

            if success:
                user_id = payload.get("sub")
                logger.warning(f"Token 已被撤销: user_id={user_id}, reason={reason}")

            return success

        except jwt.InvalidTokenError as e:
            logger.warning(f"撤销失败 - 无效的 Token: {e}")
            return False
        except Exception as e:
            logger.error(f"撤销 Token 失败: {e}")
            return False

    @staticmethod
    async def authenticate_management_token(
        db: Session, raw_token: str, client_ip: str
    ) -> tuple[User, ManagementToken] | None:
        """Management Token 认证

        Args:
            db: 数据库会话
            raw_token: Management Token 字符串
            client_ip: 客户端 IP

        Returns:
            (User, ManagementToken) 元组，认证失败返回 None

        Raises:
            RateLimitException: 超过速率限制时抛出（用于返回 429）
        """
        from src.core.exceptions import RateLimitException
        from src.core.modules import get_module_registry
        from src.models.database import AuditEventType, ManagementToken
        from src.services.rate_limit.ip_limiter import IPRateLimiter
        from src.services.system.audit import AuditService

        # 检查访问令牌模块是否激活
        module_registry = get_module_registry()
        if not module_registry.is_active("management_tokens", db):
            logger.warning("Management Token 认证失败 - 访问令牌模块未激活")
            return None

        # 速率限制检查（防止暴力破解）
        allowed, remaining, ttl = await IPRateLimiter.check_limit(
            client_ip,
            endpoint_type="management_token",
            limit=config.management_token_rate_limit,
        )
        if not allowed:
            logger.warning(f"Management Token 认证 - IP {client_ip} 超过速率限制")
            raise RateLimitException(limit=config.management_token_rate_limit, window="分钟")

        # 检查 Token 格式
        if not raw_token.startswith(ManagementToken.TOKEN_PREFIX):
            logger.warning("Management Token 认证失败 - 格式错误")
            return None

        # 哈希查找
        token_hash = ManagementToken.hash_token(raw_token)
        token_record = (
            db.query(ManagementToken)
            .options(joinedload(ManagementToken.user))
            .filter(ManagementToken.token_hash == token_hash)
            .first()
        )

        if not token_record:
            logger.warning("Management Token 认证失败 - Token 不存在")
            return None

        # 注意：数据库查询已通过 token_hash 索引匹配，此处不再需要额外的常量时间比较
        # Token 的 62^40 熵（约 238 位）加上速率限制已足够防止暴力破解

        # 检查状态
        if not token_record.is_active:
            logger.warning(f"Management Token 认证失败 - Token 已禁用: {token_record.id}")
            return None

        # 检查过期（使用属性方法，确保时区安全）
        if token_record.is_expired:
            logger.warning(f"Management Token 认证失败 - Token 已过期: {token_record.id}")
            AuditService.log_event(
                db=db,
                event_type=AuditEventType.MANAGEMENT_TOKEN_EXPIRED,
                description=f"Management Token 已过期: {token_record.name}",
                user_id=token_record.user_id,
                ip_address=client_ip,
                metadata={
                    "token_id": token_record.id,
                    "token_name": token_record.name,
                    "expired_at": (
                        token_record.expires_at.isoformat() if token_record.expires_at else None
                    ),
                },
            )
            return None

        # 检查 IP 白名单
        if not token_record.is_ip_allowed(client_ip):
            logger.warning(f"Management Token IP 限制 - Token: {token_record.id}, IP: {client_ip}")
            AuditService.log_event(
                db=db,
                event_type=AuditEventType.MANAGEMENT_TOKEN_IP_BLOCKED,
                description=f"Management Token IP 被拒绝: {token_record.name}",
                user_id=token_record.user_id,
                ip_address=client_ip,
                metadata={
                    "token_id": token_record.id,
                    "token_name": token_record.name,
                    "blocked_ip": client_ip,
                    # 不记录 allowed_ips 以防信息泄露
                },
            )
            return None

        # 获取用户
        user = token_record.user
        if not user or not user.is_active:
            logger.warning("Management Token 认证失败 - 用户不存在或已禁用")
            return None
        if user.is_deleted:
            logger.warning("Management Token 认证失败 - 用户不存在或已禁用")
            return None

        # 使用 SQL 原子操作更新使用统计
        from sqlalchemy import func

        db.query(ManagementToken).filter(ManagementToken.id == token_record.id).update(
            {
                ManagementToken.last_used_at: func.now(),  # 使用数据库时间确保一致性
                ManagementToken.last_used_ip: client_ip,
                ManagementToken.usage_count: ManagementToken.usage_count + 1,
                ManagementToken.updated_at: func.now(),  # 显式更新，因为原子 SQL 绕过 ORM
            },
            synchronize_session=False,
        )

        # 记录 Token 使用审计日志
        AuditService.log_event(
            db=db,
            event_type=AuditEventType.MANAGEMENT_TOKEN_USED,
            description=f"Management Token 认证成功: {token_record.name}",
            user_id=user.id,
            ip_address=client_ip,
            metadata={
                "token_id": token_record.id,
                "token_name": token_record.name,
            },
        )

        db.commit()

        logger.debug(f"Management Token 认证成功: user={user.email}, token={token_record.id}")
        return user, token_record
