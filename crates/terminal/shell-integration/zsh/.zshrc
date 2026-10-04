# Eludite's shell integration for zsh (brief 0041): your .zshrc, then OSC 133 marks around prompts and commands
# (A prompt start, B command start, C output start, D;<exit code> command end) and the folder (OSC 7). ZDOTDIR is
# yours again afterwards.
ZDOTDIR="$ELUDITE_USER_ZDOTDIR"
[ -r "$ZDOTDIR/.zshrc" ] && . "$ZDOTDIR/.zshrc"
if [[ -z "$__ELUDITE_INTEGRATED" && -o interactive ]]; then
    __ELUDITE_INTEGRATED=1
    __eludite_ran=
    __eludite_precmd() {
        local s=$?
        if [[ -n "$__eludite_ran" ]]; then
            printf '\033]133;D;%s\007' "$s"
            __eludite_ran=
        fi
        printf '\033]133;A\007\033]7;file://%s%s\007' "${HOST:-}" "$PWD"
        [[ "$PS1" == *'133;B'* ]] || PS1="$PS1"$'%{\033]133;B\007%}'
    }
    __eludite_preexec() {
        __eludite_ran=1
        printf '\033]133;C\007'
    }
    autoload -Uz add-zsh-hook
    add-zsh-hook precmd __eludite_precmd
    add-zsh-hook preexec __eludite_preexec
fi
