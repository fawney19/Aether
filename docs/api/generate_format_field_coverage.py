#!/usr/bin/env python3
"""Generate the provider schema field coverage matrix.

The input inventory is docs/api/provider-interface-definitions.md. Existing
coverage rows are reused so audited status/notes survive regeneration. Newly
introduced provider fields get conservative same-format/native and cross-format
fail-closed defaults until a human audits whether they deserve an explicit
mapping.
"""

from __future__ import annotations

import argparse
import dataclasses
from collections import Counter, defaultdict
from pathlib import Path
from typing import Iterable


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_DEFINITIONS = ROOT / "docs/api/provider-interface-definitions.md"
DEFAULT_MATRIX = ROOT / "docs/api/format-field-coverage-matrix.md"


@dataclasses.dataclass(frozen=True)
class SourceField:
    provider: str
    schema: str
    field: str
    required: str
    field_type: str


@dataclasses.dataclass(frozen=True)
class CoverageStatus:
    surface: str
    same_format_runtime: str
    canonical_roundtrip: str
    cross_format: str
    notes: str


OPENAI_CHAT_MAPPED = {
    "model",
    "messages",
    "max_tokens",
    "max_completion_tokens",
    "temperature",
    "top_p",
    "top_logprobs",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "metadata",
    "response_format",
    "reasoning_effort",
    "verbosity",
    "store",
    "service_tier",
    "safety_identifier",
    "prompt_cache_key",
    "prompt_cache_retention",
    "stream",
}

OPENAI_CHAT_BLOCKED = {
    "n",
    "stop",
    "presence_penalty",
    "frequency_penalty",
    "seed",
    "logprobs",
    "stream_options",
    "user",
    "function_call",
    "functions",
    "logit_bias",
    "modalities",
    "prediction",
    "audio",
    "web_search_options",
}

OPENAI_RESPONSES_MAPPED = {
    "model",
    "input",
    "instructions",
    "max_output_tokens",
    "temperature",
    "top_p",
    "top_logprobs",
    "metadata",
    "parallel_tool_calls",
    "text",
    "tools",
    "tool_choice",
    "reasoning",
    "store",
    "service_tier",
    "safety_identifier",
    "prompt_cache_key",
    "prompt_cache_retention",
}

OPENAI_RESPONSES_BLOCKED = {
    "include",
    "previous_response_id",
    "truncation",
    "prompt",
    "conversation",
    "background",
    "max_tool_calls",
    "user",
    "context_management",
    "stream",
    "stream_options",
}

CLAUDE_MAPPED_FIELDS = {
    "id",
    "type",
    "role",
    "text",
    "content",
    "source",
    "name",
    "description",
    "input",
    "input_schema",
    "messages",
    "model",
    "max_tokens",
    "system",
    "temperature",
    "top_p",
    "top_k",
    "stop_sequences",
    "tool_choice",
    "tools",
    "metadata",
    "thinking",
    "output_config",
    "usage",
    "stop_reason",
    "stop_sequence",
}

CLAUDE_PROVIDER_ONLY_FIELDS = {
    "cache_control",
    "container",
    "inference_geo",
    "service_tier",
    "allowed_callers",
    "allowed_domains",
    "blocked_domains",
    "defer_loading",
    "max_uses",
    "strict",
    "user_location",
    "citations",
    "context",
    "title",
    "file_id",
    "document_index",
    "document_title",
    "cited_text",
    "caller",
}


def split_markdown_row(line: str) -> list[str]:
    cells: list[str] = []
    current: list[str] = []
    escaped = False
    for char in line:
        if char == "|" and not escaped:
            cells.append("".join(current).strip())
            current.clear()
        else:
            current.append(char)
        escaped = char == "\\" and not escaped
        if escaped and char != "\\":
            escaped = False
    cells.append("".join(current).strip())
    return cells


def strip_markdown_code(value: str) -> str:
    value = value.strip()
    if value.startswith("`") and value.endswith("`"):
        value = value[1:-1]
    return value.replace("\\|", "|")


