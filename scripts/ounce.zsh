#!/bin/zsh

# for the bible tells us so
set -ef -o pipefail

function print_err_exit {
    echo "$@" 1>&2
    exit 1
}

function print_usage {
    ounce="\e[31mounce\e[0m"
    httm="\e[31mhttm\e[0m"

    printf "\
$ounce is a wrapper program that allows $httm to take snapshots snapshots of files you open at the command line.

$ounce aims to be transparent.  It takes no arguments except the program you wish to execute through it and that program's arguments, which may or may not be files.

USAGE: [target executable] [argument1 argument2...]\n" 1>&2
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

function needs_snap {
    local uncut_res
    uncut_res="$( httm --last-snap=no-ditto --not-so-pretty $@ )"
    [[ $? -eq 0 ]] || print_err_exit "'ounce' quit with a 'httm' error."
    echo "$uncut_res" | cut -f1 -d:
}

function ounce_of_prevention {
    # do we have commands to execute?
    prep_exec

    # declare our exec vars
    local ounce_program_name
    local filenames_string
    local files_need_snap

    # get inner executable name
    [[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
    [[ "$1" != "ounce" ]] || print_err_exit "'ounce' being called recursively. Quitting."
    ounce_program_name="$( command -v "$1" )"
    shift
    [[ -x "$ounce_program_name" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."

    # loop through our shell arguments
    for a; do
        # is the argument a file/directory that exists?
        [[ ! -e "$a" ]] || filenames_string+=( "$(printf "$a\0")" )
    done

    # check if filenames array is not empty
    if [[ -n filenames_string  ]]; then
      # httm will dynamically determine the location of
      # the file's ZFS dataset and snapshot that mount
      # check whether to take snap - do we have a snap of the live file?
      # leave FILENAMES_STRING unquoted!!!
      files_need_snap="$( needs_snap $filenames_string )"
      [[ -z "$files_need_snap" ]] || exec_snap "$files_need_snap"
    fi

    # execute original arguments
    "$ounce_program_name" "$@"
}

ounce_of_prevention "$@"