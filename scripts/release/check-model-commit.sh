#!/bin/sh
# Checks that the engine's capabilities.modelCommit may stand for the source it is built from.
#
# crates/ournotes-ffi/MODEL_COMMIT is the upstream commit whose model (ournotes-sim: the simulator
# and the deck data format) the engine implements; the desktop app refuses the engine unless it
# equals the model the bdon.moe site runs. crates/ournotes-ffi/UPSTREAM_COMMIT is the upstream
# commit the engine is built from (capabilities.upstreamCommit). The two may differ only while
# MODEL_COMMIT is an ancestor of UPSTREAM_COMMIT and ournotes-sim is identical across MODEL_COMMIT,
# UPSTREAM_COMMIT and the working tree, i.e. only search-side changes were taken in. Once
# ournotes-sim changes, MODEL_COMMIT must move to UPSTREAM_COMMIT.
#
# Exit status: 0 consistent, 1 inconsistent, 2 history unavailable (e.g. shallow clone, no upstream fetched).
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
ffi="$root/crates/ournotes-ffi"
model=$(tr -d '[:space:]' < "$ffi/MODEL_COMMIT")
source=$(tr -d '[:space:]' < "$ffi/UPSTREAM_COMMIT")
model_paths="crates/ournotes-sim"

for commit in "$model" "$source"; do
  case $commit in
    *[!0-9a-f]* | '') echo "not a full lowercase SHA-1: '$commit'" >&2; exit 1 ;;
  esac
  [ ${#commit} -eq 40 ] || { echo "not a full SHA-1: '$commit'" >&2; exit 1; }
  git -C "$root" cat-file -e "$commit^{commit}" 2>/dev/null ||
    { echo "commit $commit is not in this clone; fetch upstream" >&2; exit 2; }
done

git -C "$root" merge-base --is-ancestor "$source" HEAD ||
  { echo "UPSTREAM_COMMIT $source is not an ancestor of HEAD" >&2; exit 1; }
git -C "$root" merge-base --is-ancestor "$model" "$source" ||
  { echo "MODEL_COMMIT $model is not an ancestor of UPSTREAM_COMMIT $source" >&2; exit 1; }
if ! git -C "$root" diff --quiet "$model" "$source" -- $model_paths; then
  echo "the model changed between MODEL_COMMIT and UPSTREAM_COMMIT; set MODEL_COMMIT to $source:" >&2
  git -C "$root" diff --stat "$model" "$source" -- $model_paths >&2
  exit 1
fi
if ! git -C "$root" diff --quiet "$source" -- $model_paths; then
  echo "this tree changes the model relative to UPSTREAM_COMMIT $source:" >&2
  git -C "$root" diff --stat "$source" -- $model_paths >&2
  exit 1
fi
echo "modelCommit $model, upstreamCommit $source: model unchanged"
