#!/bin/zsh

## for the bible tells us so
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
    local -a FILENAMES_ARRAY

    # get inner executable name
    [[ "$1" != "ounce" ]] || print_err_exit "'ounce' being called recursively. Quitting."
    OUNCE_PROGRAM_NAME="$( command -v "$1" )"
    shift
    [[ -x "$OUNCE_PROGRAM_NAME" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."

    # loop through our shell arguments
    for a; do
        # is the argument a file/directory that exists?
        [[ ! -e "$a" ]] || FILENAMES_ARRAY+=( "$a" )
    done

    # check if filenames array is not empty
    if [[ ${FILENAMES_ARRAY[@]}  ]]; then
      # httm will dynamically determine the location of
      # the file's ZFS dataset and snapshot that mount
      # check whether to take snap - do we have a snap of the live file?
      local FILENAMES_STRING="${FILENAMES_ARRAY[*]}"
      # this is performance oriented sleaze.
      #
      # if any files need snapshots, then all get snapshots
      # overhead of starting up httm for each file probably(?)
      # negates any benefit of snapshotting fewer datasets
      local NEEDS_SNAP="$( httm --last-snap=no-ditto --not-so-pretty "$FILENAMES_STRING" 2>/dev/null | cut -f1 -d: | uniq )"
      [[ -z "$NEEDS_SNAP" ]] || exec_snap "$NEEDS_SNAP"
    fi

    # execute original arguments
    "$OUNCE_PROGRAM_NAME" "$@"
}

ounce_of_prevention "$@"