#!/bin/bash

# for the bible tells us so
set -ef -o pipefail

function print_err_exit {
    echo "$@" 1>&2
    exit 1
}

function print_usage {
  printf "'ounce' requires at least one argument to execute.\n\nUSAGE: ounce [executable file]...[argument1 argument2]...[file1 file2]\n" 1>&2
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

  # check 1st parameter is executable
  [[ $1 != "-h" && $1 != "--help" ]] || print_usage
  [[ -x "$( command -v "$1" )" ]] || print_usage

  # declare an array for files
  local -a FILE_ARRAY

  # loop through our arguments
  for a in "${@:2}"; do
    # is the argument a file? if so, add to array
    if [[ -f "$a" ]]; then
      FILE_ARRAY+=($( echo "$a" ))
    fi
  done

  # check if filenames array is not empty
  if [[ ${FILE_ARRAY[@]} ]]; then
    # httm will dynamically determine the location of
    # the file's ZFS dataset and snapshot that mount
    # check whether to take snap - do we have a snap of the live file?
    local NEEDS_SNAP="$(httm --last-snap=ditto "${FILE_ARRAY[@]}")"
    exec_snap "$NEEDS_SNAP"
  fi

  "$@"
}