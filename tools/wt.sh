#!/usr/bin/env bash
# 作業ブランチごとの作業ツリーを、リポジトリ直下の .worktrees/<名前> にまとめて作る。
#
# 目的:
#   兄弟ディレクトリに作業ツリーを散らかさず、リポジトリ 1 つのディレクトリの中で完結させる。
#
# ビルド先は共有しない（作業ツリーごとに .worktrees/<名前>/target）:
#   .worktrees/.cargo/config.toml で target-dir を <リポジトリ>/target に寄せると依存の
#   コンパイルは 1 回で済むが、cargo はパスの違う作業ツリーの同じクレートを同じものとして扱い、
#   新しさをファイルの更新時刻だけで判断する。そのため別の作業ツリーで作ったテストの
#   バイナリがそのまま走り、検証の結果が黙って間違う（2026-10-07 にレビューで実際に起きた）。
#   以前のスクリプトが作った .worktrees/.cargo/config.toml が残っていれば警告する。
#
# Claude Code の isolation: "worktree" は使わないこと:
#   .claude/worktrees/ に作られ、基点が origin/develop でなく古い main になることがある。
#   代わりにこのスクリプトで作った作業ツリーを使う。
#
# 使い方:
#   tools/wt.sh new <ブランチ名> [基点]   基点の既定は origin/develop。最後の行にパスを出す
#                                          ローカルに無く origin にあるブランチ（clone 直後の PR のブランチなど）は
#                                          origin/<ブランチ> から作る（基点は無視）。develop から作り直すと PR の中身が入らない
#   tools/wt.sh review <ブランチ名|PR番号>  origin/<ブランチ> を detach で .worktrees/review-<名前> に出す
#   tools/wt.sh rm <名前>                  作業ツリーを消す（未コミットの変更があれば git が拒否する）。
#                                          ローカルのブランチは残る（消すなら git branch -d <ブランチ>）
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

check_worktrees_dir() {
    mkdir -p "$wtdir"
    if [ -e "$wtdir/.cargo/config.toml" ]; then
        echo "警告: $wtdir/.cargo/config.toml がある。ビルド先の共有は古いバイナリが走るのでやめた。消すこと" >&2
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
    check_worktrees_dir
    dest="$wtdir/$(slug "$branch")"
    git -C "$root" fetch origin
    if [ -e "$dest" ]; then
        echo "作業ツリーは既にある: $dest" >&2
    elif git -C "$root" show-ref --verify --quiet "refs/heads/$branch"; then
        echo "ブランチ $branch は既にあるので、それを出す（基点 $base は無視）" >&2
        git -C "$root" worktree add "$dest" "$branch" >&2
    elif git -C "$root" show-ref --verify --quiet "refs/remotes/origin/$branch"; then
        echo "ブランチ $branch は origin にあるので、origin/$branch から作る（基点 $base は無視）" >&2
        git -C "$root" worktree add --track -b "$branch" "$dest" "origin/$branch" >&2
    else
        git -C "$root" worktree add --no-track -b "$branch" "$dest" "$base" >&2
    fi
    echo "$dest"
    ;;
review)
    [ $# -eq 1 ] || usage
    arg=$1
    check_worktrees_dir
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
    check_worktrees_dir
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
