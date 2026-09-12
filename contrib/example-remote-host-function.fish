# Start the reporter alongside an ssh to a host where your chats run, and end it with the shell.
#
# Rename `devbox` to whatever you call that host, keeping the ssh destination in step.
function devbox --wraps ssh --description "ssh to the remote host where chats run"
    # A chat on the far side cannot tell cmux what it is doing: cmux accepts agent reports only over
    # a socket connection from a process it started itself, so anything sent from the other host is
    # refused. The reporter runs here instead, inside this terminal, and asks that host what its chat
    # is doing.
    #
    # It has to know which chat, and the surface id travels in the remote shell's environment to say
    # so exactly. Carrying it on the command line rather than through SendEnv leaves the server's
    # configuration untouched; the id is a handle for a sidebar row, not a secret.
    if set -q CMUX_SURFACE_ID; and test -x ~/agents-tmux/contrib/cmux-remote-agent-status; and test (count $argv) -eq 0
        ~/agents-tmux/contrib/cmux-remote-agent-status devbox >/dev/null 2>&1 &
        set -l reporter $last_pid
        ssh -t devbox "CMUX_SURFACE_ID=$CMUX_SURFACE_ID exec \$SHELL -l"
        kill $reporter 2>/dev/null
        return
    end
    ssh devbox $argv
end
