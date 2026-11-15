#!/bin/sh
# Copyright (c) 2026 Julius ML
# MIT License
#
# Run pcx for a post-cortex plugin (Cursor, Gemini CLI). The plugin ships no
# binary: hooks and the stdio MCP server go through the pcx the user
# installed (brew, cargo, release binary). GUI apps start these with a
# minimal PATH, so the usual install locations are tried as well.
#
#   pcx.sh hook <client> <event>   a client hook: `pcx hook <client> <event>`
#                                  with the payload on stdin. Silent (exit 0)
#                                  when pcx is missing, when `pcx setup`
#                                  already put the same hooks in the client's
#                                  user config, or for a capture event while
#                                  capture is off for the client.
#   pcx.sh mcp [args]              the stdio MCP server: `pcx mcp [args]`
#                                  (the active profile, cloud by default).
#
# This file is the source; integrations/sync.py copies it into the Gemini CLI
# extension.

find_pcx() {
    if [ -n "${PCX_BIN:-}" ] && [ -x "$PCX_BIN" ]; then
        printf '%s\n' "$PCX_BIN"
        return 0
    fi
    if command -v pcx >/dev/null 2>&1; then
        command -v pcx
        return 0
    fi
    # PCX_FALLBACKS (space-separated, may be empty) replaces the list in tests.
    if [ -n "${PCX_FALLBACKS+set}" ]; then
        # shellcheck disable=SC2086
        set -- $PCX_FALLBACKS
    else
        set -- "$HOME/.cargo/bin/pcx" \
            /opt/homebrew/bin/pcx \
            /usr/local/bin/pcx \
            /home/linuxbrew/.linuxbrew/bin/pcx \
            "$HOME/.local/bin/pcx"
    fi
    for candidate in "$@"; do
        if [ -x "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

config_dir() {
    if [ -n "${PC_CONFIG_DIR:-}" ]; then
        printf '%s\n' "$PC_CONFIG_DIR"
    else
        printf '%s\n' "$HOME/.post-cortex"
    fi
}

# The client's user-level file where `pcx setup` (global mode) writes its
# hooks, and the author name the client writes as.
user_hooks_file() {
    case "$1" in
        cursor) printf '%s\n' "$HOME/.cursor/hooks.json" ;;
        gemini) printf '%s\n' "${GEMINI_CLI_HOME:-$HOME}/.gemini/settings.json" ;;
    esac
}

author() {
    case "$1" in
        gemini) printf '%s\n' gemini-cli ;;
        *) printf '%s\n' "$1" ;;
    esac
}

# Capture hooks (the conversation turns) run only with
# hosts.<author>.capture true in config.json, like the hooks `pcx setup
# --capture` writes.
is_capture_event() {
    case "$1:$2" in
        cursor:beforeSubmitPrompt | cursor:afterAgentResponse | gemini:capture) return 0 ;;
    esac
    return 1
}

capture_on() {
    config="$(config_dir)/config.json"
    [ -f "$config" ] || return 1
    # hosts.<author> holds only scalars (enabled, capture, workspace), so the
    # object ends at the first closing brace.
    tr -d ' \t\r\n' <"$config" | grep -q "\"$(author "$1")\":{[^}]*\"capture\":true"
}

# Whether `pcx setup` put pcx hooks for the client in its user config: they
# run there already, so the plugin's would run them twice.
setup_hooks_present() {
    file=$(user_hooks_file "$1")
    [ -n "$file" ] && [ -f "$file" ] && grep -Eq "pcx(\\\\\")? hook $1 " "$file"
}

skip_hook() {
    cat >/dev/null
    # Cursor waits for an answer to beforeSubmitPrompt: let the prompt go on.
    if [ "$1:$2" = "cursor:beforeSubmitPrompt" ]; then
        printf '%s\n' '{"continue": true}'
    fi
    exit 0
}

case "${1:-}" in
    hook)
        client=${2:-}
        event=${3:-}
        [ -n "$client" ] && [ -n "$event" ] || skip_hook "" ""
        pcx=$(find_pcx) || skip_hook "$client" "$event"
        setup_hooks_present "$client" && skip_hook "$client" "$event"
        if is_capture_event "$client" "$event" && ! capture_on "$client"; then
            skip_hook "$client" "$event"
        fi
        exec "$pcx" hook "$client" "$event"
        ;;
    mcp)
        shift
        if ! pcx=$(find_pcx); then
            echo "post-cortex: pcx is not installed. Install it (brew install julymetodiev/tap/post-cortex, or cargo install post-cortex-daemon), run pcx login, then reload the MCP server." >&2
            exit 127
        fi
        exec "$pcx" mcp "$@"
        ;;
    *)
        echo "usage: pcx.sh hook <client> <event> | pcx.sh mcp [args]" >&2
        exit 2
        ;;
esac
