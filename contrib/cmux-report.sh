#!/bin/bash
# Tell the terminal that owns this session what the chat is doing, from a chat running on another
# host.
#
#   cmux-report.sh notify        a desktop notification, from the hook payload on stdin
#   cmux-report.sh busy          the chat started working
#   cmux-report.sh done          the chat is waiting again
#
# Everything travels as an escape sequence written to the pane's tty, which is what makes this work
# across an ssh connection with nothing to configure and nothing to keep alive: the bytes reach
# whichever terminal is attached at that moment, so the report follows a session between terminals.
# A notification is OSC 9; working state is OSC 9;4, the progress sequence, indeterminate because a
# turn has no percentage to report.
#
# tmux would consume either sequence, so inside tmux they travel wrapped in a passthrough, which
# needs `allow-passthrough on`. Each escape inside that wrapper is doubled, per tmux's rule, and the
# sequence ends on a BEL so its introducer is the only one to double.
set -u

MODE="${1:-notify}"

# A hook is spawned without a controlling terminal of its own, so `tty` and its own `ps` entry both
# come back empty. The agent that spawned it owns the pane, so the terminal is the first one found
# walking up the ancestry. Silence is right when there is none: a headless or piped run has no
# terminal to tell.
find_tty() {
    local pid=$$ candidate
    for _ in 1 2 3 4 5 6; do
        pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
        [ -n "$pid" ] || return 1
        candidate=$(ps -o tty= -p "$pid" 2>/dev/null | tr -d ' ')
        case "$candidate" in
            pts/* | tty*) printf '/dev/%s' "$candidate"; return 0 ;;
        esac
    done
    return 1
}

TTY=$(find_tty) || exit 0
[ -w "$TTY" ] || exit 0

emit() {
    if [ -n "${TMUX:-}" ]; then
        printf '\033Ptmux;\033\033%s\007\033\\' "$1" > "$TTY"
    else
        printf '\033%s\007' "$1" > "$TTY"
    fi
}

case "$MODE" in
    busy) emit ']9;4;3' ;;
    done) emit ']9;4;0' ;;
    notify)
        PAYLOAD=$(cat 2>/dev/null)
        # `message` is what the Notification event carries; the others describe themselves.
        BODY=$(printf '%s' "$PAYLOAD" | jq -r '.message // .reason // empty' 2>/dev/null)
        [ -n "$BODY" ] || BODY="Claude"
        # The chat's own directory says which one this is when several are open.
        CWD=$(printf '%s' "$PAYLOAD" | jq -r '.cwd // empty' 2>/dev/null)
        [ -n "$CWD" ] && BODY="$BODY  ($(basename "$CWD"))"
        # The body is model-authored text, and an escape or BEL inside it would end the sequence
        # early and leave the rest to be read as the terminal's own control bytes.
        BODY=$(printf '%s' "$BODY" | tr -d '\000-\037' | cut -c1-200)
        emit "]9;$BODY"
        ;;
    *)
        printf 'usage: %s notify|busy|done\n' "${0##*/}" >&2
        exit 2
        ;;
esac
exit 0
