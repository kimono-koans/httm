#!/usr/bin/env bash

#       ___           ___           ___           ___
#      /\__\         /\  \         /\  \         /\__\
#     /:/  /         \:\  \        \:\  \       /::|  |
#    /:/__/           \:\  \        \:\  \     /:|:|  |
#   /::\  \ ___       /::\  \       /::\  \   /:/|:|__|__
#  /:/\:\  /\__\     /:/\:\__\     /:/\:\__\ /:/ |::::\__\
#  \/__\:\/:/  /    /:/  \/__/    /:/  \/__/ \/__/~~/:/  /
#       \::/  /    /:/  /        /:/  /            /:/  /
#       /:/  /     \/__/         \/__/            /:/  /
#      /:/  /                                    /:/  /
#      \/__/                                     \/__/
#
# Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
#
# For the full copyright and license information, please view the LICENSE file
# that was distributed with this source code.

## Note: env is zsh/bash here but could maybe/should work in zsh/bash too? ##

set -euf -o pipefail
#set -x

function print_version {
	printf "\
zdbstat $(httm --version | cut -f2 -d' ')
" 1>&2
	exit 0
}

function print_usage {
	local zdbstat="\e[31mzdbstat\e[0m"

	printf "\
$zdbstat prints the underlying zdb metadata for a ZFS object.  $zdbstat takes no options and requires at least one path.

USAGE:
	zdbstat [path1 path2...]

" 1>&2
	exit 1
}

function print_err_exit {
	print_err "$*"
	exit 1
}

function print_err {
	printf "%s\n" "ERROR: $*" 1>&2
}

function prep_exec {
	[[ -n "$(
		command -v stat
		exit 0
	)" ]] || print_err_exit "'stat' is required to execute 'zdbstat'.  Please check that 'stat' is in your path."
	[[ -n "$(
		command -v zdb
		exit 0
	)" ]] || print_err_exit "'zdb' is required to execute 'zdbstat'.  Please check that 'zdb' is in your path."
	[[ -n "$(
		command -v zfs
		exit 0
	)" ]] || print_err_exit "'zfs' is required to execute 'zdbstat'.  Please check that 'zfs' is in your path."
}

function prep_sudo {
	local sudo_program=""

	local -a program_list=(
		sudo
		doas
		pkexec
	)

	for p in "${program_list[@]}"; do
		sudo_program="$(
			command -v "$p"
			exit 0
		)"
		[[ -z "$sudo_program" ]] || break
	done

	[[ -n "$sudo_program" ]] ||
		print_err_exit "'sudo'-like program is required to execute.  Please check that 'sudo' (or 'doas' or 'pkexec') is in your path."

	printf "%s" "$sudo_program"
}

function dump_zfs_obj_metadata() {

    local file_name=""
    local sudo_program=""
    local dataset=""
    local inode=""

    file_name="$1"
    sudo_program="$2"
    source="$( zfs list -H -o name $file_name 2>/dev/null; exit 0 )"
    [[ -n "$source" ]] || source="$( zfs list -H -o name $file_name 2>&1 | cut -f2 -d"'" ; exit 0 )"
    inode="$( stat -c %i $file_name 2>/dev/null; exit 0 )"

    if [[ -z "$source" ]]; then
	    print_err_exit "Could not determine source dataset for path: $file_name"
    fi

    if [[ -z "$inode" ]]; then
	    print_err_exit "Could not determine inode for path: $inode"
    fi

    "$sudo_program" zdb -dddddddddd "$source" "$inode"
}


function run_loop() {
    prep_exec
    [[ $# -ge 1 ]] || print_usage

    local sudo_program="$( prep_sudo )"

    for f in "$@"; do

        local file_name=""
        file_name="$( readlink -e "$f" 2>/dev/null; exit 0 )"

        if [[ -z "$file_name" ]]; then
            print_err "WARN: Path likely does not exist: $f"
            continue
        fi

        if [[ "$( echo $file_name | grep -c ".zfs/snapshot" )" -eq 0 ]] && \
        [[ -z "$( zfs list $file_name 2>/dev/null; exit 0 )" ]]; then
            print_err "WARN: zdbstat requires a valid zfs path: $file_name"
            continue
        fi

        dump_zfs_obj_metadata "$file_name" "$sudo_program"

    done

}

run_loop "$@"
