# Crewterm shell integration (generated — do not edit).
#
# Chains the user's interactive config first (spec D18), THEN adds OSC 133
# command markers add-only. We never touch PROMPT/PS1, so oh-my-zsh / starship /
# powerlevel10k prompts keep working unchanged. If anything here fails the shell
# still runs — the completion/error signals just go quiet (spec D13).

: ${CREWTERM_USER_ZDOTDIR:=$HOME}
[[ -r "$CREWTERM_USER_ZDOTDIR/.zshrc" ]]  && source "$CREWTERM_USER_ZDOTDIR/.zshrc"
[[ -r "$CREWTERM_USER_ZDOTDIR/.zlogin" ]] && source "$CREWTERM_USER_ZDOTDIR/.zlogin"

# --- OSC 133 command markers (wire contract mirrored in parser.rs::osc_dispatch) ---
#   command start: ESC ] 133 ; C BEL
#   command end:   ESC ] 133 ; D ; <exit_code> ; <elapsed_ms> BEL
# elapsed_ms is measured shell-side from EPOCHREALTIME so it survives laptop
# sleep without an app-side clock (spec D19).
zmodload zsh/datetime 2>/dev/null
autoload -Uz add-zsh-hook 2>/dev/null

__mt_preexec() {
  # A real command is about to run: stamp its start and mark it.
  __mt_cmd_start=$EPOCHREALTIME
  printf '\033]133;C\a'
}

__mt_precmd() {
  # Runs before every prompt. Emit a command-end ONLY when a command actually
  # ran (i.e. preexec stamped a start) — a bare Enter fires no preexec, so no
  # spurious D marker is emitted at an empty prompt or at shell startup.
  local ret=$?
  if [[ -n "${__mt_cmd_start-}" ]]; then
    local ms
    ms=$(printf '%.0f' $(( (EPOCHREALTIME - __mt_cmd_start) * 1000 )))
    unset __mt_cmd_start
    printf '\033]133;D;%d;%d\a' "$ret" "$ms"
  fi
}

# add-zsh-hook is the coexistence-safe way to append hooks (never clobbers any
# preexec/precmd the user's config already installed).
if (( ${+functions[add-zsh-hook]} )); then
  add-zsh-hook preexec __mt_preexec
  add-zsh-hook precmd __mt_precmd
fi
