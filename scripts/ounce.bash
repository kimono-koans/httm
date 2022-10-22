#!/bin/bash

# for the bible tells us so
set -ef -o pipefail

function print_err_exit {
    printf "Error: $*" 1>&2
    exit 1
}

function give_priv {
    local user_name
    user_name="$( whoami )"
    [[ "$user_name" != "root" ]] || print_err_exit "'ounce' must be executed as an unprivileged user to obtain their true user name.  You will be prompted when additional privileges are needed.  Quitting."

    for p in $( sudo zpool list -o name | grep -v -e "NAME" ); do
        sudo zfs allow "$user_name" mount,snapshot "$p" || print_err_exit "'ounce' could not obtain privileges on $p.  Quitting."
    done

    exit 0
}

function print_usage {
    ounce="\e[31mounce\e[0m"
    httm="\e[31mhttm\e[0m"

    printf "\
'$ounce' is a wrapper program that allows '$httm' to take snapshots of files you open with other programs at the command line.

USAGE:
	ounce [target executable] [argument1 argument2...]
	ounce --suffix [target executable] [argument1 argument2...]
 	ounce --give-priv

OPTIONS:
	--suffix:
		You may specify a special suffix to use for the snapshots you take.
		See the $httm help, specifically \"httm --snap\", for additional information

	--give-priv:
		To use $ounce you will need privileges to ZFS snapshot ZFS datasets.
		The prefered sceme is via zfs-allow.  Executing --give-priv as a unprivileged user
		will give the current user snapshot privileges on all imported pools.
\n" 1>&2
    exit 1
}

function prep_exec {
    # Use zfs allow to operate without sudo
    [[ -n "$( command -v httm )" ]] || print_err_exit "'httm' is required to execute 'ounce'.  Please check that 'httm' is in your path."
    [[ -n "$( command -v zfs )" ]] || print_err_exit "'zfs' is required to execute 'ounce'.  Please check that 'zfs' is in your path."
    [[ -n "$( command -v sudo )" ]] || print_err_exit "'sudo' is required to execute 'ounce'.  Please check that 'sudo' is in your path."
}

function exec_snap {
   # print stderr not stdout
   if [[ "$( sudo -l | grep -c -e 'NOPASSWD' -e 'zfs snapshot')" -ne 0 ]]; then
      sudo httm --snap="$2" "$1" 1>/dev/null || print_err_exit "'ounce' quit with a 'httm' or 'zfs' error."
   else
      httm --snap="$2" "$1" 1>/dev/null || print_err_exit "'ounce' quit with a 'httm' or 'zfs' error."
   fi
}

function needs_snap {
    local uncut_res
    uncut_res="$( httm --last-snap=no-ditto --not-so-pretty "$@" )"
    [[ $? -eq 0 ]] || print_err_exit "'ounce' quit with a 'httm' error."
    cut -f1 -d: <<< "$uncut_res"
}

function ounce_of_prevention {
    # do we have commands to execute?
    prep_exec

    # declare our exec vars
    local ounce_program_name
    local filenames_string
    local files_need_snap
    local snapshot_suffix="ounceSnapFileMount"

    [[ "$1" != "ounce" ]] || print_err_exit "'ounce' being called recursively. Quitting."
    [[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
    [[ "$1" != "--give-priv" ]] || give_priv

    # get inner executable name
    while (( "$#" )); do
        if [[ "$1" == "--suffix" ]]; then
            [[ -n "$2" ]] || print_err_exit "suffix is empty"
            snapshot_suffix="$2"
            shift 2
        else
            ounce_program_name="$( command -v "$1" )"
            [[ -x "$ounce_program_name" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."
            shift
            break
        fi
    done

    # loop through our shell arguments
    for a in "$@"; do
        # is the argument a file/directory that exists?
        [[ ! -e "$a" ]] || filenames_string+="$( printf "$a\n" )"
    done

    # check if filenames array is not empty
    if [[ -n "$filenames_string"  ]]; then
      # httm will dynamically determine the location of
      # the file's ZFS dataset and snapshot that mount
      # check whether to take snap - do we have a snap of the live file?
      # leave FILENAMES_STRING unquoted!!!
      files_need_snap="$( needs_snap $filenames_string )"
      [[ -z "$files_need_snap" ]] || exec_snap "$files_need_snap" "$snapshot_suffix"
    fi

    # execute original arguments
    "$ounce_program_name" "$@"
}

ounce_of_prevention "$@"