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

set -euf -o pipefail
#set -x

print_err_exit() {
	print_err "$@"
	exit 1
}

print_err() {
	printf "%s\n" "ERROR: $*" 1>&2
}

graceful_shutdown() {
	printf "%s\n" "--"
	exit 0
}

prep_exec() {
	[[ -n "$(
		command -v bash
		exit 0
	)" ]] || print_err_exit "'bash' is required to execute 'httm --preview'.  Please check that 'bash' is in your path."
	[[ -n "$(
		command -v cut
		exit 0
	)" ]] || print_err_exit "'cut' is required to execute 'httm --preview'.  Please check that 'cut' is in your path."
}

is_fancy_border_line() {
	local raw_input=""
	raw_input=$1

	# is border line? is not a well formed line of text, probably a border line
	[[ "$( echo $raw_input | grep -c '"' )" -gt 0 ]] || graceful_shutdown
}

bootstrap_preview() {
	prep_exec

	local raw_input=""
	local snap_file=""

	raw_input={}

	[[ -n $raw_input ]] || print_err_exit "Selection is empty."

	# check does the string contain any quotes
	is_fancy_border_line $raw_input

	# remove first and last chars in string in case they are also quotes
	# possible we drop good chars, but these chars are unnecessary for parsing
	snap_file="$(echo ${raw_input:1:-1} | cut -d'"' -f2)"

	[[ -n "$snap_file" ]] || print_err_exit "Path is empty."

	[[ -f "$snap_file" ]] || [[ -d "$snap_file" ]] || [[ -L "$snap_file" ]] || print_err_exit "Path does not refer to a valid file, link or directory." 

	exec 0<&-
	{command} 2>&1
}

bootstrap_preview
