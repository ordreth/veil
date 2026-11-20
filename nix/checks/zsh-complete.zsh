#!/usr/bin/env zsh
# Prints, one a line, what zsh offers to complete LINE with the completion
# functions in DIR: an interactive zsh in a pseudo-terminal (zpty) is given
# LINE and a Tab, with `compadd` wrapped so that it prints what it adds.
#
# Usage: zsh-complete.zsh DIR LINE
emulate -L zsh
setopt err_exit
dir=${1:?usage: zsh-complete.zsh DIR LINE}
line=${2:?usage: zsh-complete.zsh DIR LINE}

zmodload zsh/zpty
zpty completer zsh -f -i
# Only DIR and zsh's own functions: not a _veil installed on the machine.
zpty -w completer "fpath=(${(q)dir} \${(M)fpath:#*/\$ZSH_VERSION/functions})"
zpty -w completer 'autoload -U compinit && compinit -u -D'
# Every match added goes out as `MATCH:<word>`; nothing is listed or
# inserted, so the output is only these lines.
zpty -w completer 'compadd () {
  local -a __matches
  builtin compadd -O __matches "$@"
  print -r -n -- ${(pj:\n:)${(@)__matches/#/MATCH:}} $'"'"'\n'"'"'
}'
zpty -w completer 'zstyle ":completion:*" list-prompt ""; zstyle ":completion:*" select-prompt ""'
zpty -w completer "PROMPT=''; print DONE-SETUP"
zpty -r -m completer setup '*DONE-SETUP*' || true
zpty -w -n completer "${line}"$'\t'
# The end of the completion: a marker printed by the next command line.
zpty -w -n completer $'\C-u'"print DONE-COMPLETE"$'\n'
zpty -r -m completer output '*DONE-COMPLETE*' || true
zpty -d completer
print -r -- ${(F)${(M)${(f)output//$'\r'/}:#*MATCH:*}} | sed -n 's/.*MATCH:\([^ ]*\).*/\1/p' | sort -u
