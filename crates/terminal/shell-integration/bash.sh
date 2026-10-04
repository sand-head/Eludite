# Eludite's shell integration for bash (brief 0041). Bash reads this file through `--rcfile` in place of ~/.bashrc;
# nothing in your home folder is changed. It first runs what bash would have run (~/.bashrc, or the login files when
# Eludite asked for a login shell), then marks each prompt and command with OSC 133 (A prompt start, B command start,
# C output start, D;<exit code> command end) and reports the current folder with OSC 7, which the terminal reads and
# never shows.
if [ -n "$ELUDITE_SHELL_LOGIN" ]; then
    unset ELUDITE_SHELL_LOGIN
    [ -r /etc/profile ] && . /etc/profile
    for __eludite_f in ~/.bash_profile ~/.bash_login ~/.profile; do
        if [ -r "$__eludite_f" ]; then . "$__eludite_f"; break; fi
    done
    unset __eludite_f
else
    [ -r ~/.bashrc ] && . ~/.bashrc
fi

if [ -z "$__ELUDITE_INTEGRATED" ] && [[ $- == *i* ]]; then
    __ELUDITE_INTEGRATED=1
    __eludite_ran=
    # First in PROMPT_COMMAND: the last command's end, its exit code kept for what follows.
    __eludite_end() {
        __eludite_status=$?
        if [ -n "$__eludite_ran" ]; then
            printf '\033]133;D;%s\007' "$__eludite_status"
            __eludite_ran=
        fi
        return $__eludite_status
    }
    # Last in PROMPT_COMMAND, just before bash prints PS1: the prompt's start and the folder; PS1 ends with B.
    __eludite_start() {
        printf '\033]133;A\007\033]7;file://%s%s\007' "${HOSTNAME:-}" "$PWD"
        case "$PS1" in
            *'133;B'*) ;;
            *) PS1="$PS1"'\[\033]133;B\007\]' ;;
        esac
        return $__eludite_status
    }
    # PS0 prints after a command is read, before it runs: C, and (an arithmetic subscript that expands to nothing)
    # the note that a command ran.
    PS0="${PS0}"'${__eludite_none[__eludite_ran=1]}\033]133;C\007'
    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
        PROMPT_COMMAND=(__eludite_end "${PROMPT_COMMAND[@]}" __eludite_start)
    else
        PROMPT_COMMAND="__eludite_end${PROMPT_COMMAND:+;$PROMPT_COMMAND};__eludite_start"
    fi
fi
