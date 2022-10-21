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

function exec_httm {
   # print stderr not stdout
   httm --snap "$FILENAMES_STRING" 1> /dev/null
   if [[ $? -ne "0" ]]; then
      print_err_exit "'ounce' quit with a 'httm' or 'zfs' error."
   fi

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
	  [[ -n "$OUNCE_PROGRAM_NAME" ]] || print_err_exit "'zfs' is required to execute 'ounce'.  Please check that 'zfs' is in your path."
          [[ -x "$OUNCE_PROGRAM_NAME" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."
          continue
        fi

        # is the argument a file?
        if [[ -f "$a" ]]; then
           local LIVE_FILE="$a"
        else
           continue
        fi

        # get last snap version of the live file?
        local LAST_SNAP="$(httm --last-snap=ditto "$LIVE_FILE")"

        # check whether to take snap - do we have a snap of the live file?
        if [[ -z "$LAST_SNAP" ]]; then
           FILENAMES_ARRAY+=($( echo "$LIVE_FILE" ))
        fi
    done

    # check if filenames array is not empty
    if [[ ${FILENAMES_ARRAY[@]} ]]; then
      # httm will dynamically determine the location of
      # the file's ZFS dataset and snapshot that mount
      local FILENAMES_STRING="${FILENAMES_ARRAY[@]}"
      exec_httm "$FILENAMES_STRING"
    fi

    "$@"
}

ounce_of_prevention "$@"