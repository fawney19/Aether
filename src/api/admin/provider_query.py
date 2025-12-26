"""
Provider Query API 端点
用于查询提供商的模型列表等信息
"""

import asyncio
from typing import Optional

import httpx
from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel
from sqlalchemy.orm import Session, joinedload

from src.api.handlers.base.chat_adapter_base import get_adapter_class
from src.api.handlers.base.cli_adapter_base import get_cli_adapter_class
from src.core.crypto import crypto_service
from src.core.logger import logger
from src.database.database import get_db
from src.models.database import Model, Provider, ProviderEndpoint, User
from src.utils.auth_utils import get_current_user

router = APIRouter(prefix="/api/admin/provider-query", tags=["Provider Query"])


# ============ Request/Response Models ============


class ModelsQueryRequest(BaseModel):
    """模型列表查询请求"""

    provider_id: str
    api_key_id: Optional[str] = None


class TestModelRequest(BaseModel):
    """模型测试请求"""

    provider_id: str
    model_name: str
    api_key_id: Optional[str] = None
    stream: bool = False
    message: Optional[str] = "你好"
    api_format: Optional[str] = None  # 指定使用的API格式，如果不指定则使用端点的默认格式


# ============ API Endpoints ============


def _get_adapter_for_format(api_format: str):
    """根据 API 格式获取对应的 Adapter 类"""
    # 先检查 Chat Adapter 注册表
    adapter_class = get_adapter_class(api_format)
    if adapter_class:
        return adapter_class

    # 再检查 CLI Adapter 注册表
    cli_adapter_class = get_cli_adapter_class(api_format)
    if cli_adapter_class:
        return cli_adapter_class

    return None


