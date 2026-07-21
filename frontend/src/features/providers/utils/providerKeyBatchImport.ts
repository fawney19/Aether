export interface ProviderKeyBatchImportItem {
  lineNumber: number
  name: string
  apiKey: string
}

export interface ProviderKeyBatchImportError {
  lineNumber: number | null
  message: string
}

export interface ProviderKeyBatchImportParseResult {
  items: ProviderKeyBatchImportItem[]
  errors: ProviderKeyBatchImportError[]
}

export const PROVIDER_KEY_BATCH_SEPARATOR = '----'

function splitNamedKey(line: string): { name: string; apiKey: string } | null {
  const separatorIndex = line.indexOf(PROVIDER_KEY_BATCH_SEPARATOR)
  if (separatorIndex < 0) return null
  return {
    name: line.slice(0, separatorIndex),
    apiKey: line.slice(separatorIndex + PROVIDER_KEY_BATCH_SEPARATOR.length),
  }
}

export function parseProviderKeyBatchImport(input: string): ProviderKeyBatchImportParseResult {
  const items: ProviderKeyBatchImportItem[] = []
  const errors: ProviderKeyBatchImportError[] = []
  const seenKeys = new Set<string>()
  const seenNames = new Set<string>()

  for (const [index, rawLine] of input.split(/\r?\n/).entries()) {
    const lineNumber = index + 1
    const line = rawLine.trim()
    if (!line || line.startsWith('#')) continue

    const named = splitNamedKey(line)
    if (!named) {
      errors.push({ lineNumber, message: `格式应为 名称${PROVIDER_KEY_BATCH_SEPARATOR}Key` })
      continue
    }
    const name = named.name.trim()
    const apiKey = named.apiKey.trim()
    if (!name) {
      errors.push({ lineNumber, message: '名称不能为空' })
      continue
    }
    if (!apiKey) {
      errors.push({ lineNumber, message: 'Key 不能为空' })
      continue
    }
    if (name.length > 100) {
      errors.push({ lineNumber, message: '名称不能超过 100 个字符' })
      continue
    }
    if (seenKeys.has(apiKey)) {
      errors.push({ lineNumber, message: 'Key 与前面行重复' })
      continue
    }
    if (seenNames.has(name)) {
      errors.push({ lineNumber, message: '名称与前面行重复' })
      continue
    }

    seenKeys.add(apiKey)
    seenNames.add(name)
    items.push({ lineNumber, name, apiKey })
  }

  return { items, errors }
}
