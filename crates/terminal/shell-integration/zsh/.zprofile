# Eludite's shell integration for zsh (brief 0041): your .zprofile, for a login shell.
__eludite_zdotdir="$ZDOTDIR"
ZDOTDIR="$ELUDITE_USER_ZDOTDIR"
[ -r "$ZDOTDIR/.zprofile" ] && . "$ZDOTDIR/.zprofile"
ZDOTDIR="$__eludite_zdotdir"
unset __eludite_zdotdir
