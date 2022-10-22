#!/bin/zsh

# for the bible tells us so
set -ef -o pipefail

function print_err_exit {
    echo "$@" 1>&2
    exit 1
}

function prep_exec {
    # Use zfs allow to operate without sudo
    [[ -n "$( command -v sudo )" ]] || print_err_exit "'sudo' is required to execute 'ounce'.  Please check that 'sudo' is in your path."
    [[ -n "$( command -v httm )" ]] || print_err_exit "'httm' is required to execute 'ounce'.  Please check that 'httm' is in your path."
    [[ -n "$( command -v zfs )" ]] || print_err_exit "'zfs' is required to execute 'ounce'.  Please check that 'zfs' is in your path."
}

function exec_snap {
   # print stderr not stdout
   if [[ "$( sudo -l | grep -c -e 'NOPASSWD' -e 'zfs snapshot *')" -ne 0 ]]; then
      sudo httm --snap="ounceSnapFileMount" "$@" 1>/dev/null
   else
      httm --snap="ounceSnapFileMount" "$@" 1>/dev/null
   fi

   [[ $? -eq 0 ]] || print_err_exit "'ounce' quit with a 'httm' or 'zfs' error."
}

function ounce_of_prevention {
    # do we have commands to execute?
    prep_exec

    # declare our exec vars
    local OUNCE_PROGRAM_NAME
    local FILENAMES_STRING
    local NEEDS_SNAP

    # get inner executable name
    [[ "$1" != "ounce" ]] || print_err_exit "'ounce' being called recursively. Quitting."
    OUNCE_PROGRAM_NAME="$( command -v "$1" )"
    shift
    [[ -x "$OUNCE_PROGRAM_NAME" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."

    # loop through our shell arguments
    for a; do
        # is the argument a file/directory that exists?
        [[ ! -e "$a" ]] || FILENAMES_STRING+=( "$(printf "$a\0")" )
    done

    # check if filenames array is not empty
    if [[ -n FILENAMES_STRING  ]]; then
      # httm will dynamically determine the location of
      # the file's ZFS dataset and snapshot that mount
      # check whether to take snap - do we have a snap of the live file?
      #
      # leave FILENAMES_STRING unquoted!!!
      NEEDS_SNAP="$( httm --last-snap=no-ditto --not-so-pretty $FILENAMES_STRING | cut -f1 -d: )"
      [[ -z "$NEEDS_SNAP" ]] || exec_snap "$NEEDS_SNAP"
    fi

    # execute original arguments
    "$OUNCE_PROGRAM_NAME" "$@"
}

ounce_of_prevention "$@"