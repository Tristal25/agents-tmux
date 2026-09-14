# Plain ssh is all a terminal needs: a single reporter on this machine gives every row its state.
#
# Rename `devbox` to whatever you call that host, keeping the ssh destination in step.
function devbox --wraps ssh --description "ssh to the remote host where chats run"
    ssh -t devbox $argv
end

# `cmux-remote-agent-rows devbox` is what fills the rows, and it belongs somewhere that keeps it up for
# as long as cmux runs, a launch agent being the obvious place. One process covers every terminal open to
# that host, so nothing has to be started or stopped alongside a connection.
#
# `cmux-remote-agent-status` is the other way round: it lives inside one terminal and reports cmux's own
# agent lifecycle for that surface, which is what draws cmux's spinner. It buys that at a price. cmux
# holds a lifecycle until something ends it, so a reporter that stops or hangs leaves the row saying
# `Running` until you notice, where the every-row reporter re-reads the truth every few seconds. Start it
# beside the ssh only where the spinner is worth that:
#
#     if set -q CMUX_SURFACE_ID
#         ~/agents-tmux/contrib/cmux-remote-agent-status devbox >/dev/null 2>&1 &
#         set -l reporter $last_pid
#         ssh -t devbox
#         kill $reporter 2>/dev/null
#         return
#     end