def escape_markdown_cell(value: str) -> str:
    return value.replace("|", "\\|")


def parse_schema_heading(line: str) -> str | None:
    if not line.startswith("### `"):
        return None
    rest = line[len("### `") :]
    schema, _, _ = rest.partition("`")
    return schema or None


def parse_provider_definition_fields(definitions: str) -> list[SourceField]:
    provider: str | None = None
    schema: str | None = None
    fields: list[SourceField] = []

    for line in definitions.splitlines():
        if line.startswith("## "):
            if "OpenAI Schema" in line:
                provider = "OpenAI"
            elif "Claude / Anthropic TypeScript" in line:
                provider = "Claude"
            elif "Gemini Schema" in line:
                provider = "Gemini"
            else:
                provider = None
            schema = None
            continue

        if provider is None:
            continue

        if heading := parse_schema_heading(line):
            schema = heading
            continue

        if schema is None or not line.startswith("| `"):
            continue

        cells = split_markdown_row(line)
        if len(cells) < 4 or cells[2] not in {"是", "否"}:
            continue
        fields.append(
            SourceField(
                provider=provider,
                schema=schema,
                field=strip_markdown_code(cells[1]),
                required=cells[2],
                field_type=strip_markdown_code(cells[3]),
            )
        )

    return fields


def parse_existing_coverage(
    matrix: str,
) -> tuple[dict[tuple[str, str, str], CoverageStatus], dict[tuple[str, str], list[CoverageStatus]]]:
    existing: dict[tuple[str, str, str], CoverageStatus] = {}
    profiles: dict[tuple[str, str], list[CoverageStatus]] = defaultdict(list)

    for line in matrix.splitlines():
        if not line.startswith("| "):
            continue
        cells = split_markdown_row(line)
        if len(cells) < 11 or cells[1] not in {"OpenAI", "Claude", "Gemini"}:
            continue
        status = CoverageStatus(
            surface=cells[6],
            same_format_runtime=cells[7],
            canonical_roundtrip=cells[8],
            cross_format=cells[9],
            notes=cells[10],
        )
        provider = cells[1]
        schema = strip_markdown_code(cells[2])
        field = strip_markdown_code(cells[3])
        existing[(provider, schema, field)] = status
        profiles[(provider, schema)].append(status)

    return existing, profiles


def most_common(values: Iterable[str]) -> str | None:
    values = list(values)
    if not values:
        return None
    return Counter(values).most_common(1)[0][0]


def openai_surface(schema: str) -> str:
    if "CreateChatCompletion" in schema or "ChatCompletion" in schema:
        return "openai:chat standard"
    if "CreateEmbedding" in schema or "Embedding" in schema:
        return "openai:embedding"
    if any(token in schema for token in ("CreateImage", "EditImage", "Image", "Images")):
        return "openai:image native-only"
    if any(token in schema for token in ("Compact", "Compaction")):
        return "openai:responses:compact native-only"
    if any(
        token in schema
        for token in (
            "Response",
            "Input",
            "Output",
            "Tool",
            "Reasoning",
            "WebSearch",
            "FileSearch",
            "Computer",
            "MCP",
            "CodeInterpreter",
            "Function",
            "Custom",
            "EasyInput",
            "Prompt",
            "Conversation",
            "Annotation",
            "Citation",
            "LogProb",
            "TopLogProb",
            "Metadata",
            "ServiceTier",
            "Verbosity",
            "TextResponse",
            "ResponseFormat",
            "Include",
            "Modalities",
            "ParallelToolCalls",
            "StopConfiguration",
        )
    ):
        return "openai:responses standard"
    return "openai auxiliary / not-in-conversion-surface"


