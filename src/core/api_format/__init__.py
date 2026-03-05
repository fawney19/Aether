"""
API 格式核心模块（新模式）。

系统内部统一使用 endpoint signature key 作为“格式”标识：
    `<api_family>:<endpoint_kind>`（全小写，例如 "openai:chat"）。
"""

from src.core.api_format.auth import (
    ApiKeyAuthHandler,
    AuthHandler,
    BearerAuthHandler,
    GoogApiKeyAuthHandler,
    OAuth2AuthHandler,
    QueryKeyAuthHandler,
    get_auth_handler,
    get_default_auth_method_for_endpoint,
)
from src.core.api_format.detection import (
    RequestContext,
    detect_cli_format_from_path,
    detect_format_and_key_from_starlette,
    detect_format_from_request,
    detect_format_from_response,
    detect_request_context,
)
from src.core.api_format.enums import ApiFamily, AuthMethod, EndpointKind, EndpointType
from src.core.api_format.headers import (
    CORE_REDACT_HEADERS,
    HOP_BY_HOP_HEADERS,
    RESPONSE_DROP_HEADERS,
    UPSTREAM_DROP_HEADERS,
    HeaderBuilder,
    build_adapter_base_headers_for_endpoint,
    build_adapter_headers_for_endpoint,
    build_anthropic_extra_headers,
    build_browser_fingerprint_headers,
    build_upstream_headers_for_endpoint,
    detect_capabilities_for_endpoint,
    extract_client_api_key_for_endpoint,
    extract_client_api_key_for_endpoint_with_query,
    extract_set_headers_from_rules,
    filter_response_headers,
    get_adapter_protected_keys_for_endpoint,
    get_extra_headers_from_endpoint,
    get_header_value,
    merge_headers_with_protection,
    normalize_headers,
    redact_headers_for_log,
)
from src.core.api_format.metadata import (
    ENDPOINT_DEFINITIONS,
    EndpointDefinition,
    can_passthrough_endpoint,
    get_auth_config_for_endpoint,
    get_data_format_id_for_endpoint,
    get_default_body_rules_for_endpoint,
    get_default_path_for_endpoint,
    get_endpoint_definition,
    get_extra_headers_for_endpoint,
    get_local_path_for_endpoint,
    get_protected_keys_for_endpoint,
    list_endpoint_definitions,
    make_endpoint_signature,
    resolve_endpoint_definition,
)
from src.core.api_format.signature import (
    EndpointSignature,
    make_signature_key,
    normalize_signature_key,
    parse_signature_key,
)
from src.core.api_format.utils import (
    get_base_format,
    is_cli_format,
    is_convertible_format,
    is_same_format,
    normalize_format,
)

__all__ = [
    # Enums
    "ApiFamily",
    "EndpointKind",
    "AuthMethod",
    "EndpointType",
    # Signature
    "EndpointSignature",
    "make_signature_key",
    "parse_signature_key",
    "normalize_signature_key",
    # Metadata
    "EndpointDefinition",
    "ENDPOINT_DEFINITIONS",
    "list_endpoint_definitions",
    "get_endpoint_definition",
    "resolve_endpoint_definition",
    "make_endpoint_signature",
    "get_default_path_for_endpoint",
    "get_local_path_for_endpoint",
    "get_auth_config_for_endpoint",
    "get_extra_headers_for_endpoint",
    "get_protected_keys_for_endpoint",
    "get_data_format_id_for_endpoint",
    "get_default_body_rules_for_endpoint",
    "can_passthrough_endpoint",
    # Utils
    "is_cli_format",
    "get_base_format",
    "normalize_format",
    "is_same_format",
    "is_convertible_format",
    # Headers
    "UPSTREAM_DROP_HEADERS",
    "CORE_REDACT_HEADERS",
    "HOP_BY_HOP_HEADERS",
    "RESPONSE_DROP_HEADERS",
    "normalize_headers",
    "get_header_value",
    "extract_client_api_key_for_endpoint",
    "extract_client_api_key_for_endpoint_with_query",
    "detect_capabilities_for_endpoint",
    "HeaderBuilder",
    "build_upstream_headers_for_endpoint",
    "build_browser_fingerprint_headers",
    "build_anthropic_extra_headers",
    "merge_headers_with_protection",
    "filter_response_headers",
    "redact_headers_for_log",
    "build_adapter_base_headers_for_endpoint",
    "build_adapter_headers_for_endpoint",
    "get_adapter_protected_keys_for_endpoint",
    "extract_set_headers_from_rules",
    "get_extra_headers_from_endpoint",
    # Detection
    "detect_format_from_request",
    "detect_format_and_key_from_starlette",
    "detect_format_from_response",
    "detect_cli_format_from_path",
    "detect_request_context",
    "RequestContext",
    # Auth
    "AuthHandler",
    "BearerAuthHandler",
    "ApiKeyAuthHandler",
    "GoogApiKeyAuthHandler",
    "OAuth2AuthHandler",
    "QueryKeyAuthHandler",
    "get_auth_handler",
    "get_default_auth_method_for_endpoint",
]
