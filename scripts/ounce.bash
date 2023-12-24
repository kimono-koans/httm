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

# for the bible tells us so
set -euf -o pipefail
#set -x

function print_version {
	printf "\
ounce $(httm --version | cut -f2 -d' ')
" 1>&2
	exit 0
}

function print_usage {
	local ounce="\e[31mounce\e[0m"
	local httm="\e[31mhttm\e[0m"

	printf "\
$ounce is a wrapper script for $httm which snapshots the datasets of files opened by another programs.

$ounce only snapshots datasets when you have file changes outstanding, uncommitted to a snapshot already,
and only when those files are given as arguments to the target executable at the command line (except in --trace
mode).

USAGE:
	ounce [OPTIONS]... [target executable] [argument1 argument2...]
	ounce [--direct] [path1 path2...]
	ounce [--give-priv]
	ounce [--help]

OPTIONS:
	--background:
		Run the $ounce target executable in the background.  Safest for non-immediate file modifications
		(perhaps for use with your \$EDITOR, but not 'rm').  $ounce is fast-ish (for a shell script)
		but the time for ZFS to dynamically mount your snapshots will swamp the actual time to search snapshots
		and execute any snapshot.

	--trace:
		Trace file 'open' and 'openat' calls of the $ounce target executable using \"strace\" and eBPF/seccomp to
		determine relevant input files.

	--direct:
		Execute directly on path/s instead of wrapping a target executable.

	--give-priv:
		To use $ounce you will need privileges to snapshot ZFS datasets, and the prefered scheme is
		\"zfs-allow\".  Executing this option will give the current user snapshot privileges on all
		imported pools via \"zfs-allow\" . NOTE: The user must execute --give-priv as an unprivileged user.
		The user will be prompted later for elevated privileges.

	--suffix [suffix name]:
		User may specify a special suffix to use for the snapshots you take with $ounce.  See the $httm help,
		specifically \"httm --snap\", for additional information.

	--utc:
		User may specify UTC time for the timestamps listed on snapshot names.

	--help:
		Display this dialog.

	--version:
		Display script version.

" 1>&2
	exit 1
}

function print_err_exit {
	printf "%s\n" "Error: $*" 1>&2
	exit 1
}

