#!/bin/sh
# Inside the code-arena-gui container: a virtual screen (Xvfb :99), the fake model, and a project to open (a small
# git repository), then waits; scripts/ui-test.sh opens and drives the window in it. The home and the project are
# under /tmp, so a test never touches the person's own.
set -eu
export DISPLAY=:99
Xvfb :99 -screen 0 1600x1000x24 -nolisten tcp >/tmp/xvfb.log 2>&1 &
python3 /src/scripts/ui/fake-model.py 18733 >/tmp/fake-model.log 2>&1 &

mkdir -p /tmp/home/config /tmp/work/src
cat >/tmp/home/config/settings.json <<'JSON'
{"arena.syncChats": false, "mcp.arenaTools": false, "mcp.codeIntelTools": false}
JSON
# The project is trusted, so the window opens without the trust question; to see it, remove this file and restart.
mkdir -p /tmp/home/data
printf '{"folders": {"/tmp/work": true}}\n' >/tmp/home/data/trust.json
cd /tmp/work
printf 'fn main() {\n    println!("hello");\n}\n' >src/main.rs
printf 'pub struct Cart {\n    items: Vec<u32>,\n}\n\nimpl Cart {\n    pub fn total(&self) -> u32 {\n        self.items.iter().sum()\n    }\n}\n' >src/lib.rs
printf 'The answer is 42.\n' >notes.txt
printf '# Work\n\nA small project for UI tests.\n' >README.md
git init -q -b main
git -c user.name=Test -c user.email=test@example.invalid add -A
git -c user.name=Test -c user.email=test@example.invalid commit -qm "start"
printf 'A change not committed.\n' >>notes.txt
touch /tmp/ready
exec sleep infinity
