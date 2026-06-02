+++
id = "backend"
name = "Backend Engineer"
description = "后端逻辑、API、数据层、系统/shell-heavy 活。"
default_cli = "codex"
# Abstract tier — resolved PER-CLI by the model settings (设置→模型 / /api/models).
# On codex it maps to whatever model you mapped for "sonnet", or codex's own
# default if unmapped (never forwards the bare Claude tier name → no 503).
default_model_tier = "sonnet"
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
