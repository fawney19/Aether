from __future__ import annotations

import os

# 测试运行在容器里时默认会被识别为 production，这里提供稳定的测试密钥。
os.environ.setdefault("JWT_SECRET_KEY", "test-jwt-secret-key-for-pytest-1234567890")
os.environ.setdefault("ENCRYPTION_KEY", "test-encryption-key-for-pytest-1234567890")
