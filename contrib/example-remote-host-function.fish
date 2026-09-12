# Start the reporter alongside an ssh to a host where your chats run, and end it with the shell.
#
# Rename `devbox` to whatever you call that host, keeping the ssh destination in step.
function devbox --wraps ssh --description "ssh to the remote host where chats run"
    # A chat on the far side cannot tell cmux what it is doing: cmux reads the state from a process it
    # can trace into one of its own terminals, so a report sent from the other machine is ignored. The
    # reporter runs here instead, in this terminal, and asks that host what its chat is doing.
    #
    # It is started beside the ssh rather than around it, which is what lets it find that connection
    # and ask the far side about the one pty this terminal holds.
    if set -q CMUX_SURFACE_ID; and test -x ~/agents-tmux/contrib/cmux-remote-agent-status; and test (count $argv) -eq 0
        ~/agents-tmux/contrib/cmux-remote-agent-status devbox >/dev/null 2>&1 &
        set -l reporter $last_pid
        ssh -t devbox
        kill $reporter 2>/dev/null
        return
    end
    ssh devbox $argv
end