def openai_default_status(field: SourceField, profile: list[CoverageStatus]) -> CoverageStatus:
    surface = most_common(status.surface for status in profile) or openai_surface(field.schema)
    if "not-in-conversion-surface" in surface or "native-only" in surface:
        return CoverageStatus(
            surface=surface,
            same_format_runtime="native",
            canonical_roundtrip="not-in-conversion-surface",
            cross_format="not-in-conversion-surface",
            notes="not part of current canonical cross-format conversion; same-format runtime path remains provider-native when routed directly",
        )
    if field.schema == "CreateChatCompletionRequest":
        if field.field in OPENAI_CHAT_MAPPED:
            return CoverageStatus(
                surface=surface,
                same_format_runtime="native",
                canonical_roundtrip="mapped",
                cross_format="mapped",
                notes="Chat request field maps provider-specifically; target-incompatible cases fail closed",
            )
        if field.field in OPENAI_CHAT_BLOCKED:
            return CoverageStatus(
                surface=surface,
                same_format_runtime="native",
                canonical_roundtrip="extension-preserved",
                cross_format="lossy-blocked",
                notes="Chat-only or provider-specific field has no audited lossless target equivalent",
            )
    if field.schema == "CreateResponse":
        if field.field in OPENAI_RESPONSES_MAPPED:
            return CoverageStatus(
                surface=surface,
                same_format_runtime="native",
                canonical_roundtrip="mapped",
                cross_format="mapped",
                notes="Responses request field maps provider-specifically; target-incompatible cases fail closed",
            )
        if field.field in OPENAI_RESPONSES_BLOCKED:
            return CoverageStatus(
                surface=surface,
                same_format_runtime="native",
                canonical_roundtrip="extension-preserved",
                cross_format="lossy-blocked",
                notes="Responses-only field has no audited lossless Chat/Claude/Gemini target equivalent",
            )
    if profile:
        cross_format = most_common(status.cross_format for status in profile) or "lossy-blocked"
        return CoverageStatus(
            surface=surface,
            same_format_runtime="native",
            canonical_roundtrip=most_common(status.canonical_roundtrip for status in profile)
            or "extension-preserved",
            cross_format=cross_format,
            notes=next(
                (status.notes for status in profile if status.cross_format == cross_format),
                "schema-level handling inherited from audited sibling fields",
            ),
        )
    return CoverageStatus(
        surface=surface,
        same_format_runtime="native",
        canonical_roundtrip="extension-preserved",
        cross_format="lossy-blocked",
        notes="OpenAI documented field is preserved same-format; cross-format requires explicit target mapping or fails closed",
    )


def claude_default_status(field: SourceField) -> CoverageStatus:
    if "CountTokens" in field.schema:
        return CoverageStatus(
            surface="claude:messages/count_tokens native-only",
            same_format_runtime="native",
            canonical_roundtrip="not-in-conversion-surface",
            cross_format="not-in-conversion-surface",
            notes="count_tokens schemas are provider-native and outside canonical generation conversion",
        )
    if field.field in CLAUDE_MAPPED_FIELDS:
        return CoverageStatus(
            surface="claude:messages standard",
            same_format_runtime="native",
            canonical_roundtrip="mapped",
            cross_format="mapped/lossy-blocked",
            notes="Claude field maps where canonical and target support an equivalent; otherwise conversion fails closed",
        )
    if (
        field.field in CLAUDE_PROVIDER_ONLY_FIELDS
        or field.field.endswith("_tokens_details")
        or "cache" in field.field
    ):
        return CoverageStatus(
            surface="claude:messages standard",
            same_format_runtime="native",
            canonical_roundtrip="extension-preserved",
            cross_format="lossy-blocked",
            notes="Claude provider-specific field is preserved same-format and blocked cross-format without an audited target equivalent",
        )
    return CoverageStatus(
        surface="claude:messages standard",
        same_format_runtime="native",
        canonical_roundtrip="extension-preserved",
        cross_format="lossy-blocked",
        notes="Claude nested/provider-specific field is same-format preserved; cross-format requires explicit mapping or fails closed",
    )


