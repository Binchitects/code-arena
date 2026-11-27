#!/bin/sh
# Drives the native window off-screen for UI tests: a container of the code-arena-gui image (docker/gui-test) with a
# virtual screen, the fake model (scripts/ui/fake-model.py) and a small project, the window being this checkout's
# target/debug/code-arena (build it first: scripts/in-builder.sh cargo build -p ca-cli). One container per checkout,
# so several can run at once.
#
#   scripts/ui-test.sh start               the screen, the model and the window (opened on /tmp/work)
#   scripts/ui-test.sh shot FILE           a PNG of the window into FILE (a path inside this checkout)
#   scripts/ui-test.sh screen FILE         a PNG of the whole screen (menus and floating windows too)
#   scripts/ui-test.sh click X Y           a left click at the window's X, Y (also: rclick, dclick, move)
#   scripts/ui-test.sh drag X1 Y1 X2 Y2    press at one place, move, release at the other
#   scripts/ui-test.sh type 'text'         types the text
#   scripts/ui-test.sh key ctrl+shift+p    presses keys (xdotool's names: Return, Escape, ctrl+s, F2 ...)
#   scripts/ui-test.sh wheel X Y up|down N scrolls at X, Y
#   scripts/ui-test.sh restart             the window again (after a rebuild), its kept layout as it was
#   scripts/ui-test.sh sh 'command'        runs a command in the container (cd /tmp/work for the project)
#   scripts/ui-test.sh log                 the window's output
#   scripts/ui-test.sh stop                the container gone
#
# To see that a change left the window as it was: scripts/ui/tour.sh takes a picture at each step of a fixed tour
# (run it on the build before the change and on the one after), and scripts/ui/compare.py compares two tours.
#
# The window sits at the screen's top left, 1280 x 800, so window and screen coordinates are the same.
set -eu
name=code-arena-ui-$(basename "$(pwd)" | tr -c 'a-zA-Z0-9_.\n-' '-')
cmd=${1:-}
[ $# -gt 0 ] && shift

inside() { docker exec -e DISPLAY=:99 "$name" sh -c "$1"; }

# There is no window manager on the virtual screen, so keys go to a window only once it is given the focus: the one
# under the pointer (a floating view's window, a menu), else the main window.
focus='w=$(xdotool getmouselocation --shell | sed -n "s/^WINDOW=//p"); r=$(xdotool search --maxdepth 0 --name "" 2>/dev/null | head -1); \
  [ -z "$w" ] || [ "$w" = "$r" ] && w=$(xdotool search --name "^Code Arena$" | head -1); xdotool windowfocus --sync "$w" 2>/dev/null || true'

open_window() {
  # The window's own process id is kept (a new sh's $$, kept by exec), so restart stops exactly it.
  docker exec -d -e DISPLAY=:99 -w /tmp/work "$name" sh -c 'echo $$ >/tmp/app.pid; exec env SHELL=/bin/bash \
    CODE_ARENA_HOME=/tmp/home ARENA_URL=https://arena.example ARENA_API_KEY=sk-test-only \
    ARENA_GATEWAY_URL=http://127.0.0.1:18733 ARENA_MODEL=qwen NO_PROXY=127.0.0.1 NO_COLOR=1 RUST_BACKTRACE=1 \
    FLOEM_FORCE_TINY_SKIA=1 FONTCONFIG_FILE=/src/scripts/ui/fonts.conf XDG_RUNTIME_DIR=/tmp \
    /src/target/debug/code-arena ide >/tmp/app.log 2>&1'
  i=0
  until inside 'xdotool search --name "^Code Arena$" >/dev/null 2>&1'; do
    i=$((i + 1))
    if [ $i -gt 60 ]; then
      echo "the window did not open; its output:" >&2
      inside 'cat /tmp/app.log' >&2
      exit 1
    fi
    sleep 0.5
  done
  inside 'w=$(xdotool search --name "^Code Arena$" | head -1); xdotool windowmove "$w" 0 0; xdotool windowfocus --sync "$w"' >/dev/null 2>&1 || true
  sleep 1.5
}

case $cmd in
  start)
    docker rm -f "$name" >/dev/null 2>&1 || true
    docker run -d --name "$name" -u "$(id -u):$(id -g)" -v "$(pwd)":/src -w /src code-arena-gui sh /src/scripts/ui/run-in.sh >/dev/null
    until inside 'test -e /tmp/ready'; do sleep 0.2; done
    open_window
    echo "window open in $name"
    ;;
  restart)
    inside 'p=$(cat /tmp/app.pid 2>/dev/null) && kill $p 2>/dev/null; while [ -n "$p" ] && kill -0 $p 2>/dev/null; do sleep 0.1; done' || true
    open_window
    ;;
  shot) inside "python3 /src/scripts/ui/shot.py '/src/$1' 'Code Arena'" ;;
  screen) inside "python3 /src/scripts/ui/shot.py '/src/$1'" ;;
  click) inside "xdotool mousemove $1 $2 click 1" ;;
  rclick) inside "xdotool mousemove $1 $2 click 3" ;;
  dclick) inside "xdotool mousemove $1 $2 click --repeat 2 --delay 80 1" ;;
  move) inside "xdotool mousemove $1 $2" ;;
  drag) inside "xdotool mousemove $1 $2 mousedown 1 sleep 0.1 mousemove $(( ($1 + $3) / 2 )) $(( ($2 + $4) / 2 )) sleep 0.1 mousemove $3 $4 sleep 0.2 mouseup 1" ;;
  type) inside "$focus"; docker exec -e DISPLAY=:99 "$name" xdotool type --delay 20 -- "$1" ;;
  key) inside "$focus"; docker exec -e DISPLAY=:99 "$name" xdotool key --delay 60 "$@" ;;
  wheel)
    b=4
    [ "$3" = down ] && b=5
    inside "xdotool mousemove $1 $2 click --repeat ${4:-3} $b"
    ;;
  sh) inside "$1" ;;
  log) inside 'cat /tmp/app.log' ;;
  stop) docker rm -f "$name" >/dev/null ;;
  *) sed -n '2,28p' "$0"; exit 2 ;;
esac
