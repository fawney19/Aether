"""
数据压缩/解压工具

提供JSON数据的gzip压缩和解压功能
"""

import gzip
from typing import Any

from src.utils import json_helper as json


def compress_json(data: Any) -> bytes | None:
    """
    将JSON数据压缩为gzip格式的字节

    Args:
        data: 任意可JSON序列化的数据

    Returns:
        gzip压缩后的字节，如果输入为None则返回None
    """
    if data is None:
        return None

    try:
        # 转换为JSON字符串
        json_str = json.dumps(data, ensure_ascii=False)
        # gzip压缩
        compressed = gzip.compress(json_str.encode("utf-8"), compresslevel=6)
        return compressed
    except Exception:
        # 如果压缩失败，返回None
        return None


def decompress_json(compressed_data: bytes | None) -> Any | None:
    """
    解压gzip格式的字节为JSON数据

    Args:
        compressed_data: gzip压缩的字节数据

    Returns:
        解压后的JSON数据，如果输入为None或解压失败则返回None
    """
    if compressed_data is None:
        return None

    try:
        # gzip解压
        json_str = gzip.decompress(compressed_data).decode("utf-8")
        # 解析JSON
        data = json.loads(json_str)
        return data
    except Exception:
        # 如果解压失败，返回None
        return None


def get_body_size(data: Any) -> int:
    """
    获取JSON数据序列化后的字节大小

    Args:
        data: 任意可JSON序列化的数据

    Returns:
        字节大小
    """
    if data is None:
        return 0
    try:
        return len(json.dumps(data, ensure_ascii=False).encode("utf-8"))
    except Exception:
        return 0