def gemini_default_status(field: SourceField, profile: list[CoverageStatus]) -> CoverageStatus:
    if profile:
        cross_format = most_common(status.cross_format for status in profile) or "lossy-blocked"
        return CoverageStatus(
            surface=most_common(status.surface for status in profile)
            or "gemini:generate_content standard",
            same_format_runtime="native",
            canonical_roundtrip=most_common(status.canonical_roundtrip for status in profile)
            or "extension-preserved",
            cross_format=cross_format,
            notes=next(
                (status.notes for status in profile if status.cross_format == cross_format),
                "Gemini field follows schema-level handling",
            ),
        )
    return CoverageStatus(
        surface="gemini:generate_content standard",
        same_format_runtime="native",
        canonical_roundtrip="extension-preserved",
        cross_format="lossy-blocked",
        notes="Gemini documented field is preserved same-format; cross-format requires explicit mapping or fails closed",
    )


def default_status(field: SourceField, profile: list[CoverageStatus]) -> CoverageStatus:
    if field.provider == "OpenAI":
        return openai_default_status(field, profile)
    if field.provider == "Claude":
        return claude_default_status(field)
    if field.provider == "Gemini":
        return gemini_default_status(field, profile)
    raise ValueError(f"unsupported provider: {field.provider}")


def render_matrix(
    fields: list[SourceField],
    existing: dict[tuple[str, str, str], CoverageStatus],
    profiles: dict[tuple[str, str], list[CoverageStatus]],
) -> str:
    rows: list[str] = [
        "# Format Field Coverage Matrix",
        "",
        "Last generated: 2026-06-03",
        "",
        "This file is generated from the schema inventory in `docs/api/provider-interface-definitions.md` and gives every documented schema field an explicit handling status. “处理到” here means the field is either mapped, preserved in same-format paths, rejected with a structured fail-closed error, or explicitly outside the current conversion surface. It does not mean every field can be cross-format converted.",
        "",
        "Provider schema updates do not require immediate conversion-code changes for runtime safety. Same-format runtime paths bypass canonical conversion, and same-format canonical roundtrip preserves provider extension fields. Cross-format conversion only enables fields with an audited semantic mapping; newly discovered or unknown provider fields default to structured fail-closed behavior until mapped.",
        "",
        "Regenerate with: `python3 docs/api/generate_format_field_coverage.py`.",
        "",
        "Statuses used in this matrix: `native`, `mapped`, `mapped/lossy-blocked`, `extension-preserved`, `unaudited`, `unsupported`, `invalid-enum`, `lossy-blocked`, `not-in-conversion-surface`.",
        "",
        "| Provider | Schema | Field | Required | Type | Surface | Same-Format Runtime | Canonical Roundtrip | Cross-Format | Notes |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]

    for field in fields:
        status = existing.get(
            (field.provider, field.schema, field.field),
            default_status(field, profiles[(field.provider, field.schema)]),
        )
        rows.append(
            "| "
            + " | ".join(
                [
                    field.provider,
                    f"`{escape_markdown_cell(field.schema)}`",
                    f"`{escape_markdown_cell(field.field)}`",
                    field.required,
                    f"`{escape_markdown_cell(field.field_type)}`",
                    escape_markdown_cell(status.surface),
                    escape_markdown_cell(status.same_format_runtime),
                    escape_markdown_cell(status.canonical_roundtrip),
                    escape_markdown_cell(status.cross_format),
                    escape_markdown_cell(status.notes),
                ]
            )
            + " |"
        )

    rows.extend(["", f"Total covered schema fields: {len(fields)}."])
    return "\n".join(rows) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--definitions", type=Path, default=DEFAULT_DEFINITIONS)
    parser.add_argument("--matrix", type=Path, default=DEFAULT_MATRIX)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()

    definitions = args.definitions.read_text()
    current_matrix = args.matrix.read_text() if args.matrix.exists() else ""
    fields = parse_provider_definition_fields(definitions)
    existing, profiles = parse_existing_coverage(current_matrix)
    next_matrix = render_matrix(fields, existing, profiles)

    if args.check:
        if current_matrix != next_matrix:
            print(
                f"{args.matrix} is not up to date; run "
                "`python3 docs/api/generate_format_field_coverage.py`",
            )
            return 1
        return 0

    args.matrix.write_text(next_matrix)
    print(f"wrote {len(fields)} field coverage rows to {args.matrix}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
