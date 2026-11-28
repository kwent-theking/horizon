#!/usr/bin/env bash
# Push the checkout's code (crates/, mods/horizon/, Cargo.*) on top of the fork's
# plan/v5 as branch ci/proto, for GitHub Actions to compile and test.
# Never touches the checkout or its index. stand/ (test server files, RCON
# password, world) stays local. The Cloud bootstrap also embeds this script.
set -euo pipefail

if [[ ${1:-} == --cloud-steering ]]; then
    branch=refs/heads/ci/proto-c
    cd "$(git rev-parse --show-toplevel)"
    case "$(git remote get-url --push --all origin)" in
        https://github.com/kwent-theking/horizon|https://github.com/kwent-theking/horizon.git) ;;
        *) echo "push-ci: unexpected Cloud push destination" >&2; exit 1 ;;
    esac
    base=$(git rev-parse HEAD)
    remote=$(git ls-remote --exit-code origin "$branch" | cut -f1)
    if [[ $base != "$remote" ]] || ! git diff --cached --quiet; then
        echo "push-ci: check out the current ci/proto-c tip and leave the candidate unstaged" >&2
        exit 1
    fi

    list=$(mktemp)
    index=$(mktemp)
    trap 'rm -f "$list" "$index"' EXIT
    {
        git diff --name-only --no-renames -z HEAD
        git ls-files --others --exclude-standard -z
    } > "$list"
    if [[ ! -s $list ]]; then
        echo "push-ci: no Cloud candidate changes" >&2
        exit 1
    fi
    while IFS= read -r -d '' path; do
        if [[ ! $path =~ ^crates/(horizon-client|horizon-protocol)/[a-zA-Z0-9_./-]+\.rs$ || -L $path ]]; then
            echo "push-ci: only regular Rust source changes in the two client crates are allowed" >&2
            exit 1
        fi
    done < "$list"
    rm -f "$index"
    export GIT_INDEX_FILE=$index
    git read-tree "$base"
    git add -A --pathspec-from-file="$list" --pathspec-file-nul
    git diff --cached --check
    tree=$(git write-tree)
    commit=$(git -c user.name="Horizon CI" -c user.email="horizon-ci@users.noreply.github.com" \
        commit-tree "$tree" -p "$base" -m "ci: cloud steering candidate")
    git push origin "$commit:$branch"
    echo "push-ci: pushed $commit"
    exit 0
fi

REPO=${REPO:-/c/Users/kwent/horizon/horizon-rust}
BASE=fork/plan/v5
WORKFLOW=${WORKFLOW:-/c/Users/kwent/horizon/horizon-test/tools/proto-ci.yml}
BRANCH=${CI_BRANCH:-ci/proto}
case "$BRANCH" in
    ci/proto|ci/proto-b|ci/proto-c|ci/proto-d) ;;
    *) echo "push-ci: refusing to push outside the Horizon CI branches" >&2; exit 1 ;;
esac
SCRIPT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/push-ci.sh
cd "$REPO"

list=$(mktemp)
{
    git diff --name-only "$BASE" -- crates mods/horizon launcher Cargo.toml Cargo.lock
    git ls-files --others --exclude-standard -- crates mods/horizon launcher
} | grep -v -e 'ruvector' -e '^stand/' | sort -u > "$list"
echo "push-ci: $(wc -l < "$list") code files differ from $BASE"

export GIT_INDEX_FILE
GIT_INDEX_FILE=$(mktemp -u)
trap 'rm -f "$GIT_INDEX_FILE" "$list"' EXIT
git read-tree "$BASE"
# Batch the changed paths so the guarded launcher starts Git once, not once per path.
git update-index --add --remove --stdin < "$list"
# Only this branch's own workflow runs on it.
git ls-files -- .github/workflows | while IFS= read -r w; do
    git update-index --force-remove -- "$w"
done
blob=$(git hash-object -w "$WORKFLOW")
git update-index --add --cacheinfo 100644,"$blob",.github/workflows/proto-ci.yml
push_option=-f
if [[ ${CI_CLOUD_STEERING:-0} == 1 ]]; then
    [[ $BRANCH == ci/proto-c ]] || { echo "push-ci: Cloud script belongs on ci/proto-c" >&2; exit 1; }
    expected=${CI_CLOUD_BOOTSTRAP_EXPECTED:?set the exact current ci/proto-c SHA}
    [[ $expected =~ ^[0-9a-f]{40}$ ]] || { echo "push-ci: invalid Cloud bootstrap base" >&2; exit 1; }
    blob=$(git hash-object -w "$SCRIPT")
    git update-index --add --cacheinfo 100644,"$blob",.github/horizon-ci/push-ci.sh
    push_option=--force-with-lease="refs/heads/$BRANCH:$expected"
fi

if git ls-files | grep -q '^stand/'; then
    echo "push-ci: refusing, stand/ is in the tree"
    exit 1
fi
tree=$(git write-tree)
commit=$(git commit-tree "$tree" -p "$BASE" -m "ci: proto/v1 code snapshot $(date +%F_%T)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>")
git push "$push_option" fork "$commit:refs/heads/$BRANCH"
echo "push-ci: pushed $commit"
