#!/bin/bash

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
# (c) Robert Swinford <robert.swinford<...at...>gmail.com>
#
# For the full copyright and license information, please view the LICENSE file
# that was distributed with this source code.

set -euf -o pipefail
#set -x

print_version() {
	printf "\
bowie $(httm --version | cut -f2 -d' ')
" 1>&2
	exit 0
}

print_usage() {
	local bowie="\e[31mbowie\e[0m"
	local httm="\e[31mhttm\e[0m"

	printf "\
$bowie is a wrapper script for $httm which displays the difference between unique snapshot versions and the live file.

USAGE:
	bowie [OPTIONS]... [file1 file2...]

OPTIONS:
	--last:
		Default mode.  Display only the difference between the last unique snapshot version and the live file.

	--all:
		Display the difference between each unique snapshot version against the next version, and finally the live file.

	--select
		Start an $httm interactive session to select the snapshot difference to display against the live file.

	--direct
		Print difference with $bowie formatting.

	--help:
		Display this dialog.

	--version:
		Display script version.

" 1>&2
	exit 1
}

print_err_exit() {
	print_err "$@"
	exit 1
}

print_err() {
	printf "%s\n" "Error: $*" 1>&2
}

prep_exec() {
	# Use zfs allow to operate without sudo
	[[ -n "$(
		command -v diff
		exit 0
	)" ]] || print_err_exit "'diff' is required to execute 'bowie'.  Please check that 'diff' is in your path."
	[[ -n "$(
		command -v httm
		exit 0
	)" ]] || print_err_exit "'httm' is required to execute 'bowie'.  Please check that 'httm' is in your path."
}

show_all_changes() {
	local filename="$1"
	local previous_version=""
	local -a all_versions

	while read -r line; do
		all_versions+=("$line")
	done <<<"$(httm -n --omit-ditto "$filename")"

	# check if versions array is not empty
	if [[ ${#all_versions[@]} -eq 0 ]]; then
		print_err "No previous version available for: $filename"
		return 0
	elif [[ ${#all_versions[@]} -eq 1 ]]; then
		show_single_change "$filename" "last"
		return 0
	fi

	display_header "$filename"

	for current_version in "${all_versions[@]}"; do
		# check if initial "previous_version" needs to be set
		if [[ -z "$previous_version" ]]; then
			previous_version="$current_version"
			continue
		fi

		display_diff "$previous_version" "$current_version"

		# set current_version to previous_version
		previous_version="$current_version"
	done
}

check_not_identical() {
	local current_version="$1"
	local previous_version="$2"

	[[ -n "$(diff -q "$previous_version" "$current_version")" ]] ||
		print_err_exit "The selected/last version and live file are 'diff'-identical, but have different modification times.  Perhaps try --all."
}

show_single_change() {
	local current_version="$1"
	local previous_version=""
	local mode="$2"

	[[ "$mode" != "select" ]] || previous_version="$(httm --select --raw "$current_version")"
	[[ "$mode" != "last" ]] || previous_version="$(httm --omit-ditto --last-snap --raw "$current_version")"

	display_header "$current_version"
	check_not_identical "$previous_version" "$current_version"
	display_diff "$previous_version" "$current_version"
}

display_header() {
	local filename="$1"

	printf "\
$filename
__
"

}

display_diff() {
	local previous_version="$1"
	local current_version="$2"

	if [[ -n "$previous_version" ]]; then
		# print that current version and previous version differ, or are the same
		(diff --color=always -q "$previous_version" "$current_version" || true)
		# print the difference between that current version and previous_version
		(diff --color=always -T "$previous_version" "$current_version" || true)
	else
		print_err "No previous version available for: $current_version"
	fi
}

exec_main() {
	#ask if we have the correct commands
	prep_exec

	# declare our variables
	local mode="last"
	local canonical_path=""

	[[ $# -ge 1 ]] || print_usage
	[[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
	[[ "$1" != "-V" && "$1" != "--version" ]] || print_version

	if [[ $1 == "--all" ]]; then
		mode="all"
		shift
	elif [[ $1 == "--select" ]]; then
		mode="select"
		shift
	elif [[ $1 == "--direct" ]]; then
		mode="direct"
		shift
	elif [[ $1 == "--last" ]]; then
		shift
	fi

	for a; do
		[[ "$a" != -* && "$a" != --* ]] ||
			print_err_exit "Option specified either was not expected or is not permitted in this context.  Quitting."
	done

	[[ ${#@} -ne 0 ]] || print_err_exit "No filenames specified.  Quitting."

	if [[ "$mode" == "direct" ]]; then
		previous_version="$(
			readlink -e "$1" 2>/dev/null
			[[ $? -eq 0 ]] ||
				(print_err "Could not determine canonical path for: $1")
		)"

		current_version="$(
			readlink -e "$2" 2>/dev/null
			[[ $? -eq 0 ]] ||
				(print_err "Could not determine canonical path for: $2")
		)"

		display_diff "$previous_version" "$current_version"
		exit 0
	fi

	for a; do
		canonical_path="$(
			readlink -e "$a" 2>/dev/null
			[[ $? -eq 0 ]] ||
				(print_err "Could not determine canonical path for: $a")
		)"

		[[ -n "$canonical_path" ]] || continue
		[[ -f "$canonical_path" ]] || print_err "Skipping path which is not a file: $a"
		
		if [[ "$mode" == "all" ]]; then
			show_all_changes "$canonical_path"
		elif [[ "$mode" == "select" ]]; then
			show_single_change "$canonical_path" "$mode"
		else
			show_single_change "$canonical_path" "$mode"
		fi
	done
}

exec_main "$@"