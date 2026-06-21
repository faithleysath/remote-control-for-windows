---
name: rcw-task-workflow
description: "Use when planning or executing repository task flow in the remote-control-for-windows repo: turning work into structured issues, following the branch/worktree convention, and using PRs plus evidence instead of ad hoc main-branch development."
---

# rcw-task-workflow

Use this skill when the task is about repository workflow in `rcw`, including:

- deciding whether work should become a new issue or stay inside an existing one
- structuring issue bodies for bugs, features, or governance work
- following the repo's branch and worktree convention
- preparing or reviewing a PR with the expected validation evidence

## Scope and boundaries

This skill is a router, not the source of truth for the workflow itself.

The durable facts live in:

- repo doc: `/home/laysath/Projects/remote-control-for-windows/docs/dev-workflow.md`
- repo doc: `/home/laysath/Projects/remote-control-for-windows/docs/debug-workflow.md`
- repo doc: `/home/laysath/Projects/remote-control-for-windows/docs/testing.md`
- repo templates: `/home/laysath/Projects/remote-control-for-windows/.github/ISSUE_TEMPLATE/`
- repo template: `/home/laysath/Projects/remote-control-for-windows/.github/pull_request_template.md`

Read those first. Do not maintain a second copy of the rules here.

Environment-specific facts that may need refresh through other skills:

- `win11-main` VM or SSH access facts: `ops-libvirt-vm-platform`, `ops-remote-host-ssh`

## Default workflow

1. Read `/home/laysath/Projects/remote-control-for-windows/docs/dev-workflow.md`.
2. Decide whether the work belongs in a new issue, an existing issue, or a PR follow-up.
3. Use the repo issue templates as the default structure instead of freehand issue bodies.
4. If code changes are needed, use a dedicated branch and worktree rather than the main working tree.
5. If runtime validation or test evidence is needed, route to `docs/debug-workflow.md` or `docs/testing.md`.
6. When reporting completion, cite the issue/PR linkage, the validation layer, and the evidence paths.

## Routing guidance

### Issue work

Use the repo templates for:

- `bug`
- `feature / change`
- `governance / workflow`

If the task is still vague, first sharpen the scope in the issue before writing code.

If the work spans multiple subsystems, phases, or expected PRs, route it into a parent issue plus child issues instead of one oversized issue. Use the parent issue for the overall goal and tracking, and keep each child issue independently actionable and verifiable.

### Branch and worktree work

Follow the repo convention from `docs/dev-workflow.md`:

- branch: `issue/<number>-<slug>`
- one main issue per active branch/worktree
- main worktree is for triage, review, and release preparation, not normal feature development

### PR work

Use `.github/pull_request_template.md` as the baseline structure.

Do not report only “done” or “tested locally”. Include:

- related issue
- scope and non-goals
- validation commands
- evidence paths
- remaining gaps

## Hard rules

- Do not start normal feature work directly on `main`.
- Do not treat chat context as the only durable task record when the work should be an issue.
- Do not invent a second branch naming convention beside the repo one.
- Do not claim closure without linking the issue, PR, and validation evidence.

## Manual GitHub boundary

Some workflow rules depend on GitHub repository settings and cannot be enforced by files alone, such as branch protection on `main`.

If those settings matter to the task, say so explicitly instead of pretending the repo files already enforce them.
