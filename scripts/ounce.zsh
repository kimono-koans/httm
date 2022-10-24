#!/usr/bin/env zsh

# Note: env is zsh/bash here but could maybe/should work in zsh/bash too?

# for the bible tells us so
set -euf -o pipefail

function print_usage {
	local ounce="\e[31mounce\e[0m"
	local httm="\e[31mhttm\e[0m"

	printf "\
$ounce is a wrapper program that allows $httm to take snapshots of files you open with other programs at the command line.

USAGE:
	ounce [target executable] [argument1 argument2...]
	ounce [OPTIONS]... [target executable] [argument1 argument2...]

OPTIONS:
	--background:
		Run the $ounce task in the background.  Safest for non-immediate file modifications
		(perhaps for your \$EDITOR but not for 'rm').  $ounce is fast-ish (for a shell script)
		but the time to ZFS mount snapshot will swamp its actual execution time.

	--give-priv:
		To use $ounce you will need privileges to snapshot ZFS datasets.  The prefered scheme
		is via zfs-allow.  Executing --give-priv will give the current user snapshot privileges
		on all imported pools. NOTE: User must execute --give-priv as an unprivileged user.  User
		will be prompted elevated privileges later.

	--suffix [suffix name]:
		You may specify a special suffix to use for the snapshots you take.  See the $httm help,
		specifically \"httm --snap\", for additional information.

	--utc:
		You may specify UTC time for the timestamps listed on snapshot names.

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

	echo "$sudo_program"
}

function exec_snap {
	# mask all the errors from the first run without privileges,
	# let the sudo run show errors
	if [[ $( printf "$1" | httm $3 --snap="$2" 1>/dev/null 2>/dev/null ) -ne 0 ]]; then
		local sudo_program
		sudo_program="$(prep_sudo)"

		printf "$1" | $sudo_program httm $3 --snap="$2" 1>/dev/null
		[[ $? -eq 0 ]] ||
			print_err_exit "'ounce' failed with a 'httm'/'zfs' snapshot error.  Check you have the correct permissions to snapshot."
	fi
}

function needs_snap {
	local uncut_res

	uncut_res="$( echo "$1" | httm --last-snap=no-ditto --not-so-pretty 2>/dev/null )"
	[[ $? -eq 0 ]] || print_err_exit "'ounce' failed with a 'httm' lookup error."
	cut -f1 -d: <<<"$uncut_res"
}

function give_priv {
	local user_name
	local pools

	user_name="$( whoami )"
	[[ "$user_name" != "root" ]] || print_err_exit "'ounce' must be executed as an unprivileged user to obtain their true user name.  You will be prompted when additional privileges are needed.  Quitting."
	pools="$( get_pools )"

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

	echo "$pools"
}


function exec_main {

	local filenames_string
	local files_need_snap
	local -a filenames_array

        # loop through the rest of our shell arguments
	for a; do
		# 1) is file, symlink or dir with 2) write permissions set? (httm will resolve links)
		[[ ! -f "$a" && ! -d "$a" && ! -L "$a" ]] || \
		[[ ! -w "$a" ]] || filenames_array+=("$a")
	done

	# check if filenames array is not empty
	if [[ ${#filenames_array[@]} -ne 0 ]]; then
		# now, httm will dynamically determine the location of
		# the file's ZFS dataset and snapshot that mount

		# do NOT use quotes on filesnames_string var
		# if delimiter is newline instead of a null!
		printf -v filenames_string "%s\0" "${filenames_array[@]}"

		files_need_snap="$( needs_snap "$filenames_string" )"
		[[ -z "$files_need_snap" ]] || exec_snap "$files_need_snap" "$snapshot_suffix" "$utc"
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

	# execute original arguments
	if [[ $background ]]; then
		local background_pid
		exec_main "$@" & background_pid="$!"
		"$program_name" "$@"
		wait "$background_pid"
	else
		exec_main
		"$program_name" "$@"
	fi
}

ounce_of_prevention "$@"