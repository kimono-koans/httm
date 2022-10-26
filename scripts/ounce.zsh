#!/usr/bin/env zsh

### Note: env is zsh/bash here but could maybe/should work in zsh/bash too? ###

### for the bible tells us so
set -x -euf -o pipefail

function print_usage {

	local ounce="\e[31mounce\e[0m"
	local httm="\e[31mhttm\e[0m"

	printf "\
$ounce is a wrapper script for $httm which snapshot the dataset of files opened by other programs.

$ounce only snapshots datasets when you have file changes outstanding, uncommitted to a snapshot already,
and only when those files are given as arguments to the target executable at the command line.

USAGE:
	ounce [target executable] [argument1 argument2...]
	ounce [OPTIONS]... [target executable] [argument1 argument2...]
	ounce [--give-priv]
	ounce [--help]

OPTIONS:
	--background:
		Run the $ounce target executable in the background.  Safest for non-immediate file modifications
		(perhaps for use with your \$EDITOR, but not 'rm').  $ounce is fast-ish (for a shell script)
		but the time for ZFS to dynamically mount your snapshots will swamp actual time to search snapshots
		and time to execute any snapshot.

	--give-priv:
		To use $ounce you will need privileges to snapshot ZFS datasets, and the prefered scheme is
		\"zfs-allow\".  Executing this option will give the current user snapshot privileges on all
		imported pools via \"zfs-allow\" . NOTE: User must execute --give-priv as an unprivileged user.
		The user will be prompted later for elevated privileges.

	--suffix [suffix name]:
		User may specify a special suffix to use for the snapshots you take with $ounce.  See the $httm help,
		specifically \"httm --snap\", for additional information.

	--utc:
		User may specify UTC time for the timestamps listed on snapshot names.

	--help:
		Display this dialog.

" 1>&2
	exit 1
}

function print_err_exit {

	printf "%s\n" "Error: $*" 1>&2
	exit 1
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

	local sudo_program
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

	local filenames="$1"
	local suffix="$2"
	local utc="$3"
	local are_we_done

	# mask all the errors from the first run without privileges,
	# let the sudo run show errors
	[[ -z "$utc" ]] || printf "$filenames" | httm "$utc" --snap="$suffix" 1>/dev/null 2>/dev/null
	[[ -n "$utc" ]] || printf "$filenames" | httm --snap="$suffix" 1>/dev/null 2>/dev/null

	if [[ $? -ne 0 ]]; then
		local sudo_program
		sudo_program="$(prep_sudo)"

		[[ -z "$utc" ]] || printf "$filenames" | "$sudo_program" httm "$utc" --snap="$suffix" 1>/dev/null
		[[ -n "$utc" ]] || printf "$filenames" | "$sudo_program" httm --snap="$suffix" 1>/dev/null

		[[ $? -eq 0 ]] ||
			print_err_exit "'ounce' failed with a 'httm'/'zfs' snapshot error.  Check you have the correct permissions to snapshot."
	fi

	for i in {1..3}; do
		are_we_done="$( needs_snap "$filenames" )"
		[[ $? -eq 0 ]] || print_err_exit "Request to confirm snapshot taken exited uncleanly.  Quitting."
		[[ -n "$are_we_done" ]] || break
		sleep 1
	done
}

function needs_snap {

	local uncut_res
	local filenames="$1"

	uncut_res="$(printf "$filenames" | httm --last-snap=no-ditto-inclusive --not-so-pretty 2>/dev/null)"
	[[ $? -eq 0 ]] || print_err_exit "'ounce' failed with a 'httm' lookup error."
	cut -f1 -d: <<<"$uncut_res"
}

function give_priv {

	local pools
	local user_name="$(whoami)"

	[[ "$user_name" != "root" ]] || print_err_exit "'ounce' must be executed as an unprivileged user to obtain their true user name.  You will be prompted when additional privileges are needed.  Quitting."
	pools="$(get_pools)"

	for p in $pools; do
		sudo zfs allow "$user_name" mount,snapshot "$p" || print_err_exit "'ounce' could not obtain privileges on $p.  Quitting."
	done

	printf "\
Successfully obtained ZFS snapshot privileges on all the following pools:
$pools
" 1>&2
	exit 0
}

function get_pools {

	local pools

	pools="$(sudo zpool list -o name | grep -v -e "NAME")"

	printf "$pools"
}

function exec_main {

	local filenames_string
	local files_need_snap
	local -a filenames_array
	local canonical_path

	# loop through the rest of our shell arguments
	for a; do
		# omits argument flags
		if [[ $a == -* ]] || [[ $a == --* ]]; then
			continue
		else
			unset canonical_path
			canonical_path="$( readlink -e "$a" 2>/dev/null )"

			# 1) is file, symlink or dir with 2) write permissions set? (httm will resolve links)
			[[ ! -f "$canonical_path" && ! -d "$canonical_path" && ! -L "$canonical_path" ]] ||
			[[ ! -w "$canonical_path" ]] || filenames_array+=("$canonical_path")
		fi
	done

	# check if filenames array is not empty
	if [[ ${#filenames_array[@]} ]]; then
		printf -v filenames_string "%s\n" "${filenames_array[@]}"

		# now, httm will dynamically determine the location of
		# the file's ZFS dataset and snapshot that mount

		files_need_snap="$(needs_snap "$filenames_string")"
		[[ -z "$files_need_snap" ]] || take_snap "$files_need_snap" "$snapshot_suffix" "$utc"
	fi

}

function ounce_of_prevention {

	# do we have commands to execute?
	prep_exec

	# declare our vars
	local program_name
	local background=false
	local snapshot_suffix="ounceSnapFileMount"
	local utc=""

	[[ "$1" != "ounce" ]] || print_err_exit "'ounce' being called recursively. Quitting."
	[[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
	[[ "$1" != "--give-priv" ]] || give_priv

	# get inner executable name
	while [[ $# -ne 0 ]]; do
		if [[ "$1" == "--suffix" ]]; then
			[[ -n "$2" ]] || print_err_exit "suffix is empty"
			snapshot_suffix="$2"
			shift 2
		elif [[ "$1" == "--utc" ]]; then
			utc="--utc"
			shift
		elif [[ "$1" == "--background" ]]; then
			background=true
			shift
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
	[[ -x "$program_name" ]] || print_err_exit "'ounce' requires a valid executable name as the first argument."

	# start search and snap, then execute original arguments
	if [[ $background ]]; then
		local background_pid
		exec_main "$@" &
		background_pid="$!"
		"$program_name" "$@"
		wait "$background_pid"
	else
		exec_main "$@"
		"$program_name" "$@"
	fi
}

ounce_of_prevention "$@"