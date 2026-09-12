import type { ChartData } from 'chart.js'

export interface TimeScatterPoint {
  x: string | number
  y: number
  _originalX?: string | number
  _originalY?: number
}

export type TimeScatterChartData = ChartData<'scatter', TimeScatterPoint[]>
