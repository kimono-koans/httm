#!/bin/zsh

# for the bible tells us so
set -ef -o pipefail

function print_err_exit {
    echo "$@" 1>&2
    exit 1
}

function prep_exec {
    [[ -n "$( command -v httm )" ]] || print_err_exit "'httm' is required to execute 'ounce'.  Please check that 'httm' is in your path."
    # Use zfs allow to operate without sudo
    # [[ -n "$( command -v sudo )" ]] || print_err_exit "'sudo' is required to execute 'ounce'.  Please check that 'sudo' is in your path."
    [[ -n "$( command -v zfs )" ]] || print_err_exit "'zfs' is required to execute 'ounce'.  Please check that 'zfs' is in your path."
}

function exec_snap {
   # print stderr not stdout
   httm --snap "$@" 1> /dev/null
   [[ $? -eq "0" ]] || print_err_exit "'ounce' quit with a 'httm' or 'zfs' error."
}

function ounce_of_prevention {
    # do we have commands to execute?
    prep_exec

    # declare our exec vars
    local OUNCE_PROGRAM_NAME

    # declare an array, convert to string later
    # this allows us to exec zfs snapshot once
    local -a FILENAMES_ARRAY

    # loop through our shell arguments
    for a; do
        # set ounce params
        if [[ -z "$OUNCE_PROGRAM_NAME" ]]; then
          OUNCE_PROGRAM_NAME="$( command -v $a )"
          [[ -x "$OUNCE_PROGRAM_NAME" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."
          continue
        fi

        # is the argument a file?
        if [[ -f "$a" ]]; then
           # check whether to take snap - do we have a snap of the live file?
           FILENAMES_ARRAY+=( "$a" )
        fi

    done

    # check if filenames array is not empty
    if [[ ${FILENAMES_ARRAY[@]} ]]; then
      # httm will dynamically determine the location of
      # the file's ZFS dataset and snapshot that mount
      # check whether to take snap - do we have a snap of the live file?
      local FILENAMES_STRING="${FILENAMES_ARRAY[@]}"
      local NEEDS_SNAP="$(httm --last-snap=ditto "$FILENAMES_STRING")"

      exec_snap "$NEEDS_SNAP"
    fi

    "$@"
}

ounce_of_prevention "$@"