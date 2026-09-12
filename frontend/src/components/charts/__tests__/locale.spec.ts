import { afterEach, describe, expect, it, vi } from 'vitest'
import { createApp, h, nextTick, type App } from 'vue'
import type { ChartConfiguration, ChartData, ChartOptions } from 'chart.js'

import BarChart from '../BarChart.vue'
import ScatterChart from '../ScatterChart.vue'
import CostForecastChart from '@/components/stats/CostForecastChart.vue'
import { setI18nLocale } from '@/i18n'

const { chartConstructor } = vi.hoisted(() => ({ chartConstructor: vi.fn() }))

vi.mock('chartjs-adapter-date-fns', () => ({}))
vi.mock('chart.js', async importOriginal => {
  const original = await importOriginal<typeof import('chart.js')>()
  return {
    ...original,
    Chart: class {
      static register = vi.fn()
      data: ChartData
      options: ChartOptions
      update = vi.fn()
      destroy = vi.fn()

      constructor(canvas: HTMLCanvasElement, config: ChartConfiguration) {
        this.data = config.data
        this.options = config.options ?? {}
        chartConstructor(canvas, config, this)
      }
    },
  }
})

const mountedApps: Array<{ app: App, root: HTMLElement }> = []

async function mountChart(app: App) {
  const root = document.createElement('div')
  document.body.appendChild(root)
  app.mount(root)
  mountedApps.push({ app, root })
  await nextTick()
  await nextTick()
}

function renderedChart() {
  return chartConstructor.mock.calls[chartConstructor.mock.calls.length - 1]?.[2] as {
    data: ChartData
    options: ChartOptions<'scatter'>
    update: ReturnType<typeof vi.fn>
  }
}

afterEach(() => {
  for (const { app, root } of mountedApps.splice(0)) {
    app.unmount()
    root.remove()
  }
  chartConstructor.mockClear()
})

describe('Chart locale updates', () => {
  it('redraws the scatter axis when the locale changes without changing its data', async () => {
    await mountChart(createApp({
      render: () => h(ScatterChart, {
        data: { datasets: [{ label: 'model-a', data: [{ x: 1000, y: 2 }] }] },
      }),
    }))
    const chart = renderedChart()
    const initialData = chart.data
    expect(chart.options.scales?.y?.title?.text).toBe('间隔 (分钟)')

    setI18nLocale('en-US')
    await nextTick()

    expect(chart.options.locale).toBe('en-US')
    expect(chart.options.scales?.y?.title?.text).toBe('Interval (minutes)')
    expect(chart.data).toBe(initialData)
    expect(chart.update).toHaveBeenCalledWith('none')
  })

  it('updates forecast legend labels while preserving cost values', async () => {
    await mountChart(createApp({
      render: () => h(CostForecastChart, {
        title: 'Forecast',
        history: [{ date: '2026-09-01', total_cost: 12.5 }],
        forecast: [{ date: '2026-09-02', total_cost: 13 }],
      }),
    }))
    const chart = renderedChart()
    expect(chart.data.datasets.map(dataset => dataset.label)).toEqual(['实际成本', '预测成本'])

    setI18nLocale('en-US')
    await nextTick()

    expect(chart.data.datasets.map(dataset => dataset.label)).toEqual(['Actual cost', 'Forecast cost'])
    expect(chart.data.datasets.map(dataset => dataset.data)).toEqual([[12.5, null], [null, 13]])
  })

  it('preserves unstacked bars when the locale changes', async () => {
    await mountChart(createApp({
      render: () => h(BarChart, {
        stacked: false,
        data: { labels: ['model-a'], datasets: [{ data: [2] }] },
      }),
    }))

    setI18nLocale('en-US')
    await nextTick()

    const chart = renderedChart()
    expect(chart.options.locale).toBe('en-US')
    expect(chart.options.scales?.x?.stacked).toBe(false)
    expect(chart.options.scales?.y?.stacked).toBe(false)
  })
})
