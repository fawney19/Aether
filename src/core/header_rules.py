"""
Headers 规则处理器

支持在 endpoint.headers 字段中定义复杂的 headers 操作规则：
1. add: 新增固定的参数和值
2. remove: 删除指定的参数
3. replace_name: 替换参数名
4. replace_value: 替换参数值

数据格式：
{
  "add": {"X-New-Header": "value"},
  "remove": ["Old-Header"],
  "replace_name": {"Old-Name": "New-Name"},
  "replace_value": {"Header-Name": {"search": "old", "replace": "new", "regex": false}}
}
"""

import re
from typing import Any, Dict, List, Optional, Union
from pydantic import BaseModel, Field, field_validator


class ReplaceValueRule(BaseModel):
    """替换值的规则"""
    search: str = Field(..., description="要搜索的值")
    replace: str = Field(..., description="替换后的值")
    regex: bool = Field(default=False, description="是否使用正则表达式")
    case_sensitive: bool = Field(default=True, description="是否区分大小写")

    @field_validator("search")
    @classmethod
    def validate_search(cls, v: str, info) -> str:
        """验证搜索字符串"""
        regex = info.data.get("regex", False)
        if regex:
            try:
                re.compile(v)
            except re.error as e:
                raise ValueError(f"Invalid regex pattern: {e}")
        return v


class HeaderRules(BaseModel):
    """Headers 规则配置"""
    add: Optional[Dict[str, str]] = Field(default=None, description="新增的 headers")
    remove: Optional[List[str]] = Field(default=None, description="要删除的 header 名称")
    replace_name: Optional[Dict[str, str]] = Field(default=None, description="参数名替换映射")
    replace_value: Optional[Dict[str, Union[ReplaceValueRule, Dict[str, Any]]]] = Field(
        default=None, description="参数值替换规则"
    )

    def is_empty(self) -> bool:
        """检查是否没有任何规则"""
        return not any([
            self.add,
            self.remove,
            self.replace_name,
            self.replace_value
        ])


def apply_header_rules(
    headers: Dict[str, str],
    rules: Optional[Dict[str, Any]]
) -> Dict[str, str]:
    """
    应用 headers 规则到 headers 字典

    Args:
        headers: 原始 headers 字典
        rules: headers 规则配置（可以是字典或 HeaderRules 对象）

    Returns:
        处理后的 headers 字典
    """
    if not rules:
        return headers

    # 解析规则
    try:
        if isinstance(rules, dict):
            # 检查是否是旧格式（直接的 headers 字典）
            if not any(key in rules for key in ["add", "remove", "replace_name", "replace_value"]):
                # 旧格式：直接合并
                header_rules = HeaderRules(add=rules)
            else:
                # 新格式：包含规则
                header_rules = HeaderRules(**rules)
        else:
            header_rules = HeaderRules()
    except Exception as e:
        # 如果解析失败，返回原始 headers
        from src.core.logger import logger
        logger.warning(f"Failed to parse header rules: {e}, using original headers")
        return headers

    if header_rules.is_empty():
        return headers

    # 创建副本避免修改原始数据
    result = dict(headers)

    # 1. 删除指定的 headers
    if header_rules.remove:
        for key in header_rules.remove:
            # 大小写不敏感删除
            keys_to_remove = [k for k in result.keys() if k.lower() == key.lower()]
            for k in keys_to_remove:
                del result[k]

    # 2. 替换参数名
    if header_rules.replace_name:
        for old_name, new_name in header_rules.replace_name.items():
            # 大小写不敏感查找
            for key in list(result.keys()):
                if key.lower() == old_name.lower():
                    value = result.pop(key)
                    result[new_name] = value
                    break

    # 3. 替换参数值
    if header_rules.replace_value:
        for header_name, value_rule in header_rules.replace_value.items():
            # 大小写不敏感查找
            for key in result.keys():
                if key.lower() == header_name.lower():
                    # 解析规则
                    if isinstance(value_rule, dict):
                        rule = ReplaceValueRule(**value_rule)
                    else:
                        rule = value_rule

                    original_value = result[key]
                    new_value = _apply_replace_value(original_value, rule)
                    result[key] = new_value
                    break

    # 4. 新增 headers（不会覆盖已存在的）
    if header_rules.add:
        for key, value in header_rules.add.items():
            # 只有当 key 不存在时才添加
            if key not in result:
                result[key] = value

    return result


def _apply_replace_value(original: str, rule: ReplaceValueRule) -> str:
    """
    应用值替换规则

    Args:
        original: 原始值
        rule: 替换规则

    Returns:
        替换后的值
    """
    if rule.regex:
        # 正则表达式替换
        flags = 0 if rule.case_sensitive else re.IGNORECASE
        try:
            return re.sub(rule.search, rule.replace, original, flags=flags)
        except re.error as e:
            from src.core.logger import logger
            logger.warning(f"Regex replace failed: {e}, returning original value")
            return original
    else:
        # 普通字符串替换
        if rule.case_sensitive:
            return original.replace(rule.search, rule.replace)
        else:
            # 大小写不敏感替换
            pattern = re.compile(re.escape(rule.search), re.IGNORECASE)
            return pattern.sub(rule.replace, original)


def validate_header_rules(rules: Dict[str, Any]) -> tuple[bool, Optional[str]]:
    """
    验证 headers 规则配置

    Args:
        rules: 要验证的规则字典

    Returns:
        (is_valid, error_message): 是否有效和错误信息
    """
    try:
        # 尝试解析规则
        header_rules = HeaderRules(**rules)

        # 额外验证：检查正则表达式是否有效
        if header_rules.replace_value:
            for header_name, value_rule in header_rules.replace_value.items():
                if isinstance(value_rule, ReplaceValueRule):
                    if value_rule.regex:
                        try:
                            re.compile(value_rule.search)
                        except re.error as e:
                            return False, f"Invalid regex in '{header_name}': {e}"

        return True, None
    except Exception as e:
        return False, str(e)


def merge_legacy_headers(
    headers: Optional[Dict[str, str]],
    rules: Optional[Dict[str, Any]]
) -> Dict[str, Any]:
    """
    合并旧格式的 headers 和新格式的规则

    旧格式：{"X-Header": "value"}
    新格式：{"add": {"X-Header": "value"}, "remove": [...]}

    Args:
        headers: 旧格式的 headers
        rules: 新格式的规则

    Returns:
        合并后的规则
    """
    result = {}

    # 如果有规则，先合并规则
    if rules:
        result.update(rules)

    # 如果有旧格式的 headers，添加到 add 中
    if headers:
        if "add" not in result:
            result["add"] = {}
        result["add"].update(headers)

    return result
