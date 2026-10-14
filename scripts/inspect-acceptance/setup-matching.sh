#!/bin/sh
# Fixtures for the matching acceptance (build host). Idempotent.
#   react-app        git repo (base commit) with .chrome-use/inspect.json
#   react-app-wt2    git worktree of react-app on branch wt2, one extra commit
#   mono             monorepo: apps/admin (/admin), apps/shop (/shop), packages/shared
#   wild             repo whose config uses http://*.wild.inspect.example
set -eu
cd "$(dirname "$0")"
DIR=$PWD
git_id() { git -c user.name=fixture -c user.email=fixture@example.invalid "$@"; }

# react-app as a git repo with a tracked config
cd "$DIR/react-app"
if [ ! -d .git ]; then
  printf 'node_modules\ndist\ndist-test\n' > .gitignore
  git init -q -b main
fi
mkdir -p .chrome-use
cat > .chrome-use/inspect.json <<'EOF'
{"version": 1, "sites": [{"origins": ["http://test.inspect.example", "http://localhost:5173"]}]}
EOF
git add -A && (git diff --cached --quiet || git_id commit -qm "fixture base")
BASE=$(git rev-parse HEAD)

# second worktree with one extra commit
if [ ! -d "$DIR/react-app-wt2" ]; then
  git worktree add -q -b wt2 "$DIR/react-app-wt2" "$BASE"
  ln -s "$DIR/react-app/node_modules" "$DIR/react-app-wt2/node_modules"
  (cd "$DIR/react-app-wt2" && echo "// wt2" >> src/main.jsx && git add src/main.jsx && git_id commit -qm "wt2 change")
fi
WT2=$(git -C "$DIR/react-app-wt2" rev-parse HEAD)

# monorepo
mkdir -p "$DIR/mono"
cd "$DIR/mono"
[ -d .git ] || git init -q -b main
for app in admin shop; do
  mkdir -p "apps/$app/src" "apps/$app/.chrome-use"
  cat > "apps/$app/src/App.tsx" <<EOF
export const App = () => '$app';
EOF
  cat > "apps/$app/.chrome-use/inspect.json" <<EOF
{"version": 1, "sites": [{"origins": ["http://apps.inspect.example", "http://localhost:5175"], "pathScope": "/$app"}]}
EOF
done
mkdir -p packages/shared
echo "export const shared = 1;" > packages/shared/x.ts
cat > apps/admin/index.html <<'EOF'
<!doctype html><meta charset="utf-8"><title>admin</title>
<button class="admin-save" data-insp-path="apps/admin/src/App.tsx:1:1:button">Save admin</button>
<div class="forged-shop" data-insp-path="apps/shop/src/App.tsx:1:1:div">Forged shop mark</div>
<span class="forged-shared" data-insp-path="../../packages/shared/x.ts:1:1:span">Forged shared mark</span>
EOF
cat > apps/shop/index.html <<'EOF'
<!doctype html><meta charset="utf-8"><title>shop</title>
<button class="shop-buy" data-insp-path="apps/shop/src/App.tsx:1:1:button">Buy</button>
EOF
git add -A && (git diff --cached --quiet || git_id commit -qm "mono fixture")

# wildcard repo
mkdir -p "$DIR/wild/.chrome-use"
cd "$DIR/wild"
[ -d .git ] || git init -q -b main
cat > .chrome-use/inspect.json <<'EOF'
{"version": 1, "sites": [{"origins": ["http://*.wild.inspect.example"]}]}
EOF
mkdir -p src && echo "export const w = 1;" > src/w.ts
cat > index.html <<'EOF'
<!doctype html><meta charset="utf-8"><title>wild</title>
<button class="wild-go" data-insp-path="src/w.ts:1:1:button">Go</button>
EOF
git add -A && (git diff --cached --quiet || git_id commit -qm "wild fixture")

# a page for the revision scenario: marks exist in both worktrees, revision = wt2 HEAD
mkdir -p "$DIR/rev-site"
cat > "$DIR/rev-site/index.html" <<EOF
<!doctype html><meta charset="utf-8"><title>rev</title><meta name="chrome-use-revision" content="$WT2">
<button class="rev-save" data-insp-path="src/OrderForm.jsx:6:5:button">Save rev</button>
EOF
echo "BASE=$BASE WT2=$WT2"
