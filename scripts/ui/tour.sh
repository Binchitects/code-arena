#!/bin/sh
# A fixed tour of the native window from a fresh start, a picture at each step: the agent (an answer, an approval,
# a queued message, a plan, the pickers), the editor (a file, find, a change, the close question, save, the palette,
# Quick Open, the themes), the menus, the views, the terminal's keys, the keys in the agent's box, a view floated.
# To see that a change left the window as it was, the tour is run before and after it and the pictures compared.
#
#   scripts/ui-test.sh start && sh scripts/ui/tour.sh before      # on the build before the change
#   scripts/ui-test.sh start && sh scripts/ui/tour.sh after       # on the build after it (start again: a fresh project)
#   python3 scripts/ui/compare.py target/ui/before target/ui/after 901,36,1280,667
#
# The pictures go to target/ui/NAME/NN-step.png. Run it from the checkout's root. What moves by itself differs
# from one run to the next and says nothing: a caret that blinks (a few pixels), and how far the thread is scrolled
# when the plan's bar comes under it (so the thread's place, the rectangle above, is left out of the comparison).
set -eu
p=$1
u=scripts/ui-test.sh
mkdir -p "target/ui/$p"
n=0
s() { n=$((n + 1)); sleep 0.7; $u shot "target/ui/$p/$(printf %02d $n)-$1.png" >/dev/null; }
esc() { $u key Escape; sleep 0.2; }
s start
# The agent. With no thread yet its box stands in the middle of the view; under a thread, at the bottom.
$u click 1000 300; $u type 'hello there'; $u key Return; sleep 3; s agent-answer
$u type 'please run it'; $u key Return; sleep 3; s agent-ask
$u type 'queued one'; $u key Return; s agent-queued
$u click 961 563; sleep 6; s agent-allowed
$u click 1000 719; $u type 'make a plan'; $u key Return; sleep 8; s agent-plan
$u click 941 747; s pick-mode; esc
$u click 997 747; s pick-model; esc
$u click 1093 747; s pick-thinking; esc
# The editor.
$u click 104 169; s readme-open
$u click 600 300; $u key ctrl+f; $u type 'a'; s find; esc
$u click 600 300; $u type 'x'; s dirty
$u key ctrl+w; s close-question; esc
$u key ctrl+s; s saved
$u key ctrl+shift+p; s palette
$u type 'view'; s palette-view; esc
$u key ctrl+p; $u type 'not'; s quick-open
$u key Return; s notes-open
$u key ctrl+k ctrl+t; s themes; esc
$u key ctrl+j; s panel-hidden
$u key ctrl+j; s panel-shown
$u key ctrl+b; s side-hidden
$u key ctrl+b; s side-shown
# The menus.
for x in 53 93 152 214 255 293 349 407; do $u click $x 17; s menu-$x; esc; done
$u rclick 940 52; s view-menu; esc
$u rclick 104 147; s explorer-menu; esc
# The views.
$u click 23 106; s search-view
$u click 23 154; s changes-view
$u click 23 202; s outline-view
$u click 23 58; s explorer-view
# The terminal has the keys.
$u click 600 620; $u type 'echo hi'; $u key Return; s terminal
$u key ctrl+p; s terminal-ctrl-p; esc
$u click 600 620; $u key ctrl+j; s terminal-ctrl-j
$u key ctrl+j; s terminal-ctrl-j-again
# The agent's box has the keys.
$u click 1000 719; $u key ctrl+b; s box-ctrl-b
$u key ctrl+b; s box-ctrl-b-again
$u key ctrl+shift+p; s box-palette; esc
$u key ctrl+w; s box-ctrl-w
# A view floated: a window of its own, so the whole screen is taken.
$u key ctrl+shift+p; $u type 'float outline'; $u key Return; sleep 1.5
n=$((n + 1)); $u screen "target/ui/$p/$(printf %02d $n)-floated.png" >/dev/null
$u log | grep -i panicked && exit 1
echo "$n pictures in target/ui/$p"
