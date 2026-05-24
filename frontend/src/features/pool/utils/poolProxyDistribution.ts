export type PoolProxyDistributionMode = 'fill' | 'rewrite'

export interface PoolProxyDistributionKey {
  key_id: string
  key_name?: string | null
  proxy?: {
    node_id?: string | null
    enabled?: boolean
  } | null
}

export interface PoolProxyDistributionNode {
  id: string
  name?: string | null
}

export interface PoolProxyDistributionAssignment {
  nodeId: string
  targetCount: number
  retainedKeys: PoolProxyDistributionKey[]
  assignedKeys: PoolProxyDistributionKey[]
  changedKeys: PoolProxyDistributionKey[]
  keys: PoolProxyDistributionKey[]
}

export interface PoolProxyDistributionPlan {
  mode: PoolProxyDistributionMode
  totalKeys: number
  nodeCount: number
  maxPerNode: number
  assignments: PoolProxyDistributionAssignment[]
  retainedCount: number
  changedCount: number
  outsideSelectedProxyCount: number
  overflowCount: number
}

export interface PoolProxyDistributionOptions {
  mode: PoolProxyDistributionMode
  keys: PoolProxyDistributionKey[]
  nodes: PoolProxyDistributionNode[]
  rng?: () => number
}

interface MutableAssignment {
  nodeId: string
  targetCount: number
  retainedKeys: PoolProxyDistributionKey[]
  assignedKeys: PoolProxyDistributionKey[]
}

export function buildPoolProxyDistributionPlan(
  options: PoolProxyDistributionOptions,
): PoolProxyDistributionPlan {
  const rng = options.rng ?? Math.random
  const keys = uniqueKeys(options.keys)
  const nodeIds = uniqueNodeIds(options.nodes)
  const selectedNodeSet = new Set(nodeIds)
  const totalKeys = keys.length
  const nodeCount = nodeIds.length
  const maxPerNode = nodeCount > 0 ? Math.ceil(totalKeys / nodeCount) : 0

  if (totalKeys === 0 || nodeCount === 0) {
    return {
      mode: options.mode,
      totalKeys,
      nodeCount,
      maxPerNode,
      assignments: [],
      retainedCount: 0,
      changedCount: 0,
      outsideSelectedProxyCount: 0,
      overflowCount: 0,
    }
  }

  const targetCounts = buildTargetCounts({
    keys,
    nodeIds,
    selectedNodeSet,
    mode: options.mode,
    rng,
  })

  const mutableAssignments = new Map<string, MutableAssignment>()
  for (const nodeId of nodeIds) {
    mutableAssignments.set(nodeId, {
      nodeId,
      targetCount: targetCounts.get(nodeId) ?? 0,
      retainedKeys: [],
      assignedKeys: [],
    })
  }

  const pendingKeys: PoolProxyDistributionKey[] = []
  let outsideSelectedProxyCount = 0
  let overflowCount = 0

  if (options.mode === 'fill') {
    const keysByNode = new Map<string, PoolProxyDistributionKey[]>()
    for (const key of keys) {
      const nodeId = getKeyProxyNodeId(key)
      if (nodeId && selectedNodeSet.has(nodeId)) {
        const grouped = keysByNode.get(nodeId) ?? []
        grouped.push(key)
        keysByNode.set(nodeId, grouped)
      } else {
        if (nodeId) outsideSelectedProxyCount += 1
        pendingKeys.push(key)
      }
    }

    for (const nodeId of nodeIds) {
      const assignment = mutableAssignments.get(nodeId)
      if (!assignment) continue
      const currentKeys = shuffle(keysByNode.get(nodeId) ?? [], rng)
      const retainedKeys = currentKeys.slice(0, assignment.targetCount)
      const overflowKeys = currentKeys.slice(assignment.targetCount)
      assignment.retainedKeys.push(...retainedKeys)
      pendingKeys.push(...overflowKeys)
      overflowCount += overflowKeys.length
    }
  } else {
    pendingKeys.push(...keys)
  }

  const shuffledPendingKeys = shuffle(pendingKeys, rng)
  const slots = shuffle(buildOpenSlots(mutableAssignments), rng)
  for (let index = 0; index < shuffledPendingKeys.length; index += 1) {
    const nodeId = slots[index]
    if (!nodeId) break
    mutableAssignments.get(nodeId)?.assignedKeys.push(shuffledPendingKeys[index])
  }

  const assignments = nodeIds.map((nodeId) => {
    const assignment = mutableAssignments.get(nodeId)!
    const nodeKeys = [...assignment.retainedKeys, ...assignment.assignedKeys]
    const changedKeys = nodeKeys.filter(key => getKeyProxyNodeId(key) !== nodeId)
    return {
      nodeId,
      targetCount: assignment.targetCount,
      retainedKeys: assignment.retainedKeys,
      assignedKeys: assignment.assignedKeys,
      changedKeys,
      keys: nodeKeys,
    }
  })

  return {
    mode: options.mode,
    totalKeys,
    nodeCount,
    maxPerNode,
    assignments,
    retainedCount: assignments.reduce((sum, item) => sum + item.retainedKeys.length, 0),
    changedCount: assignments.reduce((sum, item) => sum + item.changedKeys.length, 0),
    outsideSelectedProxyCount,
    overflowCount,
  }
}

