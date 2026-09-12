#!/bin/bash
# Raise a desktop notification on the terminal that owns this session, from a chat running on
# another host.
#
# The terminal reads notifications out of the byte stream (OSC 9), so a sequence written to the
# pane's tty crosses the ssh connection and reaches the app drawing it. tmux would otherwise
# consume the sequence, so inside tmux it travels wrapped in a passthrough DCS, which needs
# `allow-passthrough on`. Any `\033` inside that wrapper has to be doubled, per tmux's rule, and
# the sequence ends on a BEL so the only escape left to double is its introducer.
#
# Reads the hook payload on stdin and takes the title from `$1`, so one script serves several hook
# events. Silence is the correct behaviour when no tty is attached (a headless or piped run).
set -u

TITLE="${1:-Claude}"
PAYLOAD=$(cat 2>/dev/null)

# `message` is what the Notification event carries; the others describe themselves.
BODY=$(printf '%s' "$PAYLOAD" | jq -r '.message // .reason // empty' 2>/dev/null)
[ -n "$BODY" ] || BODY="$TITLE"

# The session's own directory names which chat this is when several are open.
CWD=$(printf '%s' "$PAYLOAD" | jq -r '.cwd // empty' 2>/dev/null)
[ -n "$CWD" ] && BODY="$BODY  ($(basename "$CWD"))"

# A hook is spawned without a controlling terminal of its own, so `tty` and its own `ps` entry both
# come back empty. The agent that spawned it owns the pane, so the terminal is the first one found
# walking up the ancestry.
TTY=""
pid=$$
for _ in 1 2 3 4 5 6; do
    pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
    [ -n "$pid" ] || break
    candidate=$(ps -o tty= -p "$pid" 2>/dev/null | tr -d ' ')
    case "$candidate" in
        pts/* | tty*) TTY="/dev/$candidate"; break ;;
    esac
done
[ -n "$TTY" ] && [ -w "$TTY" ] || exit 0

if [ -n "${TMUX:-}" ]; then
    printf '\033Ptmux;\033\033]9;%s\007\033\\' "$BODY" > "$TTY"
else
    printf '\033]9;%s\007' "$BODY" > "$TTY"
fi
exit 0
