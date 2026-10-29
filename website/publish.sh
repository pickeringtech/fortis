#!/bin/bash
# Publish the built site: commit website/dist to the orphan branch `website` (the site alone, index.html at its
# root) through a temporary git worktree, and push it. Idempotent: when the built site is unchanged, no commit is
# made. Never force-pushes, except that the first push creates the branch.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
BRANCH=website

python3 -I website/build.py

TMP="$(mktemp -d)"
cleanup() {
  git worktree remove --force "$TMP/wt" >/dev/null 2>&1 || true
  rm -rf "$TMP"
}
trap cleanup EXIT

git fetch -q origin "refs/heads/$BRANCH:refs/remotes/origin/$BRANCH" >/dev/null 2>&1 || true
base="$(git rev-parse -q --verify "refs/remotes/origin/$BRANCH" || git rev-parse -q --verify "refs/heads/$BRANCH" || true)"

git worktree add --no-checkout --detach "$TMP/wt" HEAD >/dev/null
cd "$TMP/wt"
git read-tree --empty
cp -a "$ROOT/website/dist/." "$TMP/wt/"
git add -A
new_tree="$(git write-tree)"

if [ -n "$base" ]; then
  if [ "$new_tree" = "$(git rev-parse "$base^{tree}")" ]; then
    echo "publish: the site is unchanged since ${base:0:12}; no commit made, nothing pushed"
    exit 0
  fi
  git update-ref "refs/heads/$BRANCH" "$base"
fi

git symbolic-ref HEAD "refs/heads/$BRANCH"
git commit -q -m "website: the static site built from $(git -C "$ROOT" rev-parse --short=12 HEAD)"
cd "$ROOT"                      # the remote's URL resolves from the repository, not from the temporary worktree
git push origin "$BRANCH"
echo "publish: pushed $BRANCH at $(git rev-parse --short=12 "refs/heads/$BRANCH")"
