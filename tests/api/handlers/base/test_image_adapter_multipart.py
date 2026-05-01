from __future__ import annotations

from src.services.provider.adapters.codex.image_transform import parse_multipart_image_edit


def _multipart_body(boundary: str) -> bytes:
    return b"".join(
        [
            f"--{boundary}\r\n".encode(),
            b'Content-Disposition: form-data; name="model"\r\n\r\n',
            b"gpt-image-1\r\n",
            f"--{boundary}\r\n".encode(),
            b'Content-Disposition: form-data; name="prompt"\r\n\r\n',
            b"make it blue\r\n",
            f"--{boundary}\r\n".encode(),
            b'Content-Disposition: form-data; name="image"; filename="input.png"\r\n',
            b"Content-Type: image/png\r\n\r\n",
            b"fake-png-bytes\r\n",
            f"--{boundary}--\r\n".encode(),
        ]
    )


def test_multipart_edits_boundary_is_case_sensitive() -> None:
    boundary = "----AetherBoundaryXyZ"
    parsed = parse_multipart_image_edit(
        _multipart_body(boundary),
        f"multipart/form-data; boundary={boundary}",
    )

    assert parsed is not None
    assert parsed["model"] == "gpt-image-1"
    assert parsed["prompt"] == "make it blue"
    assert len(parsed["images"]) == 1


def test_multipart_edits_lowercased_boundary_does_not_match_body() -> None:
    boundary = "----AetherBoundaryXyZ"
    parsed = parse_multipart_image_edit(
        _multipart_body(boundary),
        f"multipart/form-data; boundary={boundary}".lower(),
    )

    assert parsed is None
