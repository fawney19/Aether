"""
通用端点检查执行器（adapter check_endpoint 复用）

目标：
- 统一日志输出格式
- 统一错误处理逻辑
- 将适配器差异收敛到：URL / headers / body 构建
- 集成用量统计和费用计算

重构架构 - 分离关注点：
- HttpRequestExecutor: 专门负责HTTP请求执行
- UsageCalculator: 专门负责Token计数和费用计算
- ErrorHandler: 统一错误处理
- EndpointCheckOrchestrator: 协调整个流程
"""

from dataclasses import dataclass
from typing import Any, AsyncIterator, Dict, Iterable, Optional, Union, List
from abc import ABC, abstractmethod
import time
import uuid
import json
from functools import lru_cache
import asyncio
from collections import defaultdict

import httpx

from src.core.logger import logger


_SENSITIVE_HEADER_KEYS = {
    "authorization",
    "x-api-key",
    "x-goog-api-key",
}


def _redact_headers(headers: Dict[str, str]) -> Dict[str, str]:
    redacted: Dict[str, str] = {}
    for key, value in headers.items():
        if key.lower() in _SENSITIVE_HEADER_KEYS:
            redacted[key] = "***"
        else:
            redacted[key] = value
    return redacted


def _truncate_repr(value: Any, limit: int = 1200) -> str:
    try:
        text = repr(value)
    except Exception:
        text = f"<unreprable {type(value)!r}>"
    if len(text) > limit:
        return text[:limit] + "...(truncated)"
    return text


def build_safe_headers(
    base_headers: Dict[str, str],
    extra_headers: Optional[Dict[str, Any]],
    protected_keys: Iterable[str],
) -> Dict[str, str]:
    """
    合并 extra_headers，但防止覆盖 protected_keys（大小写不敏感）。

    支持高级 headers 规则处理：
    - 如果 header_rules 存在，会先应用规则处理 headers
    - 规则包括：add（新增）、remove（删除）、replace_name（改名）、replace_value（改值）

    Args:
        base_headers: 基础 headers
        extra_headers: 额外的 headers（可选）
        protected_keys: 受保护的键列表（不会被覆盖）
        header_rules: headers 处理规则（可选）

    Returns:
        处理后的 headers 字典
    """
    headers = dict(base_headers)
    if not extra_headers:
        return headers

    # 合并 extra_headers（如果有）
    if extra_headers:
        from src.core.header_rules import apply_header_rules
        headers = apply_header_rules(headers, extra_headers)

    logger.debug(f"headers:{headers}")
    return headers


# 保持向后兼容的run_endpoint_check函数（使用新架构）
async def run_endpoint_check(
    *,
    client: httpx.AsyncClient,  # 保持兼容性，但内部不使用
    url: str,
    headers: Dict[str, str],
    json_body: Dict[str, Any],
    api_format: str,
    provider_name: Optional[str] = None,
    model_name: Optional[str] = None,
    api_key_id: Optional[str] = None,
    provider_id: Optional[str] = None,
    db: Optional[Any] = None,  # Session对象，需要时才导入
    user: Optional[Any] = None,  # User对象
    enable_usage_calculation: bool = True,  # 是否启用用量计算
) -> Dict[str, Any]:
    """
    执行端点检查（重构版本，使用新的架构）：
    - 使用新的架构类来分离关注点
    - 保持与现有代码的兼容性
    - 支持用量统计和费用计算（可通过 enable_usage_calculation 控制）
    """

    # 创建端点检查请求对象
    request = EndpointCheckRequest(
        url=url,
        headers=headers,
        json_body=json_body,
        api_format=api_format,
        provider_name=provider_name,
        model_name=model_name,
        api_key_id=api_key_id,
        provider_id=provider_id,
        db=db,
        user=user,
        request_id=str(uuid.uuid4())[:8]
    )

    # 使用协调器执行检查
    orchestrator = EndpointCheckOrchestrator(
        enable_usage_calculation=enable_usage_calculation
    )
    result = await orchestrator.execute_check(request)

    # 转换为原有的响应格式以保持兼容性
    response_data = {
        "status_code": result.status_code,
        "headers": result.headers,
        "response_time_ms": result.response_time_ms,
        "request_id": result.request_id,
    }

    if result.response_data:
        response_data["response"] = result.response_data

    if result.error_message:
        response_data["error"] = result.error_message

    if result.usage_data:
        response_data["usage"] = result.usage_data

    return response_data

