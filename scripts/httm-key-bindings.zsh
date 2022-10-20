# HTTM ZSH Widgets

# ALT-d - dynamically snap PWD dataset
httm-snapshot-pwd-widget() {

  echo
  command sudo httm --snap "$PWD"

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