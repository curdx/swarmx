#!/usr/bin/env bash
# Install a local pre-commit hook that runs the harness check before every
# commit.
#
# Why local matters here: this repo pushes STRAIGHT TO main (no PR), so the
# local pre-commit hook is the primary pre-merge gate — the CI `harness` job is
# only a backstop that runs after the push has already landed. Re-run this
# script once after a fresh clone. Emergency bypass: `git commit --no-verify`.
set -euo pipefail

repo_root="$(git rev-parse --show-toplevel)"
hook="$repo_root/.git/hooks/pre-commit"

cat > "$hook" <<'EOF'
#!/usr/bin/env bash
# flockmux pre-commit — mechanical cross-file invariant guards.
# Installed by scripts/install-git-hooks.sh. Logic in scripts/harness-check.mjs.
set -euo pipefail
command -v node >/dev/null 2>&1 || {
  echo "pre-commit: harness-check 需要 node（临时绕过：git commit --no-verify）" >&2
  exit 1
}
# git runs hooks from the repo root, so cwd is correct for the script's
# process.cwd()-relative file reads.
exec node "$(git rev-parse --show-toplevel)/scripts/harness-check.mjs"
EOF

chmod +x "$hook"
echo "✅ pre-commit hook 已安装 → ${hook}"
echo "   每次 git commit 前自动跑 node scripts/harness-check.mjs"
echo "   紧急绕过：git commit --no-verify"
