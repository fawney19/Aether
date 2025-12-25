"""
Headers 规则配置示例和工具函数

提供各种使用场景的示例和辅助函数
"""

from typing import Any, Dict, List, Optional, Union
from src.core.header_rules import HeaderRules, ReplaceValueRule, validate_header_rules


# ============================================================================
# 示例配置
# ============================================================================

EXAMPLE_BASIC_ADD = {
    "add": {
        "X-Custom-Header": "custom-value",
        "X-Request-ID": "12345"
    }
}

"""示例：新增固定的 headers"""

EXAMPLE_REMOVE = {
    "remove": ["User-Agent", "Referer"]
}

"""示例：删除指定的 headers"""

EXAMPLE_REPLACE_NAME = {
    "replace_name": {
        "X-Old-Name": "X-New-Name",
        "Authorization": "X-Custom-Authorization"
    }
}

"""示例：重命名 header"""

EXAMPLE_REPLACE_VALUE = {
    "replace_value": {
        "User-Agent": {
            "search": "MyApp/1.0",
            "replace": "MyApp/2.0",
            "regex": False,
            "case_sensitive": True
        }
    }
}

"""示例：替换 header 值（普通字符串）"""

EXAMPLE_REPLACE_VALUE_REGEX = {
    "replace_value": {
        "X-Custom-Header": {
            "search": r"v\d+\.\d+",
            "replace": "v2.0",
            "regex": True,
            "case_sensitive": False
        }
    }
}

"""示例：使用正则表达式替换 header 值"""

EXAMPLE_COMPLEX = {
    "add": {
        "X-API-Version": "v1",
        "X-Request-From": "my-platform"
    },
    "remove": ["Server", "X-Powered-By"],
    "replace_name": {
        "X-Old-API-Key": "X-API-Key"
    },
    "replace_value": {
        "User-Agent": {
            "search": "MyApp/1\\.0",
            "replace": "MyApp/2.0",
            "regex": True
        },
        "X-Custom-Header": {
            "search": "old-value",
            "replace": "new-value"
        }
    }
}

"""示例：复杂的组合规则"""


# ============================================================================
# 工具函数
# ============================================================================

def create_add_rule(headers: Dict[str, str]) -> Dict[str, Any]:
    """
    创建新增 headers 的规则

    Args:
        headers: 要新增的 headers 字典

    Returns:
        规则配置字典

    Example:
        >>> rule = create_add_rule({"X-Custom": "value"})
    """
    return {"add": headers}


def create_remove_rule(header_names: List[str]) -> Dict[str, Any]:
    """
    创建删除 headers 的规则

    Args:
        header_names: 要删除的 header 名称列表

    Returns:
        规则配置字典

    Example:
        >>> rule = create_remove_rule(["User-Agent", "Referer"])
    """
    return {"remove": header_names}


def create_replace_name_rule(mapping: Dict[str, str]) -> Dict[str, Any]:
    """
    创建重命名 headers 的规则

    Args:
        mapping: 旧名称到新名称的映射

    Returns:
        规则配置字典

    Example:
        >>> rule = create_replace_name_rule({"Old-Name": "New-Name"})
    """
    return {"replace_name": mapping}


def create_replace_value_rule(
    header_name: str,
    search: str,
    replace: str,
    *,
    regex: bool = False,
    case_sensitive: bool = True
) -> Dict[str, Any]:
    """
    创建替换 header 值的规则

    Args:
        header_name: header 名称
        search: 要搜索的值
        replace: 替换后的值
        regex: 是否使用正则表达式
        case_sensitive: 是否区分大小写

    Returns:
        规则配置字典

    Example:
        >>> rule = create_replace_value_rule("User-Agent", "v1.0", "v2.0")
    """
    return {
        "replace_value": {
            header_name: {
                "search": search,
                "replace": replace,
                "regex": regex,
                "case_sensitive": case_sensitive
            }
        }
    }


