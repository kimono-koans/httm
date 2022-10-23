# HTTM ZSH Widgets

# ALT-d - Dynamically snap PWD dataset
__httm-snapshot() {

  httm --snap "$1" 2>/dev/null; [[ $? == 0 ]] || \
  sudo httm --snap "$1"; [[ $? == 0 ]] || \
  echo "httm snapshot widget quit with a snapshot error.  Check you have the correct permissions to snapshot."; return 1

  local ret=$?
  zle reset-prompt
  return $ret

}

httm-snapshot-pwd-widget() {

  echo
  __httm-snapshot "$PWD"

  local ret=$?
  zle reset-prompt
  return $ret

}
zle     -N      httm-snapshot-pwd-widget
bindkey '\ed'   httm-snapshot-pwd-widget

# ALT-m - browse for ZFS snapshots interactively
httm-lookup-widget() {

  echo
  command httm -r -R

  local ret=$?
  zle reset-prompt
  return $ret

}
zle     -N      httm-lookup-widget
bindkey '\em'   httm-lookup-widget

# ALT-s - select files on ZFS snapshots interactively
__httm-select() {

  command httm -s -R | \
  while read item; do
    echo -n "${item}"
  done

  local ret=$?
  echo
  return $ret

}

httm-select-widget() {
  LBUFFER="${LBUFFER}$(__httm-select)"
  local ret=$?
  zle reset-prompt
  return $ret
}
zle     -N      httm-select-widget
bindkey '\es'   httm-select-widget