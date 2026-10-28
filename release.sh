#!/usr/bin/env bash
# pushing the tag is what builds and publishes the release (.github/workflows/release.yml)
set -euo pipefail
cd "$(dirname "$0")"

fail() {
  printf '\n  %s\n\n' "$1" >&2
  exit 1
}

branch=$(git rev-parse --abbrev-ref HEAD)
[[ $branch == main ]] || fail "On $branch. Releases are cut from main."
[[ -z $(git status --porcelain) ]] || fail "Working tree is dirty. Commit or stash before releasing."

git fetch --quiet origin --tags
# a tag on a commit origin has never seen points at nothing anyone can fetch
read -r ahead behind < <(git rev-list --left-right --count main...origin/main)
((behind == 0)) || fail "main is $behind commit(s) behind origin/main. Pull first."

semver='^v?([0-9]+)\.([0-9]+)\.([0-9]+)$'
latest=$(git tag --list 'v*' --sort=-v:refname | grep -E "$semver" | head -n1 || true)
if [[ $latest =~ $semver ]]; then
  major=${BASH_REMATCH[1]} minor=${BASH_REMATCH[2]} patch=${BASH_REMATCH[3]}
  recommended="$major.$minor.$((patch + 1))"
else
  recommended=0.1.0
fi

current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
[[ -n $current ]] || fail "No version line found in Cargo.toml."

printf '\n  main        %s  %s\n' "$(git rev-parse --short HEAD)" "$(git log -1 --format=%s)"
printf '  Cargo.toml  %s\n' "$current"
printf '  latest tag  %s\n\n' "${latest:-none}"
if [[ -n $latest ]]; then
  printf '    patch  %s   (recommended)\n' "$recommended"
  printf '    minor  %s\n' "$major.$((minor + 1)).0"
  printf '    major  %s\n\n' "$((major + 1)).0.0"
fi

read -rp "  Next version [$recommended]: " answer
version=${answer:-$recommended}
version=${version#v}
[[ $version =~ $semver ]] || fail "\"$version\" is not a version of the form MAJOR.MINOR.PATCH."
tag="v$version"
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  fail "$tag already exists. Releasing over a tag rewrites what it means."
fi

bump=false
[[ $current == "$version" ]] || bump=true

printf '\n  This will:\n'
if $bump; then printf '    - set Cargo.toml to %s and commit it\n' "$version"; fi
if $bump || ((ahead > 0)); then printf '    - push main to origin\n'; fi
printf '    - tag %s and push it, which builds and publishes the binaries\n' "$tag"

# declining is an answer, not a failure
read -rp $'\n'"  Release $tag? [y/N]: " confirm
if [[ $confirm != [yY] ]]; then
  printf '\n  Nothing was pushed.\n\n'
  exit 0
fi

if $bump; then
  awk -v v="$version" '!done && /^version = "/ { $0 = "version = \"" v "\""; done = 1 } 1' \
    Cargo.toml >Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
  cargo update --workspace --offline --quiet
  git add Cargo.toml Cargo.lock
  git commit --quiet -m "Set the version to $version"
  printf '\n  Committed the version bump.\n'
fi

git push --quiet origin main
git tag -a "$tag" -m "skybin $version"
git push --quiet origin "$tag"

remote=$(git remote get-url origin)
remote=${remote%.git}
remote=${remote/git@github.com:/https://github.com/}
printf '  Pushed %s.\n\n' "$tag"
printf '  Build:   %s/actions\n' "$remote"
printf '  Release: %s/releases/tag/%s\n\n' "$remote" "$tag"