def combine_rules(*rules: Dict[str, Any]) -> Dict[str, Any]:
    """
    合并多个规则

    Args:
        *rules: 多个规则字典

    Returns:
        合并后的规则

    Example:
        >>> rule1 = create_add_rule({"X-1": "1"})
        >>> rule2 = create_remove_rule(["User-Agent"])
        >>> combined = combine_rules(rule1, rule2)
    """
    result: Dict[str, Any] = {}

    for rule in rules:
        for key, value in rule.items():
            if key == "add":
                if "add" not in result:
                    result["add"] = {}
                result["add"].update(value)
            elif key == "remove":
                if "remove" not in result:
                    result["remove"] = []
                result["remove"].extend(value)
                # 去重
                result["remove"] = list(set(result["remove"]))
            elif key == "replace_name":
                if "replace_name" not in result:
                    result["replace_name"] = {}
                result["replace_name"].update(value)
            elif key == "replace_value":
                if "replace_value" not in result:
                    result["replace_value"] = {}
                result["replace_value"].update(value)

    return result


def validate_and_format(
    rules: Dict[str, Any],
    *,
    pretty: bool = True
) -> tuple[bool, Optional[str], Optional[str]]:
    """
    验证规则并格式化为 JSON 字符串

    Args:
        rules: 规则字典
        pretty: 是否格式化输出

    Returns:
        (is_valid, json_string, error_message): 是否有效、JSON字符串、错误信息
    """
    is_valid, error_msg = validate_header_rules(rules)

    if not is_valid:
        return False, None, error_msg

    try:
        import json
        if pretty:
            json_str = json.dumps(rules, indent=2, ensure_ascii=False)
        else:
            json_str = json.dumps(rules, ensure_ascii=False)
        return True, json_str, None
    except Exception as e:
        return False, None, str(e)


def explain_rules(rules: Dict[str, Any]) -> str:
    """
    解释规则的作用（生成人类可读的描述）

    Args:
        rules: 规则字典

    Returns:
        规则描述文本
    """
    lines = ["Headers 规则配置："]

    if "add" in rules and rules["add"]:
        lines.append("\n1. 新增 headers：")
        for key, value in rules["add"].items():
            lines.append(f"   - {key}: {value}")

    if "remove" in rules and rules["remove"]:
        lines.append("\n2. 删除 headers：")
        for key in rules["remove"]:
            lines.append(f"   - {key}")

    if "replace_name" in rules and rules["replace_name"]:
        lines.append("\n3. 重命名 headers：")
        for old_name, new_name in rules["replace_name"].items():
            lines.append(f"   - {old_name} → {new_name}")

    if "replace_value" in rules and rules["replace_value"]:
        lines.append("\n4. 替换 header 值：")
        for header_name, rule in rules["replace_value"].items():
            if isinstance(rule, dict):
                search = rule.get("search", "")
                replace = rule.get("replace", "")
                regex = rule.get("regex", False)
                method = "正则表达式" if regex else "字符串"
                lines.append(f"   - {header_name}: {method}替换 '{search}' → '{replace}'")

    return "\n".join(lines)


# ============================================================================
# 预定义规则模板
# ============================================================================

def get_security_headers() -> Dict[str, Any]:
    """
    返回常见的安全 headers 模板

    包括：
    - X-Content-Type-Options: nosniff
    - X-Frame-Options: DENY
    - X-XSS-Protection: 1; mode=block
    """
    return create_add_rule({
        "X-Content-Type-Options": "nosniff",
        "X-Frame-Options": "DENY",
        "X-XSS-Protection": "1; mode=block"
    })


def get_remove_server_info_headers() -> Dict[str, Any]:
    """
    返回移除服务器信息的 headers 模板

    删除：
    - Server
    - X-Powered-By
    """
    return create_remove_rule(["Server", "X-Powered-By"])


def get_custom_user_agent_rule(user_agent: str) -> Dict[str, Any]:
    """
    返回自定义 User-Agent 的规则

    Args:
        user_agent: 新的 User-Agent 值

    Returns:
        替换 User-Agent 的规则
    """
    return create_replace_value_rule(
        "User-Agent",
        ".*",  # 匹配任何值
        user_agent,
        regex=True
    )