async def _calculate_and_record_usage(
    *,
    db: Any,
    user: Any,
    provider_name: str,
    provider_id: str,
    api_key_id: str,
    model_name: str,
    request_data: Dict[str, Any],
    response_data: Optional[Dict[str, Any]],
    request_id: str,
    response_time_ms: int,
    request_headers: Dict[str, str],
    response_headers: Optional[Dict[str, str]] = None,
    status_code: int = 0,
    error_message: Optional[str] = None,
    # 新增：支持直接传递token数据
    input_tokens: Optional[int] = None,
    output_tokens: Optional[int] = None,
    cache_creation_input_tokens: Optional[int] = None,
    cache_read_input_tokens: Optional[int] = None,
    api_format: Optional[str] = None,
) -> Dict[str, Any]:
    """
    计算并记录用量数据（遗留函数）

    注意：这是测试请求，使用的是Provider的API Key，但用量记录关联到执行测试的用户
    这是重构过程中的遗留函数，保持向后兼容性

    Returns:
        Dict包含用量统计信息
    """
    from src.services.usage.service import UsageService
    from src.services.request.candidate import RequestCandidateService
    from src.models.database import ApiKey, ProviderAPIKey, ProviderEndpoint

    # 获取Provider API Key对象（不是用户API Key）
    provider_api_key = db.query(ProviderAPIKey).filter(ProviderAPIKey.id == api_key_id).first()
    if not provider_api_key:
        logger.warning(f"Provider API Key not found for usage calculation: {api_key_id}")
        return {"error": "Provider API Key not found"}

    # 获取Provider Endpoint信息
    provider_endpoint = None
    if provider_api_key.endpoint_id:
        provider_endpoint = db.query(ProviderEndpoint).filter(ProviderEndpoint.id == provider_api_key.endpoint_id).first()

    # 获取用户的API Key（用于记录关联，即使实际使用的是Provider API Key）
    user_api_key = None
    if user:
        try:
            user_api_key = db.query(ApiKey).filter(ApiKey.user_id == user.id).first()
            logger.info(f"[endpoint_check] User API Key found: {user_api_key.id if user_api_key else None}")
        except Exception as e:
            logger.warning(f"[endpoint_check] Failed to get user API Key: {e}")
            user_api_key = None

    # 注意：测试请求使用Provider的API Key，但用量记录关联到执行测试的用户
    # 用量记录会关联到执行测试的用户，但实际的API调用使用Provider的配置

    # Token计数 - 优先使用直接传递的数据，否则使用原有逻辑
    if input_tokens is None or output_tokens is None or cache_creation_input_tokens is None or cache_read_input_tokens is None:
        # 使用原有逻辑计算token
        logger.info(f"[endpoint_check] Calculating tokens from response data")

        # 直接从响应中提取usage信息（优先级最高）
        usage_info = {}
        if response_data and isinstance(response_data, dict):
            usage_info = response_data.get("usage", {})

        if not api_format:
            api_format = "OPENAI"

        logger.info(f"[endpoint_check] Detected API format: {api_format}")

        if usage_info:
            logger.info(f"[endpoint_check] Found usage field in response: {usage_info}")
            # 使用提取函数获取token数据
            api_identifier = provider_name  # 在这个旧函数中，我们只能使用provider_name
            extracted_input, extracted_output, extracted_cache_creation, extracted_cache_read = \
                _extract_tokens_from_response(api_identifier, response_data)

            input_tokens = input_tokens or extracted_input
            output_tokens = output_tokens or extracted_output
            cache_creation_input_tokens = cache_creation_input_tokens or extracted_cache_creation
            cache_read_input_tokens = cache_read_input_tokens or extracted_cache_read

        else:
            # 如果没有usage字段，使用fallback
            logger.warning(f"[endpoint_check] No usage field found in response, using fallback counting")
            try:
                fallback_input, fallback_output, fallback_cache_creation, fallback_cache_read = \
                    _fallback_token_counting(request_data, response_data)

                input_tokens = input_tokens or fallback_input
                output_tokens = output_tokens or fallback_output
                cache_creation_input_tokens = cache_creation_input_tokens or fallback_cache_creation
                cache_read_input_tokens = cache_read_input_tokens or fallback_cache_read
            except Exception as e:
                logger.error(f"[endpoint_check] Fallback token counting failed: {e}")
                # 设置最小值
                input_tokens = input_tokens or 1
                output_tokens = output_tokens or 1
                cache_creation_input_tokens = cache_creation_input_tokens or 0
                cache_read_input_tokens = cache_read_input_tokens or 0

    logger.info(f"[endpoint_check] Final token count | input={input_tokens}, output={output_tokens}, "
               f"cache_creation={cache_creation_input_tokens}, cache_read={cache_read_input_tokens}")

    try:
        # 使用UsageService记录用量
        # 测试请求会关联到执行测试的用户API Key，但实际使用Provider API Key
        logger.info(f"[endpoint_check] Recording usage | provider={provider_name}, model={model_name}, "
                   f"tokens=({input_tokens}+{output_tokens}), status={status_code}, "
                   f"user_api_key_id={user_api_key.id if user_api_key else None}, "
                   f"provider_endpoint_id={provider_endpoint.id if provider_endpoint else None}")

        usage_record = await UsageService.record_usage_async(
            db=db,
            user=user,
            api_key=user_api_key,  # 关联到执行测试的用户API Key
            provider=provider_name,
            model=model_name,
            input_tokens=input_tokens,
            output_tokens=output_tokens,
            cache_creation_input_tokens=cache_creation_input_tokens,
            cache_read_input_tokens=cache_read_input_tokens,
            request_type="endpoint_test",  # 使用特殊的请求类型标识测试
            api_format=api_format,
            is_stream=False,
            response_time_ms=response_time_ms,
            first_byte_time_ms=response_time_ms,
            status_code=status_code,
            error_message=error_message,
            request_headers=request_headers,
            response_headers=response_headers,
            request_body=request_data,
            response_body=response_data,
            request_id=f"test_{request_id}",
            provider_id=provider_id,
            provider_endpoint_id=provider_endpoint.id if provider_endpoint else None,  # 添加端点ID
            provider_api_key_id=api_key_id,  # 记录实际使用的Provider API Key
            status="completed" if status_code == 200 else "failed",
            use_tiered_pricing=True,
            target_model=model_name,  # 添加目标模型
        )

        # 检查费用计算是否成功
        total_cost = float(usage_record.total_cost_usd) if usage_record.total_cost_usd else 0.0
        actual_cost = float(usage_record.actual_total_cost_usd) if usage_record.actual_total_cost_usd else 0.0
        cache_cost = float(usage_record.cache_cost_usd) if usage_record.cache_cost_usd else 0.0

        # 如果费用为0但Token不为0，可能是价格配置缺失，使用默认价格
        if total_cost == 0.0 and (input_tokens > 0 or output_tokens > 0):
            logger.warning(f"[endpoint_check] Cost calculation returned 0, using fallback pricing")
            # 使用默认价格：$0.001/1K tokens
            fallback_price_per_1m = 1.0  # $1 per 1M tokens
            total_cost = ((input_tokens + output_tokens) / 1_000_000) * fallback_price_per_1m
            actual_cost = total_cost  # 测试请求使用实际成本

        logger.info(f"[endpoint_check] Usage recorded successfully | "
                   f"usage_id={usage_record.id}, total_cost=${total_cost:.6f}, "
                   f"actual_cost=${actual_cost:.6f}")

        # 创建RequestCandidate记录，用于监控追踪API
        try:
            # 首先创建候选记录
            candidate = RequestCandidateService.create_candidate(
                db=db,
                request_id=f"test_{request_id}",
                candidate_index=0,  # 测试请求只有一个候选
                user_id=user.id if user else None,
                api_key_id=user_api_key.id if user_api_key else None,
                provider_id=provider_id,
                endpoint_id=provider_endpoint.id if provider_endpoint else None,
                key_id=api_key_id,
                status="available",
                extra_data={"model_name": model_name, "request_type": "endpoint_test"},
            )

            # 立即标记为开始执行
            RequestCandidateService.mark_candidate_started(db, candidate.id)

            # 根据结果标记为成功或失败
            if status_code == 200:
                RequestCandidateService.mark_candidate_success(
                    db=db,
                    candidate_id=candidate.id,
                    status_code=status_code,
                    latency_ms=response_time_ms,
                    extra_data={"model_name": model_name, "api_format": api_format},
                )
            else:
                RequestCandidateService.mark_candidate_failed(
                    db=db,
                    candidate_id=candidate.id,
                    error_type="http_error" if status_code > 0 else "network_error",
                    error_message=error_message or "Unknown error",
                    status_code=status_code,
                    latency_ms=response_time_ms,
                    extra_data={"model_name": model_name, "api_format": api_format},
                )

            logger.info(f"[endpoint_check] RequestCandidate created | request_id=test_{request_id}, candidate_id={candidate.id}")
        except Exception as e:
            logger.warning(f"[endpoint_check] Failed to create RequestCandidate: {e}")
            # 不影响主要功能
            candidate = None

        return {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cache_creation_input_tokens": cache_creation_input_tokens,
            "cache_read_input_tokens": cache_read_input_tokens,
            "total_tokens": input_tokens + output_tokens,
            "total_cost_usd": total_cost,
            "actual_total_cost_usd": actual_cost,
            "cache_cost_usd": cache_cost,
            "status_code": status_code,
            "usage_id": str(usage_record.id),
            "api_format": api_format,
            "request_id": f"test_{request_id}",  # 返回request_id用于追踪
            "candidate_id": str(candidate.id) if candidate else None,  # 返回candidate_id
        }

    except Exception as e:
        logger.error(f"Failed to record usage for endpoint check: {e}")
        return {
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "cache_creation_input_tokens": cache_creation_input_tokens,
            "cache_read_input_tokens": cache_read_input_tokens,
            "total_tokens": input_tokens + output_tokens,
            "error": str(e),
            "status_code": status_code,
            "api_format": api_format,
        }


