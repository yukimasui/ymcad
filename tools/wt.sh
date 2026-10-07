#!/usr/bin/env bash
# 作業ブランチごとの作業ツリーを、リポジトリ直下の .worktrees/<名前> にまとめて作る。
#
# 目的:
#   兄弟ディレクトリに作業ツリーを散らかさず、依存クレートのコンパイルを
#   全作業ツリーで 1 回に済ませる。
#
# ビルド先を共有する仕組み:
#   <リポジトリ>/.worktrees/.cargo/config.toml に
#       [build]
#       target-dir = "../target"
#   を置く（このスクリプトが無ければ自動で作る）。.worktrees/<名前> の中で走る cargo は
#   親ディレクトリの .cargo/config.toml を読むので、ビルド先が <リポジトリ>/target
#   1 つになる（相対パスは .cargo を含むディレクトリ .worktrees/ 基準）。
#
# 並行ビルド:
#   複数の作業ツリーで同時に cargo を走らせると、cargo のロックで順番待ちになる。
#   「Blocking waiting for file lock」と出るのは正常で、止まっているわけではない。
#
# Claude Code の isolation: "worktree" は使わないこと:
#   .claude/worktrees/ に作られるため共有の設定が効かず、基点も古くなりうる。
#   代わりにこのスクリプトで作った作業ツリーを使う。
#
# 使い方:
#   tools/wt.sh new <ブランチ名> [基点]   基点の既定は origin/develop。最後の行にパスを出す
#   tools/wt.sh review <ブランチ名|PR番号>  origin/<ブランチ> を detach で .worktrees/review-<名前> に出す
#   tools/wt.sh rm <名前>                  作業ツリーを消す（未コミットの変更があれば git が拒否する）
#   tools/wt.sh list                       git worktree list
#   tools/wt.sh path <名前>                絶対パスを出す
set -euo pipefail

usage() {
    cat >&2 <<'EOF'
使い方:
  tools/wt.sh new <ブランチ名> [基点]     基点の既定は origin/develop
  tools/wt.sh review <ブランチ名|PR番号>  レビュー用に detach で出す
  tools/wt.sh rm <名前>                   作業ツリーを消す
  tools/wt.sh list                        作業ツリーの一覧
  tools/wt.sh path <名前>                 作業ツリーの絶対パス
EOF
    exit 2
}

[ $# -ge 1 ] || usage
cmd=$1
shift

# 本体のリポジトリのルート（どの作業ツリーの中から実行しても同じになる）
common=$(git rev-parse --path-format=absolute --git-common-dir)
root=$(dirname "$common")
wtdir="$root/.worktrees"

ensure_cargo_config() {
    mkdir -p "$wtdir/.cargo"
    if [ ! -e "$wtdir/.cargo/config.toml" ]; then
        printf '[build]\ntarget-dir = "../target"\n' >"$wtdir/.cargo/config.toml"
    fi
    if ! git -C "$root" check-ignore -q .worktrees/; then
        echo "警告: .worktrees/ が .gitignore に無い" >&2
    fi
}

# ブランチ名の / を - にしたもの
slug() { printf '%s' "${1//\//-}"; }

case "$cmd" in
new)
    [ $# -ge 1 ] && [ $# -le 2 ] || usage
    branch=$1
    base=${2:-origin/develop}
    ensure_cargo_config
    dest="$wtdir/$(slug "$branch")"
    git -C "$root" fetch origin
    if [ -e "$dest" ]; then
        echo "作業ツリーは既にある: $dest" >&2
    elif git -C "$root" show-ref --verify --quiet "refs/heads/$branch"; then
        echo "ブランチ $branch は既にあるので、それを出す（基点 $base は無視）" >&2
        git -C "$root" worktree add "$dest" "$branch" >&2
    else
        git -C "$root" worktree add --no-track -b "$branch" "$dest" "$base" >&2
    fi
    echo "$dest"
    ;;
review)
    [ $# -eq 1 ] || usage
    arg=$1
    ensure_cargo_config
    if [[ "$arg" =~ ^[0-9]+$ ]]; then
        branch=$(gh pr view "$arg" --json headRefName -q .headRefName)
    else
        branch=$arg
    fi
    dest="$wtdir/review-$(slug "$branch")"
    git -C "$root" fetch origin
    if [ -e "$dest" ]; then
        git -C "$dest" checkout --detach "origin/$branch" >&2
    else
        git -C "$root" worktree add --detach "$dest" "origin/$branch" >&2
    fi
    echo "$dest"
    ;;
rm)
    [ $# -eq 1 ] || usage
    ensure_cargo_config
    git -C "$root" worktree remove "$wtdir/$1"
    ;;
list)
    [ $# -eq 0 ] || usage
    git -C "$root" worktree list
    ;;
path)
    [ $# -eq 1 ] || usage
    echo "$wtdir/$1"
    ;;
*)
    usage
    ;;
esac
