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

$ounce only snapshots datasets when you have file changes outstanding, uncommitted to a snapshot already
(except in --trace mode).

When $ounce is invoked with only a target executable and its arguments, including paths, and no additional options,
$ounce will perform a snapshot check on those paths are that given as arguments to the target executable, and possibly wait
for a snapshot, before proceeding with execution of the target executable.

USAGE:
	ounce [target executable] [argument1 argument2...] [path1 path2...]
	ounce [OPTIONS]... [target executable] [argument1 argument2...] [path1 path2...]
	ounce [--direct] [path1 path2...]
	ounce [--give-priv]
	ounce [--help]

OPTIONS:
	--background:
		Run the snapshot check in the background (because it's faster).  Most practical for non-immediate file 
		modifications (perhaps for use with your \$EDITOR, but not 'rm').  $ounce is fast-ish (for a shell script)
		but the time for ZFS to dynamically mount your snapshots will swamp the actual time to search 
		snapshots and execute any snapshot.

	--trace:
		Trace file 'open','openat','openat2', and 'fsync' calls of the $ounce target executable using \"strace\"
		and eBPF/seccomp to determine when to trigger a snapshot check (because it's faster and more accurate).
		Most practical for non-immediate file modifications (perhaps for use with your \$EDITOR, but not 'rm').

	--direct:
		Execute directly on path/s instead of wrapping a target executable.

	--give-priv:
		To use $ounce you will need privileges to snapshot ZFS datasets, and the preferred scheme is
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

function log_info {
	printf "%s\n" "$*" 2>&1 | logger -t ounce
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
	[[ -n "$(
		command -v awk
		exit 0
	)" ]] || print_err_exit "'awk' is required to execute 'ounce' in trace mode.  Please check that 'awk' is in your path."
	[[ -n "$(
		command -v logger
		exit 0
	)" ]] || print_err_exit "'logger' is required to execute 'ounce' in trace mode.  Please check that 'logger' is in your path."
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
	[[ -z "$utc" ]] || httm "$utc" --snap="$suffix" "$filenames" 2>&1 | grep -v "dataset already exists" | logger -t ounce || true
	[[ -n "$utc" ]] || httm --snap="$suffix" "$filenames" 2>&1 | grep -v "dataset already exists" | logger -t ounce || true

	if [[ $? -ne 0 ]]; then
		local sudo_program
		sudo_program="$(prep_sudo)"

		[[ -z "$utc" ]] || httm "$utc" --snap="$suffix" "$filenames" 2>&1 | grep -v "dataset already exists" | logger -t ounce || true
		[[ -n "$utc" ]] || httm --snap="$suffix" "$filenames" 2>&1 | grep -v "dataset already exists" | logger -t ounce || true

		[[ $? -eq 0 ]] ||
			print_err_exit "'ounce' failed with a 'httm'/'zfs' snapshot error.  Check you have the correct permissions to snapshot."
	fi
}

function needs_snap {
	local uncut_res=""
	local filenames=""

	filenames="$1"

	uncut_res="$( httm --last-snap=no-ditto-inclusive --not-so-pretty "$filenames" 2>/dev/null)"
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
		stdbuf -i0 -o0 -e0 grep --line-buffered -v -e 'O_TMPFILE' -e '/dev/pts' -e 'socket:' |
		stdbuf -i0 -o0 -e0 awk -F'[() \"<>]' '$2 ~ /fsync/ { print $4 } $2 ~ /open/ { print $7 }' |
		while read -r file; do
			canonical_path="$(
				realpath "$file" 2>/dev/null
				exit 0
			)"

			# 1) is empty, dne 2) is file, symlink or dir or 3) if file is writable
			[[ -n "$canonical_path" ]] || continue
			[[ -f "$canonical_path" || -d "$canonical_path" || -L "$canonical_path" ]] || continue
			[[ -w "$canonical_path" ]] || continue

			# 3) is file a newly created tmp file? 
			[[ "$canonical_path" != *.swp && "$canonical_path" != ~* && "$canonical_path" != *~ && "$canonical_path" != *.tmp ]] || \
			[[ -n "$( find "$canonical_path" -not -newerct '-5 seconds' )" ]] || continue

			# now, httm will dynamically determine the location of
			# the file's ZFS dataset and snapshot that mount
			files_need_snap="$(needs_snap "$canonical_path")"
			[[ -n "$files_need_snap" ]] || continue
			log_info "File which needed snapshot: $files_need_snap"
			take_snap "$files_need_snap" "$snapshot_suffix" "$utc"
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
			realpath "$a" 2>/dev/null
			exit 0
		)"

		# 1) is file, symlink or dir with 2) write permissions set? (httm will resolve links)
		if [[ -z "$canonical_path" ]]; then
			log_info "Could not determine canonical path for: $a"
			continue
		fi

		if [[ ! -f "$canonical_path" && ! -d "$canonical_path" && ! -L "$canonical_path" ]]; then
			log_info "Path is not a valid for an ounce snapshot: $canonical_path"
			continue
		fi

		if [[ ! -w "$canonical_path" ]]; then
			log_info "Path is not writable: $canonical_path"
			continue
		fi

		filenames_array+=("$canonical_path")
	done

	# check if filenames array is not empty
	[[ ${#filenames_array[@]} -ne 0 ]] || return 0

	filenames_string="$( echo ${filenames_array[@]} )"
	[[ -n "$filenames_string" ]] || print_err_exit "bash could not convert file names from array to string."
	
	# now, httm will dynamically determine the location of
	# the file's ZFS dataset and snapshot that mount
	files_need_snap="$(needs_snap "$filenames_string")"
	[[ -z "$files_need_snap" ]] || log_info "Files which need snapshot: $( echo $files_need_snap | tr '\n' ' ')"
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

	# start search and snap, then execute original arguments
	if $trace; then
		# set local vars
		local background_pid

		# create temp pipe
		[[ ! -p "$temp_pipe" ]] || rm -f "$temp_pipe"
		mkfifo "$temp_pipe"

		# exec loop waiting for strace input background
		log_info "ounce session opened in trace mode with: $program_name"
		exec_trace "$temp_pipe" &
		background_pid="$!"

		# main exec
		stdbuf -i0 -o0 -e0 strace -A -o "| stdbuf -i0 -o0 -e0 cat -u > $temp_pipe" -f -e open,openat,openat2 -y --seccomp-bpf -- "$program_name" "${@}"

		# cleanup
		wait "$background_pid"
	elif $background; then
		local background_pid

		log_info "ounce session opened in background mode with: $program_name"
		exec_args "${@}" &
		background_pid="$!"

		"$program_name" "${@}"

		wait "$background_pid"
	elif $direct; then
		log_info "ounce session opened in direct mode"
		exec_args "${@}"
	else
		# check the program name is executable
		[[ -x "$program_name" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."
		log_info "ounce session opened in wrapper mode with: $program_name"
		exec_args "${@}"
		"$program_name" "${@}"
	fi

	log_info "ounce session closed"
}

ounce_of_prevention "${@}"
