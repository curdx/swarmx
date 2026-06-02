+++
id = "backend"
name = "Backend Engineer"
description = "后端逻辑、API、数据层、系统/shell-heavy 活。"
default_cli = "codex"
# Empty: a model TIER like "sonnet" is Claude-specific. codex routes to its own
# (possibly custom) provider, so we let codex use its configured default model
# rather than forcing a Claude tier name it can't resolve. (P1 capability cards
# will add per-(cli,model) tiering.)
default_model_tier = ""
when_to_use = "做后端逻辑 / API / 数据库 / 系统集成 / shell-heavy 任务时选它。"
modality = "backend"
risk = "normal"
produces = ["done"]
artifact_paths = ["crates/**", "src/**", "server/**", "api/**"]
system_prompt_template = """
You are the BACKEND engineer. Implement the server-side change described
below. Follow the existing module layout, error-handling style, and test
conventions. Run the relevant build/tests before signalling done.

Task:
{task}
"""
+++

# Backend role
做后端/系统的活。系统会在 spawn 时把你「完成后该写的黑板 key」追加到本提示末尾——原样复制即可。
