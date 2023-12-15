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

set -euf -o pipefail
#set -x

print_err_exit() {
	print_err "$@"
	exit 1
}

print_err() {
	printf "%s\n" "Error: $*" 1>&2
}

function prep_exec {
	# Use zfs allow to operate without sudo
	[[ -n "$(
		command -v plutil
		exit 0
	)" ]] || print_err_exit "'plutil' is required to execute 'auto-tm-mnt'.  Please check that 'plutil' is in your path."
	[[ -n "$(
		command -v tmutil
		exit 0
	)" ]] || print_err_exit "'tmutil' is required to execute 'auto-tm-mnt'.  Please check that 'tmutil' is in your path."
	[[ -n "$(
		command -v mount_apfs
		exit 0
	)" ]] || print_err_exit "'mount_apfs' is required to execute 'auto-tm-mnt'.  Please check that 'mount_apfs' is in your path."
	[[ -n "$(
		command -v mount
		exit 0
	)" ]] || print_err_exit "'mount' is required to execute 'auto-tm-mnt'.  Please check that 'mount' is in your path."
	[[ -n "$(
		command -v cut
		exit 0
	)" ]] || print_err_exit "'cut' is required to execute 'auto-tm-mnt'.  Please check that 'cut' is in your path."
	[[ -n "$(
		command -v grep
		exit 0
	)" ]] || print_err_exit "'grep' is required to execute 'auto-tm-mnt'.  Please check that 'grep' is in your path."
	[[ -n "$(
		command -v xargs
		exit 0
	)" ]] || print_err_exit "'xargs' is required to execute 'auto-tm-mnt'.  Please check that 'xargs' is in your path."
}

function mount_timemachine() {
    prep_exec

	[[ "$EUID" -eq 0 ]] || print_err_exit "This script requires you run as root"

	server="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "NetworkURL" | cut -d '"' -f4 )"
	mount_source="$( echo "$server" | cut -d ':' -f2 | xargs basename )"
	dirname="$( printf "%b\n" "${mount_source//%/\\x}" )"

	[[ -n "$server" ]] || print_err_exit "Could not determine server"
	[[ -n "$mount_source" ]] || print_err_exit "Could not determine mount source"
	[[ -n "$dirname" ]] || print_err_exit "Could not determine directory name"

	open "$server"

	# Wait for server to mount, and open any sparse image bundle on the server
	until [[ -d "/Volumes/$dirname" ]]
	do
     		sleep 1
	done
	find "/Volumes/$dirname" -type d -iname "*.sparsebundle" -exec open -a DiskImageMounter.app "{}" \;

	until [[ "$( mount | grep -c "/Volumes/Backups" )" -gt 0 ]]
	do
     		sleep 1
	done

	device="$( mount | grep Backups | sort | head -1 | cut -d' ' -f1 )"
	uuid="$( tmutil listbackups / | cut -d "/" -f4 | head -1 )"

	[[ -n "$device" ]] || print_err_exit "Could not determine device"
	[[ -n "$uuid" ]] || print_err_exit "Could not determine uuid"
	[[ -d "/Volumes/.timemachine/$uuid" ]] || mkdir "/Volumes/.timemachine/$uuid"

	for snap in $( tmutil listbackups / | xargs basename ); do
		[[ -d "/Volumes/.timemachine/$uuid/$snap" ]] || mkdir "/Volumes/.timemachine/$uuid/$snap"
		mount_apfs -s "com.apple.TimeMachine.$snap" "$device" "/Volumes/.timemachine/5E44881A-89EF-4DB3-906D-54C2E9E2E2B6/$snap" 2&>1 /dev/null
	done
}

mount_timemachine &

printf 'Connecting the Time Machine share and mounting the sparse bundle image\n'
while kill -0 -- $! 2>/dev/null; do
    printf '.' > /dev/tty
    sleep 1
done

echo