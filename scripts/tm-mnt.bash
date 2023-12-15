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
# Copyright (c) 2023, Robert Swinford <robert.swinford<...at...>gmail.com>
#
# For the full copyright and license information, please view the LICENSE file
# that was distributed with this source code.

#set -euf -o pipefail
#set -x

print_err_exit() {
	print_err "$@"
	exit 1
}

print_err() {
	printf "%s\n" "Error: $*" 1>&2
}

function prep_exec {
	[[ -n "$(
		command -v plutil
		exit 0
	)" ]] || print_err_exit "'plutil' is required to execute 'tm-mnt'.  Please check that 'plutil' is in your path."
	[[ -n "$(
		command -v tmutil
		exit 0
	)" ]] || print_err_exit "'tmutil' is required to execute 'tm-mnt'.  Please check that 'tmutil' is in your path."
	[[ -n "$(
		command -v mount_apfs
		exit 0
	)" ]] || print_err_exit "'mount_apfs' is required to execute 'tm-mnt'.  Please check that 'mount_apfs' is in your path."
	[[ -n "$(
		command -v mount
		exit 0
	)" ]] || print_err_exit "'mount' is required to execute 'tm-mnt'.  Please check that 'mount' is in your path."
	[[ -n "$(
		command -v cut
		exit 0
	)" ]] || print_err_exit "'cut' is required to execute 'tm-mnt'.  Please check that 'cut' is in your path."
	[[ -n "$(
		command -v grep
		exit 0
	)" ]] || print_err_exit "'grep' is required to execute 'tm-mnt'.  Please check that 'grep' is in your path."
	[[ -n "$(
		command -v xargs
		exit 0
	)" ]] || print_err_exit "'xargs' is required to execute 'tm-mnt'.  Please check that 'xargs' is in your path."
	[[ -n "$(
		command -v open
		exit 0
	)" ]] || print_err_exit "'open' is required to execute 'tm-mnt'.  Please check that 'open' is in your path."
}

function mount_timemachine() {
	prep_exec

	[[ "$EUID" -eq 0 ]] || print_err_exit "This script requires you run as root"

	local server="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "NetworkURL" | cut -d '"' -f4 )"
	local mount_source="$( echo "$server" | cut -d ':' -f2 | xargs basename )"
	local dirname="$( printf "%b\n" "${mount_source//%/\\x}" )"

	[[ -n "$server" ]] || print_err_exit "Could not determine server"
	[[ -n "$mount_source" ]] || print_err_exit "Could not determine mount source"
	[[ -n "$dirname" ]] || print_err_exit "Could not determine directory name"

	open "$server"

	# Wait for server to connect
	until [[ -d "/Volumes/$dirname" ]]
	do
     		sleep 1
	done

	#find "/Volumes/$dirname" -type d -iname "*.sparsebundle" -exec open -a DiskImageMounter.app "{}" \;
	find "/Volumes/$dirname" -type d -iname "*.sparsebundle" | head -1 | xargs -I{} open -a DiskImageMounter.app "{}"

	# Wait for sparse image bundle to mount
	until [[ "$( mount | grep -c "/Volumes/Backups" )" -gt 0 ]]
	do
     		sleep 1
	done

	local backups="$( tmutil listbackups / )"
	local device="$( mount | grep "/Volumes/Backups" | cut -d' ' -f1 | tail -1 )"
	local uuid="$( echo "$backups" | cut -d "/" -f4 | head -1 )"

	[[ -n "$device" ]] || print_err_exit "Could not determine device"
	[[ -n "$uuid" ]] || print_err_exit "Could not determine uuid"

	[[ -d "/Volumes/.timemachine/$uuid" ]] || mkdir "/Volumes/.timemachine/$uuid"
	printf "\n%s\n" "Mounting snapshots"
	for snap in $( echo "$backups" | xargs basename ); do
		[[ -d "/Volumes/.timemachine/$uuid/$snap" ]] || mkdir "/Volumes/.timemachine/$uuid/$snap"
		printf "%s\n" "Mounting snapshot "com.apple.TimeMachine.$snap" from "$device" at "/Volumes/.timemachine/$uuid/$snap""
		[[ -d "/Volumes/.timemachine/$uuid/$snap" ]] && mount_apfs -s "com.apple.TimeMachine.$snap" "$device" "/Volumes/.timemachine/$uuid/$snap" 2> /dev/null
	done
}

mount_timemachine &

printf 'Connecting the Time Machine share and mounting the sparse bundle image\n'
while kill -0 $! 2>/dev/null; do
    printf '.' > /dev/tty
    sleep 1
done