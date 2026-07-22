#!/bin/sh

set -eu

phase="${1:-Git 操作}"
remote_name="${2:-$(git config --get dh.githubRemote || printf '%s' origin)}"
remote_url="${3:-}"

if [ -z "$remote_url" ]; then
  remote_url="$(git remote get-url --push "$remote_name" 2>/dev/null || true)"
fi

if [ -z "$remote_url" ]; then
  printf '%s\n' "[DH Git 身份检查] 未找到推送远端：$remote_name" >&2
  exit 1
fi

remote_owner="$(
  printf '%s\n' "$remote_url" |
    sed -E \
      -e 's#^https?://github\.com/([^/]+)/.*#\1#' \
      -e 's#^ssh://git@github\.com/([^/]+)/.*#\1#' \
      -e 's#^git@github\.com:([^/]+)/.*#\1#'
)"

if [ -z "$remote_owner" ] || [ "$remote_owner" = "$remote_url" ]; then
  printf '%s\n' "[DH Git 身份检查] 远端不是可识别的 GitHub 地址：$remote_url" >&2
  exit 1
fi

expected_account="$(git config --get dh.expectedGithubAccount || true)"
if [ -z "$expected_account" ]; then
  expected_account="$remote_owner"
fi

if ! command -v gh >/dev/null 2>&1; then
  printf '%s\n' "[DH Git 身份检查] 未找到 gh，请先确认 GitHub CLI 环境。" >&2
  exit 1
fi

auth_status="$(gh auth status --hostname github.com --active 2>&1 || true)"
active_account="$(
  printf '%s\n' "$auth_status" |
    sed -nE 's/.*Logged in to github\.com account ([^ ]+).*/\1/p' |
    head -n 1
)"

git_name="$(git config user.name || true)"
git_email="$(git config user.email || true)"
expected_name="$(git config --get dh.expectedGitName || true)"
expected_email="$(git config --get dh.expectedGitEmail || true)"

printf '%s\n' "[DH Git 身份检查] $phase"
printf '%s\n' "  目标远端：$remote_url"
printf '%s\n' "  远端所有者：$remote_owner"
printf '%s\n' "  预期账号：$expected_account"
printf '%s\n' "  当前 gh 账号：${active_account:-未识别}"
printf '%s\n' "  Git 作者：${git_name:-未设置} <${git_email:-未设置}>"

if [ -z "$active_account" ]; then
  printf '%s\n' "[DH Git 身份检查] 当前活动 GitHub 账号未识别，本次操作已停止。" >&2
  printf '%s\n' "$auth_status" >&2
  exit 1
fi

active_lower="$(printf '%s' "$active_account" | tr '[:upper:]' '[:lower:]')"
expected_lower="$(printf '%s' "$expected_account" | tr '[:upper:]' '[:lower:]')"
owner_lower="$(printf '%s' "$remote_owner" | tr '[:upper:]' '[:lower:]')"

if [ "$expected_lower" != "$owner_lower" ]; then
  printf '%s\n' "[DH Git 身份检查] 本地预期账号与远端所有者不一致，本次操作已停止。" >&2
  exit 1
fi

if [ "$active_lower" != "$expected_lower" ]; then
  printf '%s\n' "[DH Git 身份检查] 当前 gh 账号与目标仓库不一致，本次操作已停止。" >&2
  printf '%s\n' "请执行：gh auth switch --hostname github.com --user $expected_account" >&2
  exit 1
fi

if [ -z "$git_name" ] || [ -z "$git_email" ]; then
  printf '%s\n' "[DH Git 身份检查] Git 作者姓名或邮箱未设置，本次操作已停止。" >&2
  exit 1
fi

if [ -n "$expected_name" ] && [ "$git_name" != "$expected_name" ]; then
  printf '%s\n' "[DH Git 身份检查] Git 作者姓名与仓库配置不一致，本次操作已停止。" >&2
  exit 1
fi

if [ -n "$expected_email" ] && [ "$git_email" != "$expected_email" ]; then
  printf '%s\n' "[DH Git 身份检查] Git 作者邮箱与仓库配置不一致，本次操作已停止。" >&2
  exit 1
fi

printf '%s\n' "[DH Git 身份检查] 身份一致，可以继续。"
