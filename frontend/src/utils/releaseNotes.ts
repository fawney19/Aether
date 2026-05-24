const AUTO_GENERATED_RELEASE_SECTION_PATTERNS = [
  /^#{1,6}\s+what'?s changed\s*$/i,
  /^#{1,6}\s+new contributors\s*$/i,
  /^\*\*full changelog\*\*:/i,
]

const MARKDOWN_STRUCTURE_PATTERNS = [
  /^#{1,6}\s+\S/,
  /^\s*[-*+]\s+\S/,
  /^\s*\d+\.\s+\S/,
  /^\s*>\s+\S/,
  /^\s*```/,
  /^\s*---+\s*$/,
  /^\s*\|(?:[^|]+\|)+\s*$/,
  /`[^`]+`/,
  /\[[^\]]+\]\([^)]+\)/,
]

const SENTENCE_END_PUNCTUATION = /[，。,.!?！？；;]$/
const SECTION_HEADING_SUFFIX = /[:：]\s*$/
const URL_PATTERN = /https?:\/\/|www\./i
const BRACKET_PREFIX_PATTERN = /^[\[(（【<]/
const SECTION_HEADING_TEXT_PATTERN = /[\u3400-\u9FFFA-Za-z]/

function isStructuredMarkdownLine(line: string): boolean {
  return MARKDOWN_STRUCTURE_PATTERNS.some((pattern) => pattern.test(line))
}

function normalizeSectionHeading(line: string): string {
  return line.replace(SECTION_HEADING_SUFFIX, '').trim()
}

function looksLikeSectionHeading(line: string): boolean {
  const trimmed = normalizeSectionHeading(line)

  if (!trimmed) return false
  if (trimmed.length > 24) return false
  if (URL_PATTERN.test(trimmed)) return false
  if (BRACKET_PREFIX_PATTERN.test(trimmed)) return false
  if (isStructuredMarkdownLine(trimmed)) return false
  if (SENTENCE_END_PUNCTUATION.test(trimmed)) return false
  if (!SECTION_HEADING_TEXT_PATTERN.test(trimmed)) return false

  return true
}

function collectConfirmedHeadingIndexes(lines: string[]): Set<number> {
  const candidates = lines
    .map((line, index) => {
      if (!looksLikeSectionHeading(line.trim())) return -1
      if (index === 0) return index
      return lines[index - 1].trim() === '' ? index : -1
    })
    .filter((index) => index !== -1)

  const confirmed = new Set<number>()

  for (let i = 0; i < candidates.length; i += 1) {
    const current = candidates[i]
    const next = i + 1 < candidates.length ? candidates[i + 1] : lines.length

    for (let cursor = current + 1; cursor < next; cursor += 1) {
      const content = lines[cursor].trim()
      if (!content) continue
      confirmed.add(current)
      break
    }
  }

  return confirmed
}

function collapseBlankLines(lines: string[]): string {
  return lines
    .join('\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim()
}

export function trimReleaseNotesForDisplay(notes: string | null | undefined): string {
  if (!notes) return ''

  const normalized = notes.replace(/\r\n?/g, '\n').trim()
  if (!normalized) return ''

  const lines = normalized.split('\n')
  const cutoff = lines.findIndex((line) => {
    const trimmed = line.trim()
    return AUTO_GENERATED_RELEASE_SECTION_PATTERNS.some((pattern) => pattern.test(trimmed))
  })

  if (cutoff === -1) {
    return normalized
  }

  return lines.slice(0, cutoff).join('\n').trim()
}

export function normalizeReleaseNotesForDisplay(notes: string | null | undefined): string {
  const trimmed = trimReleaseNotesForDisplay(notes)
  if (!trimmed) return ''

  const lines = trimmed.split('\n')
  if (lines.some((line) => isStructuredMarkdownLine(line.trim()))) {
    return trimmed
  }

  const headingIndexes = collectConfirmedHeadingIndexes(lines)
  if (headingIndexes.size < 2) {
    return trimmed
  }

  const normalized: string[] = []
  let insideSection = false

  for (let index = 0; index < lines.length; index += 1) {
    const trimmedLine = lines[index].trim()

    if (!trimmedLine) {
      if (normalized.length > 0 && normalized[normalized.length - 1] !== '') {
        normalized.push('')
      }
      continue
    }

    if (headingIndexes.has(index)) {
      if (normalized.length > 0 && normalized[normalized.length - 1] !== '') {
        normalized.push('')
      }
      normalized.push(`### ${normalizeSectionHeading(trimmedLine)}`)
      insideSection = true
      continue
    }

    if (insideSection) {
      normalized.push(`- ${trimmedLine.replace(/^[-*+•]\s*/, '')}`)
      continue
    }

    normalized.push(trimmedLine)
  }

  return collapseBlankLines(normalized)
}
