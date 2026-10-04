# Eludite's shell integration for zsh (brief 0041): ZDOTDIR points here so zsh reads these files; each runs yours
# from your own ZDOTDIR (or home) first. Nothing in your home folder is changed.
__eludite_zdotdir="$ZDOTDIR"
ZDOTDIR="${ELUDITE_USER_ZDOTDIR:-$HOME}"
[ -r "$ZDOTDIR/.zshenv" ] && . "$ZDOTDIR/.zshenv"
ELUDITE_USER_ZDOTDIR="$ZDOTDIR"
ZDOTDIR="$__eludite_zdotdir"
unset __eludite_zdotdir