@router.post("/models")
async def query_available_models(
    request: ModelsQueryRequest,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    """
    查询提供商可用模型

    遍历所有活跃端点，根据端点的 API 格式选择正确的 Adapter 进行请求：
    - OPENAI/OPENAI_CLI: 使用 OpenAIChatAdapter.fetch_models
    - CLAUDE/CLAUDE_CLI: 使用 ClaudeChatAdapter.fetch_models
    - GEMINI/GEMINI_CLI: 使用 GeminiChatAdapter.fetch_models

    Args:
        request: 查询请求

    Returns:
        所有端点的模型列表（合并）
    """
    # 获取提供商及其端点
    provider = (
        db.query(Provider)
        .options(joinedload(Provider.endpoints).joinedload(ProviderEndpoint.api_keys))
        .filter(Provider.id == request.provider_id)
        .first()
    )

    if not provider:
        raise HTTPException(status_code=404, detail="Provider not found")

    # 收集所有活跃端点的配置
    endpoint_configs: list[dict] = []

    if request.api_key_id:
        # 指定了特定的 API Key，只使用该 Key 对应的端点
        for endpoint in provider.endpoints:
            for api_key in endpoint.api_keys:
                if api_key.id == request.api_key_id:
                    try:
                        api_key_value = crypto_service.decrypt(api_key.api_key)
                    except Exception as e:
                        logger.error(f"Failed to decrypt API key: {e}")
                        raise HTTPException(status_code=500, detail="Failed to decrypt API key")
                    endpoint_configs.append({
                        "api_key": api_key_value,
                        "base_url": endpoint.base_url,
                        "api_format": endpoint.api_format,
                        "extra_headers": endpoint.headers,
                    })
                    break
            if endpoint_configs:
                break

        if not endpoint_configs:
            raise HTTPException(status_code=404, detail="API Key not found")
    else:
        # 遍历所有活跃端点，每个端点取第一个可用的 Key
        for endpoint in provider.endpoints:
            if not endpoint.is_active or not endpoint.api_keys:
                continue

            # 找第一个可用的 Key
            for api_key in endpoint.api_keys:
                if api_key.is_active:
                    try:
                        api_key_value = crypto_service.decrypt(api_key.api_key)
                    except Exception as e:
                        logger.error(f"Failed to decrypt API key: {e}")
                        continue  # 尝试下一个 Key
                    endpoint_configs.append({
                        "api_key": api_key_value,
                        "base_url": endpoint.base_url,
                        "api_format": endpoint.api_format,
                        "extra_headers": endpoint.headers,
                    })
                    break  # 只取第一个可用的 Key

        if not endpoint_configs:
            raise HTTPException(status_code=400, detail="No active API Key found for this provider")

    # 并发请求所有端点的模型列表
    all_models: list = []
    errors: list[str] = []

    async def fetch_endpoint_models(
        client: httpx.AsyncClient, config: dict
    ) -> tuple[list, Optional[str]]:
        base_url = config["base_url"]
        if not base_url:
            return [], None
        base_url = base_url.rstrip("/")
        api_format = config["api_format"]
        api_key_value = config["api_key"]
        extra_headers = config.get("extra_headers")

        try:
            # 获取对应的 Adapter 类并调用 fetch_models
            adapter_class = _get_adapter_for_format(api_format)
            if not adapter_class:
                return [], f"Unknown API format: {api_format}"
            models, error = await adapter_class.fetch_models(
                client, base_url, api_key_value, extra_headers
            )
            # 确保所有模型都有 api_format 字段
            for m in models:
                if "api_format" not in m:
                    m["api_format"] = api_format
            return models, error
        except Exception as e:
            logger.error(f"Error fetching models from {api_format} endpoint: {e}")
            return [], f"{api_format}: {str(e)}"

    # 限制并发请求数量，避免触发上游速率限制
    MAX_CONCURRENT_REQUESTS = 5
    semaphore = asyncio.Semaphore(MAX_CONCURRENT_REQUESTS)

    async def fetch_with_semaphore(
        client: httpx.AsyncClient, config: dict
    ) -> tuple[list, Optional[str]]:
        async with semaphore:
            return await fetch_endpoint_models(client, config)

    async with httpx.AsyncClient(timeout=30.0) as client:
        results = await asyncio.gather(
            *[fetch_with_semaphore(client, c) for c in endpoint_configs]
        )
        for models, error in results:
            all_models.extend(models)
            if error:
                errors.append(error)

    # 查询该 Provider 下所有模型的映射关系，标记已映射的上游模型
    mapped_model_names = {
        mapping["name"]
        for prov_model in db.query(Model).filter(Model.provider_id == request.provider_id).all()
        if prov_model.provider_model_mappings
        for mapping in prov_model.provider_model_mappings
        if isinstance(mapping, dict) and mapping.get("name")
    }

    # 按 model id + api_format 去重（保留第一个），同时标记 mapped
    seen_keys: set[str] = set()
    unique_models: list = []
    for model in all_models:
        model_id = model.get("id")
        api_format = model.get("api_format", "")
        unique_key = f"{model_id}:{api_format}"
        if model_id and unique_key not in seen_keys:
            seen_keys.add(unique_key)
            model["mapped"] = model_id in mapped_model_names
            unique_models.append(model)

    error = "; ".join(errors) if errors else None
    if not unique_models and not error:
        error = "No models returned from any endpoint"

    return {
        "success": len(unique_models) > 0,
        "data": {"models": unique_models, "error": error},
        "provider": {
            "id": provider.id,
            "name": provider.name,
            "display_name": provider.display_name,
        },
    }


@router.post("/test-model")
async def test_model(
    request: TestModelRequest,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    """
    测试模型连接性

    向指定提供商的指定模型发送测试请求，验证模型是否可用

    Args:
        request: 测试请求

    Returns:
        测试结果
    """
    # 获取提供商及其端点
    provider = (
        db.query(Provider)
        .options(joinedload(Provider.endpoints).joinedload(ProviderEndpoint.api_keys))
        .filter(Provider.id == request.provider_id)
        .first()
    )

    if not provider:
        raise HTTPException(status_code=404, detail="Provider not found")

    # 找到合适的端点和API Key
    endpoint_config = None
    endpoint = None
    api_key = None

    if request.api_key_id:
        # 使用指定的API Key
        for ep in provider.endpoints:
            for key in ep.api_keys:
                if key.id == request.api_key_id and key.is_active and ep.is_active:
                    endpoint = ep
                    api_key = key
                    break
            if endpoint:
                break
    else:
        # 使用第一个可用的端点和密钥
        for ep in provider.endpoints:
            if not ep.is_active or not ep.api_keys:
                continue
            for key in ep.api_keys:
                if key.is_active:
                    endpoint = ep
                    api_key = key
                    break
            if endpoint:
                break

    if not endpoint or not api_key:
        raise HTTPException(status_code=404, detail="No active endpoint or API key found")

    try:
        api_key_value = crypto_service.decrypt(api_key.api_key)
    except Exception as e:
        logger.error(f"[test-model] Failed to decrypt API key: {e}")
        raise HTTPException(status_code=500, detail="Failed to decrypt API key")

    # 构建请求配置
    endpoint_config = {
        "api_key": api_key_value,
        "api_key_id": api_key.id,  # 添加API Key ID用于用量记录
        "base_url": endpoint.base_url,
        "api_format": endpoint.api_format,
        "extra_headers": endpoint.headers,
        "timeout": endpoint.timeout or 30.0,
    }

    try:
        # 获取对应的 Adapter 类
        adapter_class = _get_adapter_for_format(endpoint.api_format)
        if not adapter_class:
            return {
                "success": False,
                "error": f"Unknown API format: {endpoint.api_format}",
                "provider": {
                    "id": provider.id,
                    "name": provider.name,
                    "display_name": provider.display_name,
                },
                "model": request.model_name,
            }

        logger.debug(f"[test-model] 使用 Adapter: {adapter_class.__name__}")
        logger.debug(f"[test-model] 端点 API Format: {endpoint.api_format}")

        # 如果请求指定了 api_format，优先使用它
        target_api_format = request.api_format or endpoint.api_format
        if request.api_format and request.api_format != endpoint.api_format:
            logger.debug(f"[test-model] 请求指定 API Format: {request.api_format}")
            # 重新获取适配器
            adapter_class = _get_adapter_for_format(request.api_format)
            if not adapter_class:
                return {
                    "success": False,
                    "error": f"Unknown API format: {request.api_format}",
                    "provider": {
                        "id": provider.id,
                        "name": provider.name,
                        "display_name": provider.display_name,
                    },
                    "model": request.model_name,
                }
            logger.debug(f"[test-model] 重新选择 Adapter: {adapter_class.__name__}")

        # 准备测试请求数据
        check_request = {
            "model": request.model_name,
            "messages": [
                {"role": "user", "content": request.message or "Hello! This is a test message."}
            ],
            "max_tokens": 30,
            "temperature": 0.7,
        }

        # 发送测试请求
        async with httpx.AsyncClient(timeout=endpoint_config["timeout"]) as client:
            # 非流式测试
            logger.debug(f"[test-model] 开始非流式测试...")

            response = await adapter_class.check_endpoint(
                client,
                endpoint_config["base_url"],
                endpoint_config["api_key"],
                check_request,
                endpoint_config.get("extra_headers"),
                # 用量计算参数（现在强制记录）
                db=db,
                user=current_user,
                provider_name=provider.name,
                provider_id=provider.id,
                api_key_id=endpoint_config.get("api_key_id"),
                model_name=request.model_name,
            )

            # 记录提供商返回信息
            logger.debug(f"[test-model] 非流式测试结果:")
            logger.debug(f"[test-model] Status Code: {response.get('status_code')}")
            logger.debug(f"[test-model] Response Headers: {response.get('headers', {})}")
            response_data = response.get('response', {})
            response_body = response_data.get('response_body', {})
            logger.debug(f"[test-model] Response Data: {response_data}")
            logger.debug(f"[test-model] Response Body: {response_body}")
            # 尝试解析 response_body (通常是 JSON 字符串)
            parsed_body = response_body
            import json
            if isinstance(response_body, str):
                try:
                    parsed_body = json.loads(response_body)
                except json.JSONDecodeError:
                    pass

            if isinstance(parsed_body, dict) and 'error' in parsed_body:
                error_obj = parsed_body['error']
                # 兼容 error 可能是字典或字符串的情况
                if isinstance(error_obj, dict):
                    logger.debug(f"[test-model] Error Message: {error_obj.get('message')}")
                    raise HTTPException(status_code=500, detail=error_obj.get('message'))
                else:
                    logger.debug(f"[test-model] Error: {error_obj}")
                    raise HTTPException(status_code=500, detail=error_obj)
            elif 'error' in response:
                logger.debug(f"[test-model] Error: {response['error']}")
                raise HTTPException(status_code=500, detail=response['error'])
            else:
                # 如果有选择或消息，记录内容预览
                if isinstance(response_data, dict):
                    if 'choices' in response_data and response_data['choices']:
                        choice = response_data['choices'][0]
                        if 'message' in choice:
                            content = choice['message'].get('content', '')
                            logger.debug(f"[test-model] Content Preview: {content[:200]}...")
                    elif 'content' in response_data and response_data['content']:
                        content = str(response_data['content'])
                        logger.debug(f"[test-model] Content Preview: {content[:200]}...")

            # 检查测试是否成功（基于HTTP状态码）
            status_code = response.get('status_code', 0)
            is_success = status_code == 200 and 'error' not in response

            return {
                "success": is_success,
                "data": {
                    "stream": False,
                    "response": response,
                },
                "provider": {
                    "id": provider.id,
                    "name": provider.name,
                    "display_name": provider.display_name,
                },
                "model": request.model_name,
                "endpoint": {
                    "id": endpoint.id,
                    "api_format": endpoint.api_format,
                    "base_url": endpoint.base_url,
                },
            }

    except Exception as e:
        logger.error(f"[test-model] Error testing model {request.model_name}: {e}")
        return {
            "success": False,
            "error": str(e),
            "provider": {
                "id": provider.id,
                "name": provider.name,
                "display_name": provider.display_name,
            },
            "model": request.model_name,
            "endpoint": {
                "id": endpoint.id,
                "api_format": endpoint.api_format,
                "base_url": endpoint.base_url,
            } if endpoint else None,
        }
