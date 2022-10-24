# HTTM ZSH Widgets

# ALT-d - Dynamically snap selected files's dataset
__httm-snapshot() {

  command httm --snap "$1" 2>/dev/null || \
  command sudo httm --snap "$1" || \
  echo "httm snapshot widget quit with a snapshot error.  Check you have the correct permissions to snapshot."; return 1

  local ret=$?
  echo
  return $ret
}

httm-snapshot-widget() {
  local filename

  # requires an fzf function sourced to work properly
  if [[ $( type '__fsel' 2>/dev/null | grep -q 'function' ) -eq 0 ]]; then
    # need canonical path
    filename="$(realpath $(__fsel))"
    if [[ -z "$filename" ]]; then
      return 0
    fi
  else
    filename="$PWD"
  fi

  __httm-snapshot "$filename"

  local ret=$?
  zle reset-prompt
  return $ret

}
zle     -N      httm-snapshot-widget
bindkey '\ed'   httm-snapshot-widget

# ALT-i - browse for ZFS snapshots interactively
httm-lookup-widget() {

  echo
  command httm -r -R

  local ret=$?
  zle reset-prompt
  return $ret

}
zle     -N		httm-lookup-widget
bindkey '\em'	httm-lookup-widget

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