import { importWithRetry } from '@/utils/importRetry'

export const view = <T>(loader: () => Promise<T>) => () => importWithRetry(loader)