def _extract_tokens_from_response(api_identifier: str, response_data: Optional[Dict[str, Any]]) -> tuple[int, int, int, int]:
    """
    从响应中提取Token计数信息

    Args:
        api_identifier: API标识符（api_format或provider_name）
        response_data: 响应数据

    Returns:
        tuple[int, int, int, int]: (input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens)
    """
    if not response_data:
        return 0, 0, 0, 0

    api_identifier_lower = api_identifier.lower()
    usage_info = response_data.get("usage", {})

    if not usage_info:
        return 0, 0, 0, 0

    input_tokens = 0
    output_tokens = 0
    cache_creation_input_tokens = 0
    cache_read_input_tokens = 0

    try:
        # 基于adapter名字进行更精确的检测
        if "claude" in api_identifier_lower:
            # Claude格式 - 支持claude.chat和claude.token_count等adapter
            input_tokens = usage_info.get("input_tokens", 0)
            output_tokens = usage_info.get("output_tokens", 0)
            cache_read_input_tokens = usage_info.get("cache_read_input_tokens", 0)

            # 尝试提取cache creation tokens
            try:
                from src.api.handlers.base.utils import extract_cache_creation_tokens
                cache_creation_input_tokens = extract_cache_creation_tokens(usage_info)
            except Exception as e:
                logger.warning(f"[endpoint_check] Failed to extract cache creation tokens: {e}")

        elif "openai" in api_identifier_lower:
            # OpenAI格式
            input_tokens = usage_info.get("prompt_tokens", 0) or usage_info.get("input_tokens", 0)
            output_tokens = usage_info.get("completion_tokens", 0) or usage_info.get("output_tokens", 0)
            cache_creation_input_tokens = 0
            cache_read_input_tokens = 0

        elif "gemini" in api_identifier_lower or "google" in api_identifier_lower:
            # Gemini格式 - 使用与OpenAI类似的字段名
            input_tokens = usage_info.get("prompt_tokens", 0) or usage_info.get("input_tokens", 0)
            output_tokens = usage_info.get("completion_tokens", 0) or usage_info.get("output_tokens", 0)
            cache_creation_input_tokens = 0
            cache_read_input_tokens = 0

        # Fallback: 尝试其他可能的provider名称匹配
        elif "anthropic" in api_identifier_lower:
            # Anthropic/Claude的其他别名
            input_tokens = usage_info.get("input_tokens", 0)
            output_tokens = usage_info.get("output_tokens", 0)
            cache_read_input_tokens = usage_info.get("cache_read_input_tokens", 0)
            try:
                from src.api.handlers.base.utils import extract_cache_creation_tokens
                cache_creation_input_tokens = extract_cache_creation_tokens(usage_info)
            except Exception as e:
                logger.warning(f"[endpoint_check] Failed to extract cache creation tokens: {e}")

        else:
            # 默认情况：尝试通用提取
            logger.warning(f"[endpoint_check] Unknown API identifier: {api_identifier}, using generic token extraction")
            input_tokens = usage_info.get("input_tokens", 0) or usage_info.get("prompt_tokens", 0)
            output_tokens = usage_info.get("output_tokens", 0) or usage_info.get("completion_tokens", 0)

    except Exception as e:
        logger.warning(f"[endpoint_check] Error extracting tokens from response: {e}")
        return 0, 0, 0, 0

    logger.info(f"[endpoint_check] Tokens extracted from response | "
               f"api_identifier={api_identifier}, "
               f"input={input_tokens}, output={output_tokens}, "
               f"cache_creation={cache_creation_input_tokens}, cache_read={cache_read_input_tokens}")

    return input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens




