import { describe, expect, it } from 'vitest'

import {
  buildPoolProxyDistributionPlan,
  type PoolProxyDistributionKey,
} from '@/features/pool/utils/poolProxyDistribution'

const nodes = [
  { id: 'node-a', name: 'Node A' },
  { id: 'node-b', name: 'Node B' },
]

function key(id: string, nodeId?: string | null): PoolProxyDistributionKey {
  return {
    key_id: id,
    key_name: id,
    proxy: nodeId ? { node_id: nodeId, enabled: true } : null,
  }
}

function fixedRng(): () => number {
  return () => 0
}

function assignedIds(plan: ReturnType<typeof buildPoolProxyDistributionPlan>): string[] {
  return plan.assignments.flatMap(item => item.keys.map(key => key.key_id)).sort()
}

describe('buildPoolProxyDistributionPlan', () => {
  it('keeps existing selected-node proxy bindings and fills empty capacity', () => {
    const plan = buildPoolProxyDistributionPlan({
      mode: 'fill',
      nodes,
      rng: fixedRng(),
      keys: [
        key('a-1', 'node-a'),
        key('a-2', 'node-a'),
        key('b-1', 'node-b'),
        key('new-1', null),
      ],
    })

    expect(plan.totalKeys).toBe(4)
    expect(plan.maxPerNode).toBe(2)
    expect(plan.retainedCount).toBe(3)
    expect(plan.changedCount).toBe(1)
    expect(assignedIds(plan)).toEqual(['a-1', 'a-2', 'b-1', 'new-1'])
    expect(plan.assignments.map(item => item.keys).map(keys => keys.length).sort()).toEqual([2, 2])
  })

  it('moves overflowed existing bindings before final assignment', () => {
    const plan = buildPoolProxyDistributionPlan({
      mode: 'fill',
      nodes,
      rng: fixedRng(),
      keys: [
        key('a-1', 'node-a'),
        key('a-2', 'node-a'),
        key('a-3', 'node-a'),
        key('a-4', 'node-a'),
        key('b-1', 'node-b'),
      ],
    })

    const nodeA = plan.assignments.find(item => item.nodeId === 'node-a')!
    const nodeB = plan.assignments.find(item => item.nodeId === 'node-b')!

    expect(plan.maxPerNode).toBe(3)
    expect(plan.overflowCount).toBe(1)
    expect(nodeA.keys).toHaveLength(3)
    expect(nodeB.keys).toHaveLength(2)
    expect(nodeB.changedKeys).toHaveLength(1)
    expect(assignedIds(plan)).toEqual(['a-1', 'a-2', 'a-3', 'a-4', 'b-1'])
  })

  it('reassigns accounts bound to non-selected proxy nodes', () => {
    const plan = buildPoolProxyDistributionPlan({
      mode: 'fill',
      nodes,
      rng: fixedRng(),
      keys: [
        key('outside-1', 'node-c'),
        key('empty-1', null),
      ],
    })

    expect(plan.outsideSelectedProxyCount).toBe(1)
    expect(plan.changedCount).toBe(2)
    expect(plan.assignments.every(item => item.keys.length === 1)).toBe(true)
  })

  it('force rewrites all accounts into balanced random targets', () => {
    const plan = buildPoolProxyDistributionPlan({
      mode: 'rewrite',
      nodes,
      rng: fixedRng(),
      keys: [
        key('k-1', 'node-a'),
        key('k-2', 'node-a'),
        key('k-3', 'node-b'),
        key('k-4', null),
        key('k-5', 'node-c'),
      ],
    })

    expect(plan.maxPerNode).toBe(3)
    expect(plan.retainedCount).toBe(0)
    expect(assignedIds(plan)).toEqual(['k-1', 'k-2', 'k-3', 'k-4', 'k-5'])
    expect(plan.assignments.map(item => item.keys).map(keys => keys.length).sort()).toEqual([2, 3])
    expect(plan.assignments.every(item => item.keys.length <= plan.maxPerNode)).toBe(true)
  })

  it('supports fewer accounts than selected proxy nodes', () => {
    const plan = buildPoolProxyDistributionPlan({
      mode: 'rewrite',
      nodes: [
        ...nodes,
        { id: 'node-c', name: 'Node C' },
      ],
      rng: fixedRng(),
      keys: [key('only-1', null)],
    })

    expect(plan.maxPerNode).toBe(1)
    expect(plan.assignments.map(item => item.keys).map(keys => keys.length).sort()).toEqual([0, 0, 1])
    expect(assignedIds(plan)).toEqual(['only-1'])
  })
})