function prep_trace {
	[[ "$(uname)" == "Linux" ]] || print_err_exit "ounce --trace mode is only available on Linux.  Sorry.  PRs welcome."
	[[ -n "$(
		command -v strace
		exit 0
	)" ]] || print_err_exit "'strace' is required to execute 'ounce' in trace mode.  Please check that 'strace' is in your path."
	[[ -n "$(
		command -v uuidgen
		exit 0
	)" ]] || print_err_exit "'uuidgen' is required to execute 'ounce' in trace mode.  Please check that 'uuidgen' is in your path."
}

function prep_exec {
	# Use zfs allow to operate without sudo
	[[ -n "$(
		command -v httm
		exit 0
	)" ]] || print_err_exit "'httm' is required to execute 'ounce'.  Please check that 'httm' is in your path."
	[[ -n "$(
		command -v zfs
		exit 0
	)" ]] || print_err_exit "'zfs' is required to execute 'ounce'.  Please check that 'zfs' is in your path."
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
		print_err_exit "'sudo'-like program is required to execute 'ounce' without special zfs-allow permissions.  Please check that 'sudo' (or 'doas' or 'pkexec') is in your path."

	printf "$sudo_program"
}

function take_snap {
	local filenames=""
	local suffix=""
	local utc=""

	filenames="$1"
	suffix="$2"
	utc="$3"

	# mask all the errors from the first run without privileges,
	# let the sudo run show errors
	[[ -z "$utc" ]] || httm "$utc" --snap="$suffix" $filenames 1>/dev/null 2>/dev/null
	[[ -n "$utc" ]] || httm --snap="$suffix" $filenames 1>/dev/null 2>/dev/null

	if [[ $? -ne 0 ]]; then
		local sudo_program
		sudo_program="$(prep_sudo)"

		[[ -z "$utc" ]] || httm "$utc" --snap="$suffix" $filenames 1>/dev/null 2>/dev/null
		[[ -n "$utc" ]] || httm --snap="$suffix" $filenames 1>/dev/null 2>/dev/null

		[[ $? -eq 0 ]] ||
			print_err_exit "'ounce' failed with a 'httm'/'zfs' snapshot error.  Check you have the correct permissions to snapshot."
	fi
}

function needs_snap {
	local uncut_res=""
	local filenames=""

	filenames="$1"

	uncut_res="$( httm --last-snap=no-ditto-inclusive --not-so-pretty $filenames 2>/dev/null)"
	#[[ $? -eq 0 ]] || print_err_exit "'ounce' failed with a 'httm' lookup error."
	[[ $? -eq 0 ]] || uncut_res=""

	cut -f1 -d: <<<"$uncut_res"
}

function give_priv {
	local pools=""
	local user_name=""
	local sudo_program=""

	user_name="$(whoami)"
	sudo_program="$(prep_sudo)"
	pools="$(get_pools)"

	[[ "$user_name" != "root" ]] || print_err_exit "'ounce' must be executed as an unprivileged user to obtain their true user name.  You will be prompted when additional privileges are needed.  Quitting."

	for p in $pools; do
		"$sudo_program" zfs allow "$user_name" mount,snapshot "$p" || print_err_exit "'ounce' could not obtain privileges on $p.  Quitting."
	done

	printf "\
Successfully obtained ZFS snapshot privileges on all the following pools:
$pools
" 1>&2
	exit 0
}

function get_pools {
	local pools=""
	local sudo_program=""

	sudo_program=$(prep_sudo)
	pools="$(sudo zpool list -o name | grep -v -e "NAME")"

	[[ -n "$pools" ]] ||
		print_err_exit "'ounce' failed because it appears no pools were imported.  Quitting."

	printf "$pools"
}

function exec_trace {
	local temp_pipe="$1"

	stdbuf -i0 -o0 -e0 cat -u "$temp_pipe" |
		stdbuf -i0 -o0 -e0 cut -f 2 -d$'\"' |
		stdbuf -i0 -o0 -e0 grep --line-buffered "\S" |
		stdbuf -i0 -o0 -e0 grep --line-buffered -v "+++" |
		while read -r file; do
			files_need_snap="$(needs_snap "$file")"
			[[ -z "$files_need_snap" ]] || take_snap "$files_need_snap" "$snapshot_suffix" "$utc"
		done
}

function exec_args {
	local filenames_string=""
	local files_need_snap=""
	local -a filenames_array=()
	local canonical_path=""

	# simply exit if there are no remaining arguments
	[[ $# -ge 1 ]] || return 0

	# loop through the rest of our shell arguments
	for a; do
		# omits argument flags
		[[ $a != -* && $a != --* ]] || continue
		canonical_path="$(
			readlink -e "$a" 2>/dev/null
			exit 0
		)"

		# 1) is file, symlink or dir with 2) write permissions set? (httm will resolve links)
		[[ -z "$canonical_path" ]] ||
			[[ ! -f "$canonical_path" && ! -d "$canonical_path" && ! -L "$canonical_path" ]] ||
			[[ ! -w "$canonical_path" ]] || filenames_array+=("$canonical_path")
	done

	# check if filenames array is not empty
	[[ ${#filenames_array[@]} -ne 0 ]] || return 0

	filenames_string="$( echo ${filenames_array[@]} )"
	[[ -n "$filenames_string" ]] || print_err_exit "bash could not covert file names from array to string."

	# now, httm will dynamically determine the location of
	# the file's ZFS dataset and snapshot that mount
	files_need_snap="$(needs_snap "$filenames_string")"
	[[ -z "$files_need_snap" ]] || take_snap "$files_need_snap" "$snapshot_suffix" "$utc"
}

function ounce_of_prevention {
	# do we have commands to execute?
	prep_exec

	# declare special vars
	local temp_pipe=""
	local uuid=""

	# declare our vars
	local program_name=""
	local background=false
	local trace=false
	local direct=false
	local -x snapshot_suffix="ounceSnapFileMount"
	local -x utc=""

	[[ $# -ge 1 ]] || print_usage
	[[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
	[[ "$1" != "-V" && "$1" != "--version" ]] || print_version
	[[ "$1" != "ounce" ]] || print_err_exit "'ounce' being called recursively. Quitting."
	[[ "$1" != "--give-priv" ]] || give_priv

	# get inner executable name
	while [[ $# -ge 1 ]]; do
		if [[ "$1" == "--suffix" ]]; then
			shift
			[[ $# -ge 1 ]] || print_err_exit "suffix is empty"
			snapshot_suffix="$1"
			shift
		elif [[ "$1" == "--utc" ]]; then
			utc="--utc"
			shift
		elif [[ "$1" == "--trace" ]]; then
			prep_trace

			trace=true

			uuid="$(uuidgen)"
			temp_pipe="/tmp/pipe.$uuid"

			trap "[[ ! -p $temp_pipe ]] || rm -f $temp_pipe" EXIT
			shift
		elif [[ "$1" == "--background" ]]; then
			background=true
			shift
		elif [[ "$1" == "--direct" ]]; then
			direct=true
			shift
			break
		else
			program_name="$(
				command -v "$1"
				exit 0
			)"
			shift
			break
		fi
	done

	# check the program name is executable
	[[ -x "$program_name" ]] || [[ $direct ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."

	# start search and snap, then execute original arguments
	if $trace; then
		# set local vars
		local background_pid

		# create temp pipe
		[[ ! -p "$temp_pipe" ]] || rm -f "$temp_pipe"
		mkfifo "$temp_pipe"

		# exec loop waiting for strace input background
		exec_trace "$temp_pipe" &
		background_pid="$!"

		# main exec
		stdbuf -i0 -o0 -e0 strace -A -o "| stdbuf -i0 -o0 -e0 cat -u > $temp_pipe" -f -e open,openat,openat2 -y --seccomp-bpf -- "$program_name" "$@"

		# cleanup
		wait "$background_pid"
	elif $background; then
		local background_pid

		exec_args "$@" &
		background_pid="$!"

		"$program_name" "$@"

		wait "$background_pid"
	elif $direct; then
		exec_args "$@" && printf "%s\n" "ounce successfully preserved all unsnapped changes." 1>&2
	else
		exec_args "$@" && printf "%s\n" "ounce successfully preserved all unsnapped changes." 1>&2
		"$program_name" "$@"
	fi
}

ounce_of_prevention "$@"