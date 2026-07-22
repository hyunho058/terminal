# mac-terminal shell integration (generated — do not edit).
#
# We point the child zsh at a private ZDOTDIR so we can register OSC 133 hooks.
# To avoid clobbering the user's setup (spec D18) this file chains the user's own
# .zshenv. It runs for EVERY zsh started under our ZDOTDIR (interactive or not),
# which is why the user's .zshenv (PATH, etc.) must be sourced here — subshells
# inherit our exported ZDOTDIR and would otherwise skip it.
#
# $MAC_TERMINAL_USER_ZDOTDIR is the user's real ZDOTDIR (or $HOME), captured by
# the app before it overrode ZDOTDIR.
: ${MAC_TERMINAL_USER_ZDOTDIR:=$HOME}
[[ -r "$MAC_TERMINAL_USER_ZDOTDIR/.zshenv" ]] && source "$MAC_TERMINAL_USER_ZDOTDIR/.zshenv"
