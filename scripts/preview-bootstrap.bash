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
	print_err "${@}"
	exit 1
}

print_err() {
	printf "%s\n" "ERROR: $*" 1>&2
}

print_warn_exit() {
	print_warn "${@}"
	exit 0
}

print_warn() {
	printf "%s\n" "WARN: $*"
}

graceful_shutdown() {
	printf "%s\n" "--"
	exit 0
}

prep_exec() {
	[[ -n "$(
		command -v cut
		exit 0
	)" ]] || print_err_exit "'cut' is required to execute 'httm --preview'.  Please check that 'cut' is in your path."
}

bootstrap_preview() {
	prep_exec

	local raw_input=""
	local snap_file=""

	raw_input={}

	[[ -n $raw_input ]] || print_err_exit "Selection is empty."

	[[ $raw_input != ─*─ ]] || graceful_shutdown

	# remove first and last chars in string in case they are also quotes 
	# possible we drop good chars, but these chars are unnecessary for parsing
	snap_file="$(echo ${raw_input} | cut -d'"' -f2)"

	[[ -n "$snap_file" ]] || print_err_exit "Snap file path is empty."

	[[ -f "$snap_file" ]] || [[ -d "$snap_file" ]] || [[ -L "$snap_file" ]] || print_warn_exit "Selection does not refer to a valid file, link or directory."

	exec 0<&-
	{command} 2>&1
}

bootstrap_preview