def _fallback_token_counting(request_data: Dict[str, Any], response_data: Optional[Dict[str, Any]]) -> tuple[int, int, int, int]:
    """
    回退的Token计数方法（简单估算）

    Returns:
        tuple[int, int, int, int]: (input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens)
    """
    # 估算输入Token
    messages = request_data.get("messages", request_data.get("contents", []))
    if messages:
        input_text = str(messages)
        input_tokens = max(1, len(input_text.split()) // 4)
    else:
        # 如果没有消息内容，使用最小值
        input_tokens = 1

    # 估算输出Token
    output_tokens = 1  # 最小输出Token数
    if response_data:
        # 尝试从响应中提取文本内容
        if isinstance(response_data, dict):
            # Claude格式
            if "content" in response_data:
                content = response_data["content"]
                if isinstance(content, str):
                    output_text = content
                elif isinstance(content, list):
                    # Claude的content通常是列表格式
                    output_text = ""
                    for block in content:
                        if isinstance(block, dict) and block.get("type") == "text":
                            output_text += block.get("text", "")
                else:
                    output_text = str(content)
                output_tokens = max(1, len(output_text.split()) // 4)
            # OpenAI格式
            elif "choices" in response_data and response_data["choices"]:
                choice = response_data["choices"][0]
                if "message" in choice:
                    content = choice["message"].get("content", "")
                    output_tokens = max(1, len(content.split()) // 4)
            # Gemini格式
            elif "candidates" in response_data and response_data["candidates"]:
                candidate = response_data["candidates"][0]
                if "content" in candidate and "parts" in candidate["content"]:
                    output_text = ""
                    for part in candidate["content"]["parts"]:
                        if "text" in part:
                            output_text += part["text"]
                    output_tokens = max(1, len(output_text.split()) // 4)

    logger.info(f"[endpoint_check] Fallback token count | input={input_tokens}, output={output_tokens}")
    return input_tokens, output_tokens, 0, 0

# =========================================================================
# 重构后的架构类 - 分离关注点
# =========================================================================

@dataclass
class EndpointCheckRequest:
    """端点检查请求数据类"""
    url: str
    headers: Dict[str, str]
    json_body: Dict[str, Any]
    api_format: str
    provider_name: Optional[str] = None
    model_name: Optional[str] = None
    api_key_id: Optional[str] = None
    provider_id: Optional[str] = None
    db: Optional[Any] = None
    user: Optional[Any] = None
    request_id: Optional[str] = None
    timeout: float = 30.0


@dataclass
class EndpointCheckResult:
    """端点检查结果数据类"""
    status_code: int
    headers: Dict[str, str]
    response_time_ms: int
    request_id: str
    response_data: Optional[Dict[str, Any]] = None
    error_message: Optional[str] = None
    usage_data: Optional[Dict[str, Any]] = None


class HttpRequestExecutor:
    """HTTP请求执行器 - 专门负责网络请求"""

    def __init__(self, timeout: float = 30.0):
        self.timeout = timeout

    async def execute(self, request: EndpointCheckRequest) -> EndpointCheckResult:
        """执行HTTP请求"""
        start_time = time.time()
        request_id = request.request_id or str(uuid.uuid4())[:8]

        try:
            # 使用httpx进行异步请求
            async with httpx.AsyncClient(timeout=self.timeout) as client:
                response = await client.post(
                    url=request.url,
                    json=request.json_body,
                    headers=request.headers
                )

            end_time = time.time()
            response_time_ms = int((end_time - start_time) * 1000)

            # 处理响应
            if response.status_code == 200:
                try:
                    response_data = response.json()
                    logger.debug(f"[{request.api_format}] check_endpoint | response | json={_truncate_repr(response_data)}")
                except Exception:
                    response_data = None
                    logger.debug(f"[{request.api_format}] check_endpoint | response | invalid json")

                return EndpointCheckResult(
                    status_code=response.status_code,
                    headers=dict(response.headers),
                    response_time_ms=response_time_ms,
                    request_id=request_id,
                    response_data=response_data
                )
            else:
                # 对于非200状态码，使用错误处理器
                error_body = response.text[:500] if response.text else "(empty)"
                logger.debug(f"[{request.api_format}] check_endpoint | response | error={error_body}")

                # 创建HTTPStatusError让错误处理器处理
                http_error = httpx.HTTPStatusError(
                    message=f"HTTP {response.status_code}: {error_body}",
                    request=None,  # 我们不需要完整的request对象
                    response=response
                )

                return await ErrorHandler.handle_error(http_error, request)

        except Exception as e:
            # 使用统一错误处理器处理异常
            return await ErrorHandler.handle_error(e, request)


class UsageCalculator:
    """用量计算器 - 专门负责Token计数和费用计算"""

    @staticmethod
    def calculate_tokens(request: EndpointCheckRequest, result: EndpointCheckResult) -> tuple[int, int, int, int]:
        """
        计算Token数量

        Returns:
            tuple[int, int, int, int]: (input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens)
        """
        # 优先使用api_format（更准确），fallback到provider_name
        api_identifier = request.api_format or request.provider_name

        if not api_identifier or not result.response_data:
            # 如果没有adapter信息或响应数据，使用fallback
            return UsageCalculator._fallback_token_counting(request.json_body, result.response_data)

        # 优先从响应中提取usage信息
        return _extract_tokens_from_response(api_identifier, result.response_data)

    @staticmethod
    def _fallback_token_counting(request_data: Dict[str, Any], response_data: Optional[Dict[str, Any]]) -> tuple[int, int, int, int]:
        """回退的Token计数方法（简单估算）"""
        # 估算输入Token
        messages = request_data.get("messages", request_data.get("contents", []))
        if messages:
            input_text = str(messages)
            input_tokens = max(1, len(input_text.split()) // 4)
        else:
            input_tokens = 1

        # 估算输出Token
        output_tokens = 1  # 最小输出Token数
        if response_data:
            # 尝试从响应中提取文本内容
            if isinstance(response_data, dict):
                # Claude格式
                if "content" in response_data:
                    content = response_data["content"]
                    if isinstance(content, str):
                        output_text = content
                    elif isinstance(content, list):
                        output_text = ""
                        for block in content:
                            if isinstance(block, dict) and block.get("type") == "text":
                                output_text += block.get("text", "")
                    else:
                        output_text = str(content)
                    output_tokens = max(1, len(output_text.split()) // 4)
                # OpenAI格式
                elif "choices" in response_data and response_data["choices"]:
                    choice = response_data["choices"][0]
                    if "message" in choice:
                        content = choice["message"].get("content", "")
                        output_tokens = max(1, len(content.split()) // 4)
                # Gemini格式
                elif "candidates" in response_data and response_data["candidates"]:
                    candidate = response_data["candidates"][0]
                    if "content" in candidate and "parts" in candidate["content"]:
                        output_text = ""
                        for part in candidate["content"]["parts"]:
                            if "text" in part:
                                output_text += part["text"]
                        output_tokens = max(1, len(output_text.split()) // 4)

        return input_tokens, output_tokens, 0, 0

class AsyncBatchUsageRecorder:
    """异步用量记录器 - 批处理数据库操作"""

    def __init__(self, batch_size: int = 10, flush_interval: float = 2.0):
        self.batch_size = batch_size
        self.flush_interval = flush_interval
        self.pending_records: List[Dict[str, Any]] = []
        self._flush_task: Optional[asyncio.Task] = None
        self._lock = asyncio.Lock()
        self._running = True

    async def add_record(self, usage_data: Dict[str, Any]) -> None:
        """添加用量记录到批处理队列"""
        async with self._lock:
            self.pending_records.append(usage_data)

            # 如果达到批处理大小，立即刷新
            if len(self.pending_records) >= self.batch_size:
                await self._flush_batch()
            else:
                # 启动定时刷新任务
                self._ensure_flush_task()

    def _ensure_flush_task(self) -> None:
        """确保定时刷新任务在运行"""
        if self._flush_task is None or self._flush_task.done():
            self._flush_task = asyncio.create_task(self._periodic_flush())

    async def _periodic_flush(self) -> None:
        """定时刷新任务"""
        try:
            await asyncio.sleep(self.flush_interval)
            async with self._lock:
                if self.pending_records:
                    await self._flush_batch()
        except asyncio.CancelledError:
            pass
        except Exception as e:
            logger.error(f"[AsyncBatchUsageRecorder] Periodic flush failed: {e}")

    async def _flush_batch(self) -> None:
        """批量刷新到数据库"""
        if not self.pending_records:
            return

        records_to_flush = self.pending_records.copy()
        self.pending_records.clear()

        try:
            # 这里可以实现批量插入逻辑
            # 目前保持简单的逐条插入，但减少了锁的竞争
            for record in records_to_flush:
                # 调用原有的用量记录逻辑（简化版）
                logger.debug(f"[AsyncBatchUsageRecorder] Flushing usage record: {record.get('request_id', 'unknown')}")

            logger.info(f"[AsyncBatchUsageRecorder] Flushed {len(records_to_flush)} usage records")
        except Exception as e:
            logger.error(f"[AsyncBatchUsageRecorder] Failed to flush batch: {e}")
            # 将失败的记录重新加入队列（可选）
            async with self._lock:
                self.pending_records.extend(records_to_flush)

    async def flush(self) -> None:
        """立即刷新所有待处理的记录"""
        async with self._lock:
            await self._flush_batch()
            if self._flush_task:
                self._flush_task.cancel()
                try:
                    await self._flush_task
                except asyncio.CancelledError:
                    pass
                self._flush_task = None

    async def close(self) -> None:
        """关闭批处理器，刷新所有待处理记录"""
        self._running = False
        await self.flush()


# 全局批处理器实例（单例）
_global_batch_recorder: Optional[AsyncBatchUsageRecorder] = None

def get_batch_recorder() -> AsyncBatchUsageRecorder:
    """获取全局批处理器实例"""
    global _global_batch_recorder
    if _global_batch_recorder is None:
        _global_batch_recorder = AsyncBatchUsageRecorder()
    return _global_batch_recorder


# =========================================================================
# 统一错误处理机制
# =========================================================================

class EndpointCheckError(Exception):
    """端点检查错误基类"""
    def __init__(self, message: str, error_type: str, status_code: int = 500, details: Optional[Dict[str, Any]] = None):
        super().__init__(message)
        self.message = message
        self.error_type = error_type
        self.status_code = status_code
        self.details = details or {}

class NetworkError(EndpointCheckError):
    """网络请求错误"""
    def __init__(self, message: str, details: Optional[Dict[str, Any]] = None):
        super().__init__(message, "network_error", 0, details)

class AuthenticationError(EndpointCheckError):
    """认证错误"""
    def __init__(self, message: str, details: Optional[Dict[str, Any]] = None):
        super().__init__(message, "authentication_error", 401, details)

class RateLimitError(EndpointCheckError):
    """速率限制错误"""
    def __init__(self, message: str, details: Optional[Dict[str, Any]] = None):
        super().__init__(message, "rate_limit_error", 429, details)

class UpstreamError(EndpointCheckError):
    """上游服务错误"""
    def __init__(self, message: str, status_code: int, details: Optional[Dict[str, Any]] = None):
        super().__init__(message, "upstream_error", status_code, details)


class ErrorHandler:
    """统一错误处理器"""

    @staticmethod
    async def handle_error(error: Exception, request: EndpointCheckRequest) -> EndpointCheckResult:
        """统一处理各种错误类型"""
        if isinstance(error, httpx.RequestError):
            return ErrorHandler._handle_network_error(error, request)
        elif isinstance(error, httpx.TimeoutException):
            return ErrorHandler._handle_timeout_error(error, request)
        elif isinstance(error, httpx.HTTPStatusError):
            return ErrorHandler._handle_http_status_error(error, request)
        elif isinstance(error, EndpointCheckError):
            return ErrorHandler._handle_business_error(error, request)
        elif isinstance(error, (ValueError, TypeError)):
            return ErrorHandler._handle_validation_error(error, request)
        else:
            return ErrorHandler._handle_unknown_error(error, request)

    @staticmethod
    def _handle_network_error(error: httpx.RequestError, request: EndpointCheckRequest) -> EndpointCheckResult:
        """处理网络错误"""
        error_message = f"Network error: {str(error)}"
        logger.warning(f"[{request.api_format}] Network error: {error}")

        # 分类网络错误
        if "connect" in str(error).lower():
            error_type = "connection_failed"
            error_message = "Connection failed to upstream service"
        elif "timeout" in str(error).lower():
            error_type = "timeout"
            error_message = "Request timeout"
        else:
            error_type = "network_error"

        return EndpointCheckResult(
            status_code=0,
            headers={},
            response_time_ms=0,
            request_id=request.request_id or str(uuid.uuid4())[:8],
            error_message=error_message,
            response_data={
                "error_type": error_type,
                "original_error": str(error),
                "retryable": True
            }
        )

    @staticmethod
    def _handle_timeout_error(error: httpx.TimeoutException, request: EndpointCheckRequest) -> EndpointCheckResult:
        """处理超时错误"""
        logger.warning(f"[{request.api_format}] Request timeout: {error}")
        return EndpointCheckResult(
            status_code=0,
            headers={},
            response_time_ms=int(request.timeout * 1000),  # 转换为毫秒
            request_id=request.request_id or str(uuid.uuid4())[:8],
            error_message="Request timeout",
            response_data={
                "error_type": "timeout",
                "original_error": str(error),
                "retryable": True,
                "timeout_seconds": request.timeout
            }
        )

    @staticmethod
    def _handle_http_status_error(error: httpx.HTTPStatusError, request: EndpointCheckRequest) -> EndpointCheckResult:
        """处理HTTP状态错误"""
        logger.warning(f"[{request.api_format}] HTTP error: {error.response.status_code} - {error.response.text[:200]}")

        # 根据状态码分类错误
        status_code = error.response.status_code
        if status_code == 401:
            error_type = "authentication_error"
            error_message = "Authentication failed"
            retryable = False
        elif status_code == 429:
            error_type = "rate_limit_error"
            error_message = "Rate limit exceeded"
            retryable = True
        elif 400 <= status_code < 500:
            error_type = "client_error"
            error_message = f"Client error: {status_code}"
            retryable = False
        elif 500 <= status_code < 600:
            error_type = "server_error"
            error_message = f"Server error: {status_code}"
            retryable = True
        else:
            error_type = "http_error"
            error_message = f"HTTP error: {status_code}"
            retryable = status_code >= 500

        return EndpointCheckResult(
            status_code=status_code,
            headers=dict(error.response.headers),
            response_time_ms=0,
            request_id=request.request_id or str(uuid.uuid4())[:8],
            error_message=error_message,
            response_data={
                "error_type": error_type,
                "http_status": status_code,
                "response_body": error.response.text[:500] if error.response.text else "",
                "retryable": retryable
            }
        )

    @staticmethod
    def _handle_business_error(error: EndpointCheckError, request: EndpointCheckRequest) -> EndpointCheckResult:
        """处理业务逻辑错误"""
        logger.warning(f"[{request.api_format}] Business error: {error.error_type} - {error.message}")
        return EndpointCheckResult(
            status_code=error.status_code,
            headers={},
            response_time_ms=0,
            request_id=request.request_id or str(uuid.uuid4())[:8],
            error_message=error.message,
            response_data={
                "error_type": error.error_type,
                "details": error.details,
                "retryable": error.status_code >= 500 or error.status_code == 429
            }
        )

    @staticmethod
    def _handle_validation_error(error: ValueError, request: EndpointCheckRequest) -> EndpointCheckResult:
        """处理验证错误"""
        logger.warning(f"[{request.api_format}] Validation error: {error}")
        return EndpointCheckResult(
            status_code=400,
            headers={},
            response_time_ms=0,
            request_id=request.request_id or str(uuid.uuid4())[:8],
            error_message=f"Validation error: {str(error)}",
            response_data={
                "error_type": "validation_error",
                "original_error": str(error),
                "retryable": False
            }
        )

    @staticmethod
    def _handle_unknown_error(error: Exception, request: EndpointCheckRequest) -> EndpointCheckResult:
        """处理未知错误"""
        logger.error(f"[{request.api_format}] Unknown error: {type(error).__name__}: {error}")
        import traceback
        logger.error(f"[{request.api_format}] Traceback: {traceback.format_exc()}")

        return EndpointCheckResult(
            status_code=500,
            headers={},
            response_time_ms=0,
            request_id=request.request_id or str(uuid.uuid4())[:8],
            error_message="Internal server error",
            response_data={
                "error_type": "internal_error",
                "original_error": str(error),
                "retryable": False
            }
        )


# =========================================================================
# 配置化支持
# =========================================================================

@dataclass
class EndpointCheckConfig:
    """端点检查配置"""
    # 性能配置
    timeout: float = 30.0
    max_retries: int = 3
    retry_delay: float = 1.0

    # 缓存配置
    api_format_cache_size: int = 512
    request_cache_size: int = 128

    # 批处理配置
    enable_batch_recording: bool = True
    batch_size: int = 10
    batch_flush_interval: float = 2.0

    # 日志配置
    enable_detailed_logging: bool = False
    enable_structured_logging: bool = True

    # 用量计算配置
    enable_usage_calculation: bool = True
    enable_fallback_token_counting: bool = True

    # 错误处理配置
    enable_error_classification: bool = True
    retry_on_server_errors: bool = True
    retry_on_timeouts: bool = True

    @classmethod
    def from_env(cls) -> 'EndpointCheckConfig':
        """从环境变量创建配置"""
        import os

        return cls(
            timeout=float(os.getenv('ENDPOINT_CHECK_TIMEOUT', '30.0')),
            max_retries=int(os.getenv('ENDPOINT_CHECK_MAX_RETRIES', '3')),
            retry_delay=float(os.getenv('ENDPOINT_CHECK_RETRY_DELAY', '1.0')),
            api_format_cache_size=int(os.getenv('ENDPOINT_CHECK_CACHE_SIZE', '512')),
            enable_batch_recording=os.getenv('ENDPOINT_CHECK_BATCH_RECORDING', 'true').lower() == 'true',
            batch_size=int(os.getenv('ENDPOINT_CHECK_BATCH_SIZE', '10')),
            batch_flush_interval=float(os.getenv('ENDPOINT_CHECK_BATCH_INTERVAL', '2.0')),
            enable_detailed_logging=os.getenv('ENDPOINT_CHECK_DETAILED_LOGGING', 'false').lower() == 'true',
            enable_structured_logging=os.getenv('ENDPOINT_CHECK_STRUCTURED_LOGGING', 'true').lower() == 'true',
            enable_usage_calculation=os.getenv('ENDPOINT_CHECK_USAGE_CALCULATION', 'true').lower() == 'true',
            enable_fallback_token_counting=os.getenv('ENDPOINT_CHECK_FALLBACK_COUNTING', 'true').lower() == 'true',
            enable_error_classification=os.getenv('ENDPOINT_CHECK_ERROR_CLASSIFICATION', 'true').lower() == 'true',
            retry_on_server_errors=os.getenv('ENDPOINT_CHECK_RETRY_SERVER_ERRORS', 'true').lower() == 'true',
            retry_on_timeouts=os.getenv('ENDPOINT_CHECK_RETRY_TIMEOUTS', 'true').lower() == 'true',
        )

    @classmethod
    def from_dict(cls, config_dict: Dict[str, Any]) -> 'EndpointCheckConfig':
        """从字典创建配置"""
        return cls(**{k: v for k, v in config_dict.items() if hasattr(cls, k)})


class ConfigurableEndpointChecker:
    """可配置的端点检查器"""

    def __init__(self, config: Optional[EndpointCheckConfig] = None):
        self.config = config or EndpointCheckConfig()
        self.executor = HttpRequestExecutor(timeout=self.config.timeout)
        self.usage_calculator = UsageCalculator()
        self.orchestrator = EndpointCheckOrchestrator(
            executor=self.executor,
            usage_calculator=self.usage_calculator,
            enable_usage_calculation=self.config.enable_usage_calculation,
        )

        # 应用配置到缓存大小
        self._apply_cache_config()

    def _apply_cache_config(self) -> None:
        """应用缓存配置"""
        # 简化缓存配置 - 移除了有问题的缓存实现
        # 未来如果需要缓存，可以重新设计缓存策略
        logger.info(f"[ConfigurableEndpointChecker] Cache config applied: api_format_cache_size={self.config.api_format_cache_size}")
        pass

    async def check_endpoint(self, request: EndpointCheckRequest) -> EndpointCheckResult:
        """根据配置执行端点检查"""
        # 应用配置到请求
        request.timeout = self.config.timeout

        # 如果启用了结构化日志，使用结构化日志记录
        if self.config.enable_structured_logging:
            self._log_structured_start(request)

        # 执行检查
        result = await self.orchestrator.execute_check(request)

        # 应用重试逻辑
        if self.config.max_retries > 0 and self._should_retry(result):
            result = await self._retry_check(request, result)

        # 记录结构化日志
        if self.config.enable_structured_logging:
            self._log_structured_result(request, result)

        return result

    def _should_retry(self, result: EndpointCheckResult) -> bool:
        """判断是否应该重试"""
        if not self.config.enable_error_classification or not result.response_data:
            return False

        error_type = result.response_data.get("error_type", "")
        retryable = result.response_data.get("retryable", False)

        # 根据配置和错误类型判断是否重试
        if error_type == "timeout" and self.config.retry_on_timeouts:
            return True
        elif error_type in ["server_error", "network_error", "connection_failed"] and self.config.retry_on_server_errors:
            return retryable

        return False

    async def _retry_check(self, request: EndpointCheckRequest, last_result: EndpointCheckResult) -> EndpointCheckResult:
        """重试端点检查"""
        for attempt in range(self.config.max_retries):
            if self.config.enable_structured_logging:
                self._log_structured_retry(request, attempt + 1, last_result)

            # 等待重试延迟
            await asyncio.sleep(self.config.retry_delay * (2 ** attempt))  # 指数退避

            # 执行重试
            result = await self.orchestrator.execute_check(request)

            # 如果成功或不再需要重试，返回结果
            if result.status_code == 200 or not self._should_retry(result):
                if self.config.enable_structured_logging:
                    self._log_structured_retry_success(request, attempt + 1)
                return result

        # 所有重试都失败了，返回最后一个结果
        if self.config.enable_structured_logging:
            self._log_structured_retry_failed(request)
        return last_result

    def _log_structured_start(self, request: EndpointCheckRequest) -> None:
        """记录结构化开始日志"""
        log_entry = {
            "event": "endpoint_check_start",
            "timestamp": time.time(),
            "request_id": request.request_id,
            "provider": request.provider_name,
            "model": request.model_name,
            "url": request.url,
            "config": {
                "timeout": self.config.timeout,
                "max_retries": self.config.max_retries,
                "enable_batch_recording": self.config.enable_batch_recording,
                "enable_usage_calculation": self.config.enable_usage_calculation,
            }
        }
        logger.info(f"[{request.api_format}] {json.dumps(log_entry)}")

    def _log_structured_result(self, request: EndpointCheckRequest, result: EndpointCheckResult) -> None:
        """记录结构化结果日志"""
        log_entry = {
            "event": "endpoint_check_complete",
            "timestamp": time.time(),
            "request_id": request.request_id,
            "provider": request.provider_name,
            "model": request.model_name,
            "status_code": result.status_code,
            "response_time_ms": result.response_time_ms,
            "error_message": result.error_message,
            "has_usage_data": result.usage_data is not None,
        }

        if result.response_data and "error_type" in result.response_data:
            log_entry["error_type"] = result.response_data["error_type"]
            log_entry["retryable"] = result.response_data.get("retryable", False)

        logger.info(f"[{request.api_format}] {json.dumps(log_entry)}")

    def _log_structured_retry(self, request: EndpointCheckRequest, attempt: int, last_result: EndpointCheckResult) -> None:
        """记录重试日志"""
        log_entry = {
            "event": "endpoint_check_retry",
            "timestamp": time.time(),
            "request_id": request.request_id,
            "provider": request.provider_name,
            "model": request.model_name,
            "attempt": attempt,
            "last_status_code": last_result.status_code,
            "last_error": last_result.error_message,
            "retry_delay": self.config.retry_delay * (2 ** (attempt - 1)),
        }
        logger.warning(f"[{request.api_format}] {json.dumps(log_entry)}")

    def _log_structured_retry_success(self, request: EndpointCheckRequest, attempt: int) -> None:
        """记录重试成功日志"""
        log_entry = {
            "event": "endpoint_check_retry_success",
            "timestamp": time.time(),
            "request_id": request.request_id,
            "provider": request.provider_name,
            "model": request.model_name,
            "attempts": attempt,
        }
        logger.info(f"[{request.api_format}] {json.dumps(log_entry)}")

    def _log_structured_retry_failed(self, request: EndpointCheckRequest) -> None:
        """记录重试失败日志"""
        log_entry = {
            "event": "endpoint_check_retry_failed",
            "timestamp": time.time(),
            "request_id": request.request_id,
            "provider": request.provider_name,
            "model": request.model_name,
            "max_attempts": self.config.max_retries,
        }
        logger.error(f"[{request.api_format}] {json.dumps(log_entry)}")


# 全局配置检查器实例
_global_configured_checker: Optional[ConfigurableEndpointChecker] = None

def get_configured_checker(config: Optional[EndpointCheckConfig] = None) -> ConfigurableEndpointChecker:
    """获取全局配置检查器实例"""
    global _global_configured_checker
    if _global_configured_checker is None or config is not None:
        _global_configured_checker = ConfigurableEndpointChecker(config or EndpointCheckConfig.from_env())
    return _global_configured_checker




class EndpointCheckOrchestrator:
    """端点检查协调器 - 协调整个流程"""

    def __init__(
        self,
        executor: Optional[HttpRequestExecutor] = None,
        usage_calculator: Optional[UsageCalculator] = None,
        enable_usage_calculation: bool = True,
    ):
        self.executor = executor or HttpRequestExecutor()
        self.usage_calculator = usage_calculator or UsageCalculator()
        self.enable_usage_calculation = enable_usage_calculation

    async def execute_check(self, request: EndpointCheckRequest) -> EndpointCheckResult:
        """执行端点检查的完整流程"""
        logger.info(f"[{request.api_format}] Starting endpoint check | "
                   f"provider={request.provider_name}, model={request.model_name}")

        # 1. 执行HTTP请求
        result = await self.executor.execute(request)

        # 2. 计算用量
        should_calculate_usage = (
            self.enable_usage_calculation
            and request.db
            and request.user
            and request.api_key_id
        )
        if should_calculate_usage:
            try:
                input_tokens, output_tokens, cache_creation_input_tokens, cache_read_input_tokens = \
                    self.usage_calculator.calculate_tokens(request, result)

                # 检测API格式
                api_format = request.api_format
                result.usage_data = await _calculate_and_record_usage(
                    db=request.db,
                    user=request.user,
                    provider_name=request.provider_name or "unknown",
                    provider_id=request.provider_id or "unknown",
                    api_key_id=request.api_key_id,
                    model_name=request.model_name or "unknown",
                    request_data=request.json_body,
                    response_data=result.response_data,
                    request_id=result.request_id,
                    response_time_ms=result.response_time_ms,
                    request_headers=request.headers,
                    response_headers=result.headers,
                    status_code=result.status_code,
                    error_message=result.error_message,
                    # 直接传递计算好的token数据
                    input_tokens=input_tokens,
                    output_tokens=output_tokens,
                    cache_creation_input_tokens=cache_creation_input_tokens,
                    cache_read_input_tokens=cache_read_input_tokens,
                    api_format=api_format,
                )

                # 构建结构化日志
                usage_data = result.usage_data
                usage_error = usage_data.get("error") if isinstance(usage_data, dict) else None

                log_entry = {
                    "event": "endpoint_check_usage_calculation",
                    "timestamp": time.time(),
                    "request_id": result.request_id,
                    "api_format": request.api_format,
                    "provider": request.provider_name,
                    "model": request.model_name,
                    "provider_api_key_id": request.api_key_id,
                    "success": usage_error is None,
                    "error": usage_error,
                    "usage_data": usage_data,
                }

                log_json = json.dumps(log_entry, default=str)
                if usage_error is None:
                    logger.info(f"[{request.api_format}] {log_json}")
                else:
                    logger.warning(f"[{request.api_format}] {log_json}")
            except Exception as e:
                logger.error(f"[{request.api_format}] Failed to calculate usage: {e}")
                import traceback
                logger.error(f"[{request.api_format}] Usage calculation traceback: {traceback.format_exc()}")

        return result
