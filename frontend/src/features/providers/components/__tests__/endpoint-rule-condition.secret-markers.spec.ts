import { describe, expect, it } from 'vitest'

import type { BodyRuleCondition } from '@/api/endpoints'
import {
  conditionToEditable,
  editableConditionToApi,
  type EditableConditionLeaf,
  type EditableConditionNode,
} from '../endpoint-rule-condition'

function findLeaf(node: EditableConditionNode | null): EditableConditionLeaf {
  if (!node) throw new Error('condition was not converted')
  if (node.kind === 'leaf') return node
  const child = node.children[0]
  if (!child || child.kind !== 'leaf') throw new Error('condition leaf was not converted')
  return child
}

describe('endpoint condition secret markers', () => {
  it('round-trips a masked request-header condition through a nested group', () => {
    const condition: BodyRuleCondition = {
      all: [{
        source: 'request_headers',
        path: 'x-tenant-token',
        op: 'eq',
        value: '***',
        has_value: true,
      }],
    }

    const editable = conditionToEditable(condition)

    expect(findLeaf(editable).retainValue).toBe(true)
    expect(editableConditionToApi(editable)).toEqual(condition)
  })

  it('drops the marker after the user replaces the masked value', () => {
    const editable = conditionToEditable({
      source: 'request_headers',
      path: 'authorization',
      op: 'eq',
      value: '***',
      has_value: true,
    })
    const leaf = findLeaf(editable)
    leaf.value = 'Bearer replacement-token'
    leaf.retainValue = false

    expect(editableConditionToApi(editable)).toEqual({
      source: 'request_headers',
      path: 'authorization',
      op: 'eq',
      value: 'Bearer replacement-token',
    })
  })
})
