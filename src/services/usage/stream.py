"""
流式响应用量统计服务
处理流式响应的token计算和使用量记录
"""

from __future__ import annotations

import asyncio
import json
from collections import deque
from collections.abc import AsyncIterator
from typing import Any

from sqlalchemy.orm import Session

from src.core.exceptions import EmptyStreamException
from src.core.logger import logger
from src.core.stream_types import StreamStats, get_parser_for_format
from src.database.database import create_session
from src.models.database import ApiKey, User
from src.services.usage.service import UsageService


class StreamUsageTracker:
    """流式响应用量跟踪器"""

    def __init__(
        self,
        db: Session,
        user: User,
        api_key: ApiKey,
        provider: str,
        model: str,
        request_headers: dict[str, Any] | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        request_id: str | None = None,
        start_time: float | None = None,
        attempt_id: str | None = None,
        # Provider 侧追踪信息（用于记录真实成本）
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        # API 格式（用于选择正确的响应解析器）
        api_format: str | None = None,
        # 结构化格式维度（从 Adapter 层透传）
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        # 格式转换信息
        endpoint_api_format: str | None = None,
        has_format_conversion: bool = False,
    ):
        """
        初始化流式用量跟踪器

        Args:
            db: 数据库会话
            user: 用户对象
            api_key: API密钥对象
            provider: 提供商名称
            model: 模型名称
            request_headers: 实际的请求头
            provider_request_headers: 向提供商发送的请求头
            request_id: 请求ID（用于日志关联）
            start_time: 请求开始时间（用于计算总响应时间）
            attempt_id: RequestTrace 请求尝试ID
            provider_id: Provider ID（用于记录真实成本）
            provider_endpoint_id: Endpoint ID（用于记录真实成本）
            provider_api_key_id: API Key ID（用于记录真实成本）
            api_format: endpoint signature（如 "claude:chat", "openai:cli"）
            endpoint_api_format: 端点原生 API 格式
            has_format_conversion: 是否发生了格式转换
        """
        self.db = db
        # 只存储ID，避免会话绑定问题
        self.user_id = user.id if user else None
        self.api_key_id = api_key.id if api_key else None
        self.provider = provider
        self.model = model
        self.request_headers = request_headers or {}
        self.provider_request_headers = provider_request_headers or {}
        self.request_id = request_id
        self.request_start_time = start_time

        # Provider 侧追踪信息
        self.provider_id = provider_id
        self.provider_endpoint_id = provider_endpoint_id
        self.provider_api_key_id = provider_api_key_id

        # API 格式和响应解析器
        self.api_format = api_format or "claude:chat"
        self.api_family = api_family
        self.endpoint_kind = endpoint_kind
        self.endpoint_api_format = endpoint_api_format
        self.has_format_conversion = has_format_conversion
        self.response_parser = get_parser_for_format(self.api_format)
        self.stream_stats = StreamStats()  # 解析器统计信息

        # Token计数
        self.input_tokens = 0
        self.output_tokens = 0
        self.cache_creation_input_tokens = 0
        self.cache_read_input_tokens = 0
        self.cache_creation_input_tokens_5m = 0
        self.cache_creation_input_tokens_1h = 0
        self._accumulated_content_len = 0  # 仅记录长度，不存储实际文本

        # 完整响应跟踪（仅用于内部统计，不记录到数据库）
        self._complete_response_content_chars = 0  # content 累计字符数
        self._COMPLETE_RESPONSE_CONTENT_MAX_CHARS = 64 * 1024  # 64KB 上限
        self.complete_response = {
            "id": None,
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": model,
            "stop_reason": None,
            "stop_sequence": None,
            "usage": {},
        }
        self.response_chunks = []  # 保存解析后的响应块
        self.response_chunks_count = 0  # 响应块总计数（含被丢弃的）
        self.response_chunks_size = 0  # 响应块累计序列化大小（字节）
        self._response_chunks_max_size = 4 * 1024 * 1024  # 4MB，留余量给 truncate_body 的 5MB 上限
        self.raw_chunks: deque[str | bytes] = deque(
            maxlen=50
        )  # 仅保留最后50个原始chunk（用于错误诊断）

        # 请求体（由 track_stream 设置，初始化为 None 以消除 hasattr 检查）
        self.request_data: dict[str, Any] | None = None

        # 时间跟踪
        self.start_time = None
        self.end_time = None

        # 响应头 (将在track_stream中设置)
        self.response_headers = {}

        # SSE解析缓冲区
        self.buffer = b""  # 用于处理不完整的字节流
        self.current_line = ""  # 用于累积SSE行
        self.sse_event_buffer = {
            "event": None,
            "data": [],
            "id": None,
            "retry": None,
        }  # SSE事件缓冲

        # 错误状态跟踪
        self.status_code = 200  # 默认成功状态码
        self.error_message = None  # 错误消息(如果有)
        self.attempt_id = attempt_id

    def _append_response_chunk(self, data: dict[str, Any]) -> None:
        """追加响应块，超过大小限制后只计数不存储"""
        self.response_chunks_count += 1
        if self.response_chunks_size < self._response_chunks_max_size:
            chunk_size = len(json.dumps(data, ensure_ascii=False))
            self.response_chunks_size += chunk_size
            if self.response_chunks_size <= self._response_chunks_max_size:
                self.response_chunks.append(data)

    def set_error_status(self, status_code: int, error_message: str) -> None:
        """
        设置错误状态

        Args:
            status_code: HTTP状态码
            error_message: 错误消息
        """
        self.status_code = status_code
        self.error_message = error_message
        logger.debug(
            f"ID:{self.request_id} | 流式响应错误状态已设置 | 状态码:{status_code} | 错误:{error_message[:100]}"
        )

    def _update_complete_response(self, chunk: dict[str, Any]) -> None:
        """根据响应块更新完整响应结构（文本累积受 64KB 上限保护）"""
        try:
            # 更新响应ID
            if chunk.get("id"):
                self.complete_response["id"] = chunk["id"]

            # 更新模型
            if chunk.get("model"):
                self.complete_response["model"] = chunk["model"]

            # 处理不同类型的事件
            event_type = chunk.get("type")

            if event_type == "message_start":
                # 消息开始事件
                message = chunk.get("message", {})
                if message.get("id"):
                    self.complete_response["id"] = message["id"]
                if message.get("model"):
                    self.complete_response["model"] = message["model"]
                self.complete_response["usage"] = message.get("usage", {})

            elif event_type == "content_block_start":
                # 内容块开始
                content_block = chunk.get("content_block", {})
                self.complete_response["content"].append(content_block)

            elif event_type == "content_block_delta":
                # 内容块增量更新
                index = chunk.get("index", 0)
                delta = chunk.get("delta", {})

                # 确保content列表有足够的元素
                while len(self.complete_response["content"]) <= index:
                    self.complete_response["content"].append({"type": "text", "text": ""})

                current_block = self.complete_response["content"][index]

                # 超过上限后停止累积文本/JSON，仅保留前缀用于调试
                over_limit = (
                    self._complete_response_content_chars
                    >= self._COMPLETE_RESPONSE_CONTENT_MAX_CHARS
                )

                if delta.get("type") == "text_delta":
                    # 文本增量
                    text = delta.get("text", "")
                    self._complete_response_content_chars += len(text)
                    if not over_limit and current_block.get("type") == "text":
                        current_block["text"] = current_block.get("text", "") + text
                elif delta.get("type") == "input_json_delta":
                    # 工具调用输入增量
                    partial = delta.get("partial_json", "")
                    self._complete_response_content_chars += len(partial)
                    if not over_limit and current_block.get("type") == "tool_use":
                        current_input = current_block.get("input", {})
                        if isinstance(current_input, str):
                            current_input += partial
                        current_block["input"] = current_input

            elif event_type == "content_block_stop":
                # 内容块结束
                pass

            elif event_type == "message_delta":
                # 消息级别的增量更新
                delta = chunk.get("delta", {})
                if delta.get("stop_reason"):
                    self.complete_response["stop_reason"] = delta["stop_reason"]
                if delta.get("stop_sequence"):
                    self.complete_response["stop_sequence"] = delta["stop_sequence"]

            elif event_type == "message_stop":
                # 消息结束
                pass

            # 更新usage信息
            if chunk.get("usage"):
                self.complete_response["usage"].update(chunk["usage"])

        except Exception as e:
            # 记录错误但不中断流处理
            logger.warning(f"Failed to update complete response: {e}")

    def estimate_input_tokens(self, messages: list) -> int:
        """
        估算输入tokens

        Args:
            messages: 消息列表

        Returns:
            估算的token数
        """
        total_chars = 0
        for msg in messages:
            if isinstance(msg, dict):
                content = msg.get("content", "")
                if isinstance(content, str):
                    total_chars += len(content)
                elif isinstance(content, list):
                    for block in content:
                        if hasattr(block, "text"):
                            total_chars += len(block.text)

        # 粗略估算：4个字符约等于1个token
        return max(1, total_chars // 4)

    def _process_sse_event(self) -> tuple[str | None, dict[str, Any] | None]:
        """
        处理缓冲区中的完整SSE事件

        Returns:
            (内容文本, 使用信息)
        """
        content = None
        usage = None

        # 如果没有data，直接返回
        if not self.sse_event_buffer["data"]:
            return None, None

        # 合并所有data行（根据SSE规范，每个data行之间加入换行符）
        data_str = "\n".join(self.sse_event_buffer["data"])

        # 清空缓冲区
        self.sse_event_buffer = {"event": None, "data": [], "id": None, "retry": None}

        if not data_str or data_str == "[DONE]":
            return None, None

        try:
            data = json.loads(data_str)

            if isinstance(data, dict):
                self._append_response_chunk(data)
                try:
                    self._update_complete_response(data)
                except Exception as update_error:
                    logger.warning(f"Failed to update complete response from chunk: {update_error}")

                # Claude格式
                if "type" in data:
                    if data["type"] == "content_block_delta":
                        delta = data.get("delta", {})
                        content = delta.get("text", "")
                        if content:
                            logger.debug(f"Extracted content from delta: {len(content)} chars")
                    elif data["type"] == "message_delta":
                        usage_data = data.get("usage", {})
                        if usage_data:
                            usage = usage_data
                            logger.debug(f"Extracted usage from message_delta: {usage}")
                    elif data["type"] == "message_stop":
                        logger.debug("Received message_stop event")

                # OpenAI格式
                elif "choices" in data:
                    choices = data.get("choices", [])
                    if choices:
                        delta = choices[0].get("delta", {})
                        content = delta.get("content", "")

                    if "usage" in data:
                        usage = data["usage"]

        except json.JSONDecodeError as e:
            # 更详细的JSON解析错误日志
            logger.warning(f"Failed to parse SSE JSON data: {str(e)}")
            return None, None
        except Exception as e:
            logger.error(f"Unexpected error processing SSE event: {type(e).__name__}: {str(e)}")

        return content, usage

    def parse_sse_line(self, line: str) -> tuple[str | None, dict[str, Any] | None]:
        """
        解析单行SSE事件（使用统一响应解析器）

        Args:
            line: SSE格式的一行

        Returns:
            (内容文本, 使用信息) - 当遇到空行时处理完整事件
        """
        # 使用统一的响应解析器
        chunk = self.response_parser.parse_sse_line(line, self.stream_stats)

        if chunk is None:
            return None, None

        # 从 ParsedChunk 中提取内容和使用信息
        content = chunk.text_delta

        # 构建 usage 字典（如果有 token 信息）
        usage = None
        if (
            chunk.input_tokens
            or chunk.output_tokens
            or chunk.cache_creation_tokens
            or chunk.cache_read_tokens
        ):
            usage = {
                "input_tokens": chunk.input_tokens or self.stream_stats.input_tokens,
                "output_tokens": chunk.output_tokens or self.stream_stats.output_tokens,
                "cache_creation_input_tokens": chunk.cache_creation_tokens
                or self.stream_stats.cache_creation_tokens,
                "cache_read_input_tokens": chunk.cache_read_tokens
                or self.stream_stats.cache_read_tokens,
            }

        # 更新响应 ID
        if chunk.response_id and not self.complete_response.get("id"):
            self.complete_response["id"] = chunk.response_id

        # 更新完整响应（如果有数据）
        if chunk.data:
            self._append_response_chunk(chunk.data)
            try:
                self._update_complete_response(chunk.data)
            except Exception as update_error:
                logger.warning(f"Failed to update complete response from chunk: {update_error}")

        return content, usage

    def parse_stream_chunk(self, chunk: bytes) -> tuple[str | None, dict[str, Any] | None]:
        """
        解析流式响应块（处理原始字节流）

        Args:
            chunk: 原始字节流

        Returns:
            (累积的内容文本, 使用信息)
        """
        total_content = ""
        final_usage = None

        # 将新chunk添加到缓冲区
        # 确保 chunk 是字节类型
        if isinstance(chunk, str):
            chunk = chunk.encode("utf-8")
        self.buffer += chunk

        # 尝试解码并处理完整的行
        try:
            # 尝试解码整个缓冲区
            text = self.buffer.decode("utf-8")
            self.buffer = b""  # 清空缓冲区

            # 将文本添加到当前行
            self.current_line += text

            # 按换行符分割，处理完整的行
            lines = self.current_line.split("\n")

            # 最后一个可能是不完整的行，保留它
            self.current_line = lines[-1]

            # 处理完整的行
            for line in lines[:-1]:
                line = line.rstrip("\r")
                content, usage = self.parse_sse_line(line)
                if content:
                    total_content += content
                if usage:
                    final_usage = usage

        except UnicodeDecodeError:
            # 如果解码失败，说明缓冲区中有不完整的UTF-8序列
            # 尝试找到最后一个完整的字符边界
            for i in range(len(self.buffer) - 1, max(0, len(self.buffer) - 4), -1):
                try:
                    text = self.buffer[:i].decode("utf-8")
                    # 成功解码，处理这部分
                    remaining = self.buffer[i:]
                    self.buffer = remaining  # 保留未解码的部分

                    # 处理解码的文本
                    self.current_line += text
                    lines = self.current_line.split("\n")
                    self.current_line = lines[-1]

                    for line in lines[:-1]:
                        line = line.rstrip("\r")
                        content, usage = self.parse_sse_line(line)
                        if content:
                            total_content += content
                        if usage:
                            final_usage = usage
                    break
                except UnicodeDecodeError:
                    continue

        return total_content if total_content else None, final_usage

    async def track_stream(
        self,
        stream: AsyncIterator[str],
        request_data: dict[str, Any],
        response_headers: dict[str, Any] | None = None,
    ) -> AsyncIterator[str]:
        """
        跟踪流式响应并计算用量

        Args:
            stream: 原始流式响应
            request_data: 请求数据
            response_headers: 实际的响应头

        Yields:
            流式响应块
        """
        import time

        from src.core.usage_tokens import extract_cache_creation_tokens_detail

        self.start_time = time.time()
        self.request_data = request_data  # 保存请求数据

        # 保存响应头（如果没有提供，使用空字典而不是默认值）
        # 这样可以确保记录的是实际的响应头，而不是构造的默认值
        self.response_headers = response_headers if response_headers is not None else {}

        # 估算输入tokens
        messages = request_data.get("messages", [])
        self.input_tokens = self.estimate_input_tokens(messages)

        logger.debug(
            f"ID:{self.request_id} | 开始跟踪流式响应 | 估算输入tokens:{self.input_tokens}"
        )

        chunk_count = 0
        first_byte_time_ms = None  # 预先记录 TTFB，避免 yield 后计算不准确
        try:
            async for chunk in stream:
                chunk_count += 1
                # 保存原始字节流（用于错误诊断）
                self.raw_chunks.append(chunk)

                # 第一个 chunk 收到时，记录 TTFB 时间点（但先不更新数据库，避免阻塞）
                if chunk_count == 1:
                    # 计算 TTFB（使用请求原始开始时间或 track_stream 开始时间）
                    base_time = self.request_start_time or self.start_time
                    first_byte_time_ms = (
                        int((time.time() - base_time) * 1000) if base_time else None
                    )

                # 先返回原始块给客户端，确保 TTFB 不受数据库操作影响
                yield chunk

                # yield 后再更新数据库状态（仅第一个 chunk 时执行）
                if chunk_count == 1 and self.request_id:
                    try:
                        await asyncio.to_thread(
                            UsageService.update_usage_status,
                            db=self.db,
                            request_id=self.request_id,
                            status="streaming",
                            provider=self.provider,
                            first_byte_time_ms=first_byte_time_ms,
                            provider_id=self.provider_id,
                            provider_endpoint_id=self.provider_endpoint_id,
                            provider_api_key_id=self.provider_api_key_id,
                            api_format=self.api_format,
                            endpoint_api_format=self.endpoint_api_format,
                            has_format_conversion=self.has_format_conversion,
                            request_headers=self.request_headers,
                            request_body=self.request_data,
                            provider_request_headers=self.provider_request_headers,
                        )
                    except Exception as e:
                        logger.warning("更新使用记录状态为 streaming 失败: {}", e)

                # 解析块以提取内容和使用信息（chunk是原始字节）
                content, usage = self.parse_stream_chunk(chunk)

                if content:
                    self._accumulated_content_len += len(content)
                    # 实时估算输出tokens
                    self.output_tokens = max(1, self._accumulated_content_len // 4)

                if usage:
                    # 如果响应中包含准确的usage信息，使用它
                    self.input_tokens = usage.get("input_tokens", self.input_tokens)
                    self.output_tokens = usage.get("output_tokens", self.output_tokens)
                    self.cache_read_input_tokens = usage.get(
                        "cache_read_input_tokens", self.cache_read_input_tokens
                    )

                    # 统一提取 cache_creation tokens（新格式优先于旧格式）
                    total, t5m, t1h = extract_cache_creation_tokens_detail(usage)
                    if total:
                        self.cache_creation_input_tokens = total
                    if t5m or t1h:
                        self.cache_creation_input_tokens_5m = t5m
                        self.cache_creation_input_tokens_1h = t1h

        finally:
            # 流结束后记录使用量
            self.end_time = time.time()

            logger.debug(
                f"ID:{self.request_id} | 流式响应结束 | 共处理{chunk_count}个chunks | "
                f"累积内容长度:{self._accumulated_content_len} | 输出tokens:{self.output_tokens}"
            )

            # 检查是否收到了有效数据
            # 情况1: 收到了原始数据但无法解析为有效的SSE JSON
            if chunk_count > 0 and not self.response_chunks:
                error_msg = f"流式响应完成但未解析到任何有效数据（收到 {chunk_count} 个原始chunk但无法解析）"
                logger.error(f"ID:{self.request_id} | {error_msg}")
                # 设置错误状态，避免被记录为成功
                self.set_error_status(502, error_msg)
                # 抛出异常让 TaskService/FailoverEngine 捕获并触发故障转移
                raise EmptyStreamException(
                    provider_name=self.provider,
                    chunk_count=chunk_count,
                    request_metadata=None,
                )

            # 情况2: 流式响应完成但没有收到完整的消息（没有 message_stop 事件）
            # 这种情况通常发生在服务器重启或连接中断时
            if not self.stream_stats.has_completion and not self.response_chunks:
                error_msg = "流式响应中断：未收到任何有效数据（可能是连接中断或服务重启）"
                logger.warning(f"ID:{self.request_id} | {error_msg}")
                self.set_error_status(503, error_msg)

            # 确保日志一定会输出，即使记录usage失败
            try:
                await self._record_usage()
            except Exception as e:
                # 如果记录失败，至少输出基本的汇总日志
                logger.exception(
                    f"Failed to record stream usage for request {self.request_id}: {e}"
                )
                # 尝试输出基本的汇总日志，使用多层防护
                try:
                    # 计算响应时间，使用多层后备机制
                    try:
                        if self.request_start_time and self.end_time:
                            total_response_time = int(
                                (self.end_time - self.request_start_time) * 1000
                            )
                        elif self.start_time and self.end_time:
                            total_response_time = int((self.end_time - self.start_time) * 1000)
                        else:
                            total_response_time = 0
                    except Exception:
                        total_response_time = 0

                    # 安全地输出汇总日志
                    logger.info(
                        f"[请求完成] ID:{self.request_id or 'unknown'} | 200 | 耗时:{total_response_time}ms | "
                        f"Token:输入{self.input_tokens}/输出{self.output_tokens} | 费用:未知(记录失败)"
                    )
                except Exception as log_error:
                    # 最后的防线：输出最简单的完成标记
                    logger.error(f"Failed to output summary log: {log_error}")
                    try:
                        logger.info(
                            f"[请求完成] ID:{self.request_id or 'unknown'} | 记录失败但流已完成"
                        )
                    except Exception:
                        # 如果连最简单的日志都失败了，放弃
                        pass

    async def _record_usage(self) -> None:
        """记录最终的使用量"""
        try:
            if self.request_start_time and self.end_time:
                response_time_ms = int((self.end_time - self.request_start_time) * 1000)
            elif self.start_time and self.end_time:
                response_time_ms = int((self.end_time - self.start_time) * 1000)
            else:
                response_time_ms = None

            # 如果没有准确的token计数，使用估算值
            if self.output_tokens == 0 and self._accumulated_content_len > 0:
                self.output_tokens = max(1, self._accumulated_content_len // 4)

            # 使用完整的响应体（包含所有信息，包括工具调用）
            # 更新最终的usage信息
            self.complete_response["usage"].update(
                {
                    "input_tokens": self.input_tokens,
                    "output_tokens": self.output_tokens,
                    "cache_creation_input_tokens": self.cache_creation_input_tokens,
                    "cache_read_input_tokens": self.cache_read_input_tokens,
                }
            )

            # 记录响应数据
            # 如果成功解析了SSE chunks，使用解析后的结构化数据
            # 否则使用原始字节流（用于错误诊断，如403 HTML响应）
            if self.response_chunks:
                # 正常情况：成功解析的SSE JSON响应
                stored_chunks = len(self.response_chunks)
                total_chunks = self.response_chunks_count
                metadata = {
                    "stream": True,
                    "total_chunks": total_chunks,
                    "stored_chunks": stored_chunks,
                    "content_length": self._accumulated_content_len,
                    "response_time_ms": response_time_ms,
                }
                if stored_chunks < total_chunks:
                    metadata["truncated"] = True
                    metadata["dropped_chunks"] = total_chunks - stored_chunks
                response_body = {
                    "chunks": self.response_chunks,
                    "metadata": metadata,
                }
            else:
                # 错误情况：无法解析为JSON（如HTML错误页面）
                # 尝试解码原始字节流为文本
                raw_response_text = ""
                for chunk in self.raw_chunks:
                    try:
                        if isinstance(chunk, bytes):
                            raw_response_text += chunk.decode("utf-8", errors="replace")
                        else:
                            raw_response_text += str(chunk)
                    except Exception:
                        pass

                response_body = {
                    "chunks": [],
                    "raw_response": raw_response_text[:10000],  # 限制大小，避免过大
                    "metadata": {
                        "stream": True,
                        "total_chunks": 0,
                        "raw_chunks_count": len(self.raw_chunks),
                        "content_length": len(raw_response_text),
                        "response_time_ms": response_time_ms,
                        "parse_error": "Failed to parse response as SSE JSON format",
                    },
                }

            # 检查会话状态，如果会话处于不可用状态，需要回滚并创建新事务
            from sqlalchemy.exc import InvalidRequestError

            user = None
            api_key = None
            db_for_usage = self.db
            created_temp_session = False

            def _load_user_and_key(db_session: Session) -> tuple[User | None, ApiKey | None]:
                local_user = (
                    db_session.query(User).filter(User.id == self.user_id).first()
                    if self.user_id
                    else None
                )
                local_api_key = (
                    db_session.query(ApiKey).filter(ApiKey.id == self.api_key_id).first()
                    if self.api_key_id
                    else None
                )
                return local_user, local_api_key

            try:
                # 检查会话是否可用
                db_for_usage.info

                # 重新查询用户和API密钥对象，确保它们在会话中
                user, api_key = _load_user_and_key(db_for_usage)
            except InvalidRequestError:
                # 会话处于不可用状态，需要回滚并重新开始
                logger.warning(
                    f"Session in invalid state for request {self.request_id}, rolling back and retrying"
                )
                try:
                    db_for_usage.rollback()
                except Exception:
                    pass

                try:
                    db_for_usage.close()
                except Exception:
                    pass

                # 使用新的会话记录用量，避免 prepared 状态继续影响查询
                try:
                    db_for_usage = create_session()
                    created_temp_session = True
                    user, api_key = _load_user_and_key(db_for_usage)
                except Exception as session_error:
                    logger.exception(
                        f"Failed to recover from invalid session for request {self.request_id}: {session_error}"
                    )
                    return

            # 根据状态码确定请求状态
            final_status = "completed" if self.status_code == 200 else "failed"

            usage_record = await UsageService.record_usage_async(
                db=db_for_usage,
                user=user,
                api_key=api_key,
                provider=self.provider,
                model=self.model,
                input_tokens=self.input_tokens,
                output_tokens=self.output_tokens,
                cache_creation_input_tokens=self.cache_creation_input_tokens,
                cache_read_input_tokens=self.cache_read_input_tokens,
                cache_creation_input_tokens_5m=self.cache_creation_input_tokens_5m,
                cache_creation_input_tokens_1h=self.cache_creation_input_tokens_1h,
                request_type="chat",
                api_format=self.api_format,
                api_family=self.api_family,
                endpoint_kind=self.endpoint_kind,
                endpoint_api_format=self.endpoint_api_format,
                has_format_conversion=self.has_format_conversion,
                is_stream=True,
                response_time_ms=response_time_ms,
                status_code=self.status_code,  # 使用实际的状态码
                error_message=self.error_message,  # 使用实际的错误消息
                metadata={"stream": True, "content_length": self._accumulated_content_len},
                request_body=self.request_data,
                request_headers=self.request_headers,
                provider_request_headers=self.provider_request_headers,
                response_headers=self.response_headers,
                response_body=response_body,
                request_id=self.request_id,  # 传递 request_id
                # Provider 侧追踪信息（用于记录真实成本）
                provider_id=self.provider_id,
                provider_endpoint_id=self.provider_endpoint_id,
                provider_api_key_id=self.provider_api_key_id,
                # 请求状态
                status=final_status,
            )

            # 立即获取 total_cost_usd 的值，避免后续访问时对象已脱离会话
            total_cost = 0.0
            if usage_record:
                try:
                    # 在 usage_record 仍在会话中时，立即获取所需属性
                    total_cost = float(usage_record.total_cost_usd or 0)
                except Exception as e:
                    logger.warning(f"Failed to access total_cost_usd from usage_record: {e}")
                    total_cost = 0.0

            if db_for_usage and self.attempt_id:
                # RequestTrace 功能已移除，使用 RequestCandidate 表追踪
                # 状态更新已在 RequestCandidateService 中完成
                pass

            # 计算总响应时间（从请求开始到流结束）
            if self.request_start_time:
                total_response_time = int((self.end_time - self.request_start_time) * 1000)
            else:
                total_response_time = response_time_ms

            # 输出汇总日志（类似非流式请求的完成日志）
            # 根据状态码决定图标和日志级别
            status_prefix = "[请求完成]" if self.status_code == 200 else "[请求失败]"
            # 根据费用大小选择合适的格式
            if total_cost >= 0.01:
                cost_str = f"${total_cost:.4f}"
            elif total_cost > 0:
                cost_str = f"${total_cost:.6f}"
            else:
                cost_str = "$0"

            logger.info(
                f"{status_prefix} ID:{self.request_id} | {self.status_code} | 耗时:{total_response_time}ms | "
                f"Token:输入{self.input_tokens}/输出{self.output_tokens} | 费用:{cost_str}"
            )

            # 记录提供商结果用于动态权重调整
            # 健康监控/自适应调整已由 RequestExecutor/ErrorClassifier 处理
            # 这里不再需要手动记录
        except Exception as e:
            logger.exception(f"Failed to record stream usage: {e}")
        finally:
            if created_temp_session:
                try:
                    db_for_usage.close()
                except Exception:
                    pass


class EnhancedStreamUsageTracker(StreamUsageTracker):
    """
    增强的流式用量跟踪器
    支持更准确的token计算
    """

    def __init__(
        self,
        db: Session,
        user: User,
        api_key: ApiKey,
        provider: str,
        model: str,
        request_headers: dict[str, Any] | None = None,
        provider_request_headers: dict[str, Any] | None = None,
        request_id: str | None = None,
        start_time: float | None = None,
        attempt_id: str | None = None,
        # Provider 侧追踪信息（用于记录真实成本）
        provider_id: str | None = None,
        provider_endpoint_id: str | None = None,
        provider_api_key_id: str | None = None,
        # API 格式（用于选择正确的响应解析器）
        api_format: str | None = None,
        # 结构化格式维度（从 Adapter 层透传）
        api_family: str | None = None,
        endpoint_kind: str | None = None,
        # 格式转换信息
        endpoint_api_format: str | None = None,
        has_format_conversion: bool = False,
    ):
        super().__init__(
            db,
            user,
            api_key,
            provider,
            model,
            request_headers,
            provider_request_headers,
            request_id,
            start_time,
            attempt_id,
            provider_id,
            provider_endpoint_id,
            provider_api_key_id,
            api_format,
            api_family,
            endpoint_kind,
            endpoint_api_format,
            has_format_conversion,
        )
        # 用于更准确的token计算
        self._init_tokenizer()
        # 继承父类的SSE解析缓冲区
        # 这些已经在父类中初始化了

    def _init_tokenizer(self) -> None:
        """初始化分词器（如果可用）"""
        try:
            # 尝试导入tiktoken用于更准确的token计算
            import tiktoken

            # 根据模型选择合适的编码
            if "gpt-4" in self.model:
                self.tokenizer = tiktoken.get_encoding("cl100k_base")
            elif "gpt-3.5" in self.model:
                self.tokenizer = tiktoken.get_encoding("cl100k_base")
            else:
                # Claude或其他模型，使用近似方法
                self.tokenizer = None
        except ImportError:
            logger.debug("tiktoken not available, using estimation")
            self.tokenizer = None

    def count_tokens(self, text: str) -> int:
        """
        更准确地计算tokens

        Args:
            text: 文本内容

        Returns:
            token数量
        """
        if self.tokenizer:
            try:
                return len(self.tokenizer.encode(text))
            except Exception as e:
                logger.warning(f"Token encoding failed: {e}")

        # 回退到基于长度的估算
        return self.count_tokens_by_len(len(text))

    @staticmethod
    def count_tokens_by_len(text_len: int) -> int:
        """基于文本长度估算 token 数（避免持有完整文本）"""
        # 混合语言平均约 3 字符/token
        return max(1, text_len // 3)

    def estimate_input_tokens(self, messages: list) -> int:
        """
        更准确地估算输入tokens

        Args:
            messages: 消息列表

        Returns:
            估算的token数
        """
        total_text = ""
        for msg in messages:
            if isinstance(msg, dict):
                content = msg.get("content", "")
                if isinstance(content, str):
                    total_text += content + " "
                elif isinstance(content, list):
                    for block in content:
                        if hasattr(block, "text"):
                            total_text += block.text + " "

        return self.count_tokens(total_text)

    async def track_stream(
        self,
        stream: AsyncIterator[str],
        request_data: dict[str, Any],
        response_headers: dict[str, Any] | None = None,
    ) -> AsyncIterator[str]:
        """
        跟踪流式响应并更准确地计算用量

        Args:
            stream: 原始流式响应
            request_data: 请求数据
            response_headers: 实际的响应头

        Yields:
            流式响应块
        """
        import time

        self.start_time = time.time()
        self.request_data = request_data  # 保存请求数据

        # 保存响应头（如果没有提供，使用空字典而不是默认值）
        # 这样可以确保记录的是实际的响应头，而不是构造的默认值
        self.response_headers = response_headers if response_headers is not None else {}

        # 更准确地估算输入tokens
        messages = request_data.get("messages", [])
        self.input_tokens = self.estimate_input_tokens(messages)

        logger.debug(
            f"ID:{self.request_id} | 开始跟踪流式响应(Enhanced) | 估算输入tokens:{self.input_tokens}"
        )

        chunk_count = 0
        first_byte_time_ms = None  # 预先记录 TTFB，避免 yield 后计算不准确
        try:
            async for chunk in stream:
                chunk_count += 1
                # 保存原始字节流（用于错误诊断）
                self.raw_chunks.append(chunk)

                # 第一个 chunk 收到时，记录 TTFB 时间点（但先不更新数据库，避免阻塞）
                if chunk_count == 1:
                    # 计算 TTFB（使用请求原始开始时间或 track_stream 开始时间）
                    base_time = self.request_start_time or self.start_time
                    first_byte_time_ms = (
                        int((time.time() - base_time) * 1000) if base_time else None
                    )

                # 先返回原始块给客户端，确保 TTFB 不受数据库操作影响
                yield chunk

                # yield 后再更新数据库状态（仅第一个 chunk 时执行）
                if chunk_count == 1 and self.request_id:
                    try:
                        await asyncio.to_thread(
                            UsageService.update_usage_status,
                            db=self.db,
                            request_id=self.request_id,
                            status="streaming",
                            provider=self.provider,
                            first_byte_time_ms=first_byte_time_ms,
                            provider_id=self.provider_id,
                            provider_endpoint_id=self.provider_endpoint_id,
                            provider_api_key_id=self.provider_api_key_id,
                            api_format=self.api_format,
                            endpoint_api_format=self.endpoint_api_format,
                            has_format_conversion=self.has_format_conversion,
                            request_headers=self.request_headers,
                            request_body=self.request_data,
                            provider_request_headers=self.provider_request_headers,
                        )
                    except Exception as e:
                        logger.warning("更新使用记录状态为 streaming 失败: {}", e)

                # 解析块以提取内容和使用信息（chunk是原始字节）
                content, usage = self.parse_stream_chunk(chunk)

                if content:
                    self._accumulated_content_len += len(content)
                    # 使用更准确的方法计算输出tokens
                    self.output_tokens = self.count_tokens_by_len(self._accumulated_content_len)

                if usage:
                    # 如果响应中包含准确的usage信息，优先使用
                    if "input_tokens" in usage:
                        self.input_tokens = usage["input_tokens"]
                    if "output_tokens" in usage:
                        self.output_tokens = usage["output_tokens"]
                    if "cache_creation_input_tokens" in usage:
                        self.cache_creation_input_tokens = usage["cache_creation_input_tokens"]
                    if "cache_read_input_tokens" in usage:
                        self.cache_read_input_tokens = usage["cache_read_input_tokens"]

        finally:
            # 流结束后记录使用量
            self.end_time = time.time()

            logger.debug(
                f"ID:{self.request_id} | 流式响应结束 | 共处理{chunk_count}个chunks | "
                f"累积内容长度:{self._accumulated_content_len} | 输出tokens:{self.output_tokens}"
            )

            # 检查是否收到了有效数据
            # 情况1: 收到了原始数据但无法解析为有效的SSE JSON
            if chunk_count > 0 and not self.response_chunks:
                error_msg = f"流式响应完成但未解析到任何有效数据（收到 {chunk_count} 个原始chunk但无法解析）"
                logger.error(f"ID:{self.request_id} | {error_msg}")
                # 设置错误状态，避免被记录为成功
                self.set_error_status(502, error_msg)
                # 抛出异常让 TaskService/FailoverEngine 捕获并触发故障转移
                raise EmptyStreamException(
                    provider_name=self.provider,
                    chunk_count=chunk_count,
                    request_metadata=None,
                )

            # 情况2: 流式响应完成但没有收到完整的消息（没有 message_stop 事件）
            # 这种情况通常发生在服务器重启或连接中断时
            if not self.stream_stats.has_completion and not self.response_chunks:
                error_msg = "流式响应中断：未收到任何有效数据（可能是连接中断或服务重启）"
                logger.warning(f"ID:{self.request_id} | {error_msg}")
                self.set_error_status(503, error_msg)

            # 确保日志一定会输出，即使记录usage失败
            try:
                await self._record_usage()
            except Exception as e:
                # 如果记录失败，至少输出基本的汇总日志
                logger.exception(
                    f"Failed to record stream usage for request {self.request_id}: {e}"
                )
                # 尝试输出基本的汇总日志，使用多层防护
                try:
                    # 计算响应时间，使用多层后备机制
                    try:
                        if self.request_start_time and self.end_time:
                            total_response_time = int(
                                (self.end_time - self.request_start_time) * 1000
                            )
                        elif self.start_time and self.end_time:
                            total_response_time = int((self.end_time - self.start_time) * 1000)
                        else:
                            total_response_time = 0
                    except Exception:
                        total_response_time = 0

                    # 安全地输出汇总日志
                    logger.info(
                        f"[请求完成] ID:{self.request_id or 'unknown'} | 200 | 耗时:{total_response_time}ms | "
                        f"Token:输入{self.input_tokens}/输出{self.output_tokens} | 费用:未知(记录失败)"
                    )
                except Exception as log_error:
                    # 最后的防线：输出最简单的完成标记
                    logger.error(f"Failed to output summary log: {log_error}")
                    try:
                        logger.info(
                            f"[请求完成] ID:{self.request_id or 'unknown'} | 记录失败但流已完成"
                        )
                    except Exception:
                        # 如果连最简单的日志都失败了，放弃
                        pass


# 导出便捷函数
def create_stream_tracker(
    db: Session,
    user: User,
    api_key: ApiKey,
    provider: str,
    model: str,
    enhanced: bool = True,
    request_headers: dict[str, Any] | None = None,
    provider_request_headers: dict[str, Any] | None = None,
    request_id: str | None = None,
    start_time: float | None = None,
    attempt_id: str | None = None,
    # Provider 侧追踪信息（用于记录真实成本）
    provider_id: str | None = None,
    provider_endpoint_id: str | None = None,
    provider_api_key_id: str | None = None,
    # API 格式（用于选择正确的响应解析器）
    api_format: str | None = None,
    # 结构化格式维度（从 Adapter 层透传）
    api_family: str | None = None,
    endpoint_kind: str | None = None,
    # 格式转换信息
    endpoint_api_format: str | None = None,
    has_format_conversion: bool = False,
) -> StreamUsageTracker:
    """
    创建流式用量跟踪器

    Args:
        db: 数据库会话
        user: 用户对象
        api_key: API密钥对象
        provider: 提供商名称
        model: 模型名称
        enhanced: 是否使用增强版跟踪器
        request_headers: 实际的请求头
        provider_request_headers: 向提供商发送的请求头
        request_id: 请求ID（用于日志关联）
        start_time: 请求开始时间（用于计算总响应时间）
        attempt_id: RequestTrace 请求尝试ID（可选）
        provider_id: Provider ID（用于记录真实成本）
        provider_endpoint_id: Endpoint ID（用于记录真实成本）
        provider_api_key_id: API Key ID（用于记录真实成本）
        api_format: endpoint signature（如 "claude:chat", "openai:cli"）
        endpoint_api_format: 端点原生 API 格式
        has_format_conversion: 是否发生了格式转换

    Returns:
        流式用量跟踪器实例
    """
    if enhanced:
        return EnhancedStreamUsageTracker(
            db,
            user,
            api_key,
            provider,
            model,
            request_headers,
            provider_request_headers,
            request_id,
            start_time,
            attempt_id,
            provider_id,
            provider_endpoint_id,
            provider_api_key_id,
            api_format,
            api_family,
            endpoint_kind,
            endpoint_api_format,
            has_format_conversion,
        )
    else:
        return StreamUsageTracker(
            db,
            user,
            api_key,
            provider,
            model,
            request_headers,
            provider_request_headers,
            request_id,
            start_time,
            attempt_id,
            provider_id,
            provider_endpoint_id,
            provider_api_key_id,
            api_format,
            api_family,
            endpoint_kind,
            endpoint_api_format,
            has_format_conversion,
        )