function buildTargetCounts(options: {
  keys: PoolProxyDistributionKey[]
  nodeIds: string[]
  selectedNodeSet: Set<string>
  mode: PoolProxyDistributionMode
  rng: () => number
}): Map<string, number> {
  const baseCount = Math.floor(options.keys.length / options.nodeIds.length)
  const extraCount = options.keys.length % options.nodeIds.length
  const existingCounts = new Map<string, number>()

  if (options.mode === 'fill') {
    for (const key of options.keys) {
      const nodeId = getKeyProxyNodeId(key)
      if (nodeId && options.selectedNodeSet.has(nodeId)) {
        existingCounts.set(nodeId, (existingCounts.get(nodeId) ?? 0) + 1)
      }
    }
  }

  const extraNodeIds = new Set(
    options.nodeIds
      .map(nodeId => ({
        nodeId,
        existingCount: existingCounts.get(nodeId) ?? 0,
        rank: options.rng(),
      }))
      .sort((left, right) => {
        if (options.mode === 'fill' && left.existingCount !== right.existingCount) {
          return right.existingCount - left.existingCount
        }
        return left.rank - right.rank
      })
      .slice(0, extraCount)
      .map(item => item.nodeId),
  )

  return new Map(
    options.nodeIds.map((nodeId) => [
      nodeId,
      baseCount + (extraNodeIds.has(nodeId) ? 1 : 0),
    ]),
  )
}

function buildOpenSlots(assignments: Map<string, MutableAssignment>): string[] {
  const slots: string[] = []
  for (const assignment of assignments.values()) {
    const openSlotCount = Math.max(
      assignment.targetCount - assignment.retainedKeys.length - assignment.assignedKeys.length,
      0,
    )
    for (let index = 0; index < openSlotCount; index += 1) {
      slots.push(assignment.nodeId)
    }
  }
  return slots
}

function getKeyProxyNodeId(key: PoolProxyDistributionKey): string | null {
  const nodeId = key.proxy?.node_id?.trim()
  return nodeId || null
}

function uniqueNodeIds(nodes: PoolProxyDistributionNode[]): string[] {
  const seen = new Set<string>()
  const ids: string[] = []
  for (const node of nodes) {
    const id = node.id.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    ids.push(id)
  }
  return ids
}

function uniqueKeys(keys: PoolProxyDistributionKey[]): PoolProxyDistributionKey[] {
  const seen = new Set<string>()
  const items: PoolProxyDistributionKey[] = []
  for (const key of keys) {
    const id = key.key_id.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    items.push(key)
  }
  return items
}

function shuffle<T>(items: T[], rng: () => number): T[] {
  const result = [...items]
  for (let index = result.length - 1; index > 0; index -= 1) {
    const swapIndex = Math.floor(clampRandom(rng()) * (index + 1))
    const current = result[index]
    result[index] = result[swapIndex]
    result[swapIndex] = current
  }
  return result
}

function clampRandom(value: number): number {
  if (!Number.isFinite(value)) return 0
  if (value < 0) return 0
  if (value >= 1) return 0.999999999
  return value
}
