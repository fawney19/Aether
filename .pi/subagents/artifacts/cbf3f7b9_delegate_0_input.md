# Task for delegate

你是只读的夜间监督代理，不是源码 writer，也不是第二个 Coordinator。目标是监督现有 Herdr Coordinator w5:p6 完成 Aether key-scope 工作流。

现场：repo=/Users/zbs/projectwork/zbs-develop/Aether；worktree=/Users/zbs/.herdr/worktrees/Aether/workflow-key-scope；旧 run=/Users/zbs/.local/state/herdr-codex-pi-workflow/Aether/run-20260813-153010-key-scope；现有 Coordinator=w5:p6；当前 writer=implementer-key-scope-2(w7:p2)。

硬约束：
- 永远不要编辑 Aether 源码、测试、配置或 Git index；不要自己 commit。
- 永远不要启动或提示源码 writer；现有 Coordinator 是唯一编排者。
- 不并发创建 Reviewer/Planner/Implementer。
- 实施模型保持 opencode-go/deepseek-v4-flash thinking=max；禁止自动 Pro/Terra。
- 禁止 merge master/main、tag、GitHub Release、GHCR publish、远程服务器或生产部署。
- Reviewer PASS 后允许 Coordinator push workflow/key-scope、创建/更新 Draft PR 触发 rust-ci；CI 通过后允许 workflow_dispatch release.yml ref=workflow/key-scope（必须确认是 branch 非 tag、preflight publish=false），只生成 artifacts。

监督方法：
1. 使用 herdr agent wait w5:p6 --timeout 43200000 等待 Coordinator settled；不要高频轮询。
2. settled 后读取 state、IMPLEMENTATION/TEST_RESULTS/最新 REVIEW、git status/log、gh PR/check/run/artifact 状态。
3. 完成条件：最新独立 Codex Reviewer PASS；reviewed commit==worktree HEAD；worktree clean；目标测试证据有效；workflow/key-scope 已 push；Draft PR 存在且 Rust CI 全绿；release.yml 在该 branch 的手动非发布构建成功；下载或至少核验 artifact 列表，记录 URLs、commit、artifact names/checksums。
4. 若 Coordinator 提前 settled 但条件未满足，且不存在不可逆/凭据/需求实质变更问题，只通过 `herdr agent prompt w5:p6` 发送具体下一步，随后再次 `herdr agent wait w5:p6`。最多提醒 6 次。不要自己执行推进动作。
5. 若出现权限绕过、数据一致性、迁移/回滚 blocking finding，不允许接受风险；提醒 Coordinator 按 finding 分类继续 Plan Revision 或新的 Flash Implementer。
6. 如果遇到凭据、不可逆外部副作用、实质需求变化、CI 仅能通过发布 tag 触发等情况，停止并记录 BLOCKED，不猜测。
7. 将最终监督报告写到指定输出，包含状态、commit、测试、Review verdict、PR/CI/build URLs、artifacts/checksum、未完成项和是否等待用户部署批准。

当前任务可能持续数小时。用 Herdr 的 agent wait 等待事件，不要用紧密 sleep/poll 循环。

---
**Output:**
Write your findings to exactly this path: /Users/zbs/.herdr-codex-pi-workflow/tasks/Aether/run-20260813-153010-key-scope/OVERNIGHT-SUPERVISOR.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: checked
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope
- criterion-2: Return evidence sufficient for an independent acceptance review

Required evidence: changed-files, tests-added, commands-run, residual-risks, no-staged-files

Review gate: required by reviewer.

Finish with a fenced JSON block tagged `acceptance-report` in this shape:
Use empty arrays when no items apply; array fields contain strings unless object entries are shown.
`criteriaSatisfied[].status` must be exactly one of: satisfied, not-satisfied, not-applicable.
`commandsRun[].result` must be exactly one of: passed, failed, not-run.
`manualNotes` and `notes` are optional strings; an empty string means no note and does not satisfy `manual-notes` evidence.
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    },
    {
      "id": "criterion-2",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}
```