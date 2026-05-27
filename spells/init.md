+++
name = "init"
description = "工作空间初始化：scout 进目录扫一眼，写 project.summary 黑板 + 给 user 发开场白"
# 必须 true：否则 server 走 PerAgent layout，scout 会进
# `<workspaces_root>/<agent_id>/` 而不是用户在 wizard 里填的项目目录。
# init 的全部价值就是让 scout cwd = 用户的真实项目目录，所以 shared 是必须。
shared_workspace = true

[[agents]]
role_ref = "scout"
+++

# init

A one-agent spell that runs the moment a new workspace is created.
The web frontend fires this from the create-workspace wizard with
`workspace_dir = <user's chosen folder>` and `task = ""` (or any
context-free string — scout doesn't need a task).

## What you'll see

1. One scout pane appears (claude). It runs ~20-40 seconds, looks
   around the directory (LS / Read / Glob / Bash with read-only
   commands), then stops.
2. By the time it stops, the blackboard has a `project.summary`
   entry and the chat shows a short greeting from scout to user.
3. The user types what they want done; the frontend dispatches that
   to `auto-dispatch`, which reads `project.summary` from the
   blackboard for context and picks the appropriate workflow spell.

## Why not use `auto-dispatch` directly

Auto-dispatch's planner is a one-shot router — it needs a natural
language task to pick a spell. At workspace creation time the user
hasn't said anything yet. Running auto-dispatch with an empty task
would make the planner guess (poorly). Init defers the planner call
until the user actually speaks, while still using the wait time to
ground future dispatches with real project context.

## When NOT to use it

- If the user already typed a concrete task at create time (e.g.
  from a recipe template or returning to an existing project), call
  the relevant spell directly with that task. Init is for the
  "blank slate" case where the user is still thinking.
