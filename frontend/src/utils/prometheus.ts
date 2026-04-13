export interface PrometheusSample {
  name: string
  labels: Record<string, string>
  value: string
}

export function parsePrometheusSamples(text: string): PrometheusSample[] {
  return text
    .split(/\r?\n/)
    .map(line => line.trim())
    .filter(line => line.length > 0 && !line.startsWith('#'))
    .map(parsePrometheusLine)
    .filter((sample): sample is PrometheusSample => sample !== null)
}

export function findMetricValueNumber(
  samples: PrometheusSample[],
  metricName: string,
  labels: Record<string, string> = {}
): number | null {
  const sample = samples.find(item =>
    metricNameMatches(item.name, metricName) && labelsMatch(item.labels, labels)
  )

  if (!sample) {
    return null
  }

  const value = Number(sample.value)
  return Number.isFinite(value) ? value : null
}

export function sumMetricValues(
  samples: PrometheusSample[],
  metricName: string
): number {
  return samples.reduce((total, sample) => {
    if (!metricNameMatches(sample.name, metricName)) {
      return total
    }

    const value = Number(sample.value)
    return Number.isFinite(value) ? total + value : total
  }, 0)
}

function metricNameMatches(actual: string, expected: string): boolean {
  return actual === expected || actual.split('_').pop() === expected || actual.endsWith(`_${expected}`)
}

function labelsMatch(
  actual: Record<string, string>,
  expected: Record<string, string>
): boolean {
  return Object.entries(expected).every(([key, value]) => actual[key] === value)
}

function parsePrometheusLine(line: string): PrometheusSample | null {
  const separatorIndex = line.lastIndexOf(' ')
  if (separatorIndex === -1) {
    return null
  }

  const metric = line.slice(0, separatorIndex).trim()
  const value = line.slice(separatorIndex + 1).trim()
  if (!metric || !value) {
    return null
  }

  const labelStart = metric.indexOf('{')
  if (labelStart === -1 || !metric.endsWith('}')) {
    return {
      name: metric,
      labels: {},
      value,
    }
  }

  return {
    name: metric.slice(0, labelStart),
    labels: parseLabels(metric.slice(labelStart + 1, -1)),
    value,
  }
}

function parseLabels(raw: string): Record<string, string> {
  const labels: Record<string, string> = {}
  const pattern = /([^=,\s]+)="((?:\\.|[^"])*)"/g

  for (const match of raw.matchAll(pattern)) {
    const [, key, value] = match
    if (!key) {
      continue
    }
    labels[key] = unescapePrometheusLabel(value ?? '')
  }

  return labels
}

function unescapePrometheusLabel(value: string): string {
  return value
    .replace(/\\"/g, '"')
    .replace(/\\n/g, '\n')
    .replace(/\\\\/g, '\\')
}
