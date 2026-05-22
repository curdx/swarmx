# Handoff Protocol

How multi-agent fullstack spells (M6a) coordinate via the blackboard
and swarm messages. **This is a convention, not a runtime contract** —
agents that don't follow it deadlock or produce inconsistent artifacts,
but flockmux-core does not enforce it.

## Standard blackboard keys

| Key              | Writer    | Reader(s)        | Schema                                                                                                            |
| ---------------- | --------- | ---------------- | ----------------------------------------------------------------------------------------------------------------- |
| `api.spec`       | backend   | frontend, test   | Markdown OR OpenAPI 3 snippet. Required fields per endpoint: method, path, request body schema, response schema, error responses. |
| `frontend.done`  | frontend  | test             | `{ commit, components: [], entry, dev_server, built_at }`                                                          |
| `frontend.error` | frontend  | test             | `{ reason, details }`                                                                                              |
| `backend.done`   | backend   | frontend, test   | `{ commit, endpoints: [], entry, run_cmd, port, built_at }`                                                        |
| `backend.error`  | backend   | frontend, test   | `{ reason, details }`                                                                                              |
| `test.passed`    | test      | system (user)    | `{ framework, passed, failed: 0, report, ran_at }`                                                                 |
| `test.failed`    | test      | system (user)    | `{ framework, passed, failed, failures: [{ name, reason }], ran_at }`                                              |
| `test.skipped`   | test      | system (user)    | `{ reason, upstream_error }` — emitted when FE or BE failed                                                        |

All `*_at` timestamps are ISO 8601 UTC.

The blackboard keeps **version history** on every write (see
`flockmux-storage`), so amendments to e.g. `api.spec` mid-build are
recoverable — read the latest version via `swarm_read_blackboard`.

## Standard swarm messages

| From     | To              | kind    | When                                | Body                                                            |
| -------- | --------------- | ------- | ----------------------------------- | --------------------------------------------------------------- |
| backend  | frontend        | reply   | After `api.spec` is written         | `"api.spec written. <N> endpoints. FE can start."`              |
| frontend | backend         | note    | When api.spec needs amendment       | `"need X endpoint for Y. please amend api.spec."`               |
| frontend | test            | reply   | After `frontend.done` written       | `"Frontend ready at commit <SHA>. Entry: ..."`                  |
| backend  | test            | reply   | After `backend.done` written        | `"Backend ready at commit <SHA>. Run with: ..."`                |
| test     | system          | reply   | After test run                      | `"✅ test passed: N tests."` or `"❌ test failed: M/N tests."`   |
| test     | system          | reply   | If upstream failed                  | `"⏭️ test skipped — <which> failed."`                            |

Messages drive **wake-check**: receiving an unread message turns the
recipient's next Stop hook into a `block` decision, giving them a
fresh turn.

**M6b update — blackboard writes also wake subscribers.** A role that
declares `depends_on = ["X"]` in its TOML front-matter is subscribed
to key `X` automatically at spell-launch. When anyone writes that key,
the server (a) drops a system note `kind="wake"` into the role's
mailbox AND (b) injects `\x15…\r` directly into its PTY input, so the
agent does NOT have to be currently mid-Stop to get reactivated. Roles
that don't declare `depends_on` still work the M6a way (other agents
explicitly notify them via swarm messages).

## Why both blackboard AND messages?

| Mechanism   | What it carries          | Wake semantics                          |
| ----------- | ------------------------ | --------------------------------------- |
| blackboard  | Structured artifacts    | Wakes any subscriber declared via `depends_on` (M6b) |
| messages    | "Something happened" signals | Triggers wake-check → fresh turn       |

The pattern is: **write the artifact to the blackboard, then send a
notification message**. Subscribers wake on the blackboard event;
unsubscribed recipients wake on the message.

This is the same pattern git+CI uses: the artifact (commit) goes to a
repo, the notification (webhook) wakes the consumer.

## Failure model

- An agent fails → writes `<role>.error` to blackboard + notifies
  downstream agents
- Downstream agents observe the error, mark their phase skipped, and
  STOP. They do NOT attempt repair (M6a). M6b will add a critic /
  fixer loop.
- The user observes the final state via the swarm panel and the
  blackboard inspector.

## Why this layout

Modeled on MetaGPT's "Code = SOP(Team)" finding (ICLR 2024): pinning
the inter-agent protocol to a small set of named slots (PRD, design,
code, tests in MetaGPT; `api.spec`, `*.done`, test.passed here)
prevents the cascade-hallucination failure mode where FE and BE drift
on their assumed API shape. The shared key namespace forces the
contract to be explicit and inspectable.
