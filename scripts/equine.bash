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

function print_version {
	printf "\
equine $(httm --version | cut -f2 -d' ')
" 1>&2
	exit 0
}

function print_usage {
	local equine="\e[31mequine\e[0m"
	local httm="\e[31mhttm\e[0m"

	printf "\
$equine is a script to connect to the Time Machine network volume (NAS), mount the image file, 
and finally, mount all APFS snapshots necessary to use with $httm.  Not for use with Time Machines 
which utilize direct attached storage (DAS).

USAGE:
	equine [OPTIONS]

OPTIONS:
	--mount:
		Attempt to mount your Time Machine snapshots, the Time Machine image file from the server.

	--unmount:
		Attempt to unmount your Time Machine snapshots, the Time Machine image file from the server.

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
		command -v mount_smbfs
		exit 0
	)" ]] || print_err_exit "'mount_smbfs' is required to execute 'tm-mnt'.  Please check that 'mount_smbfs' is in your path."
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
		command -v hdiutil
		exit 0
	)" ]] || print_err_exit "'hdiutil' is required to execute 'tm-mnt'.  Please check that 'hdiutil' is in your path."
}

function _unmount_timemachine_() {
	printf "%s\n" "Unmounting any mounted snapshots...."
	mount | grep "com.apple.TimeMachine.*.backup@" | cut -d' ' -f1 | xargs -I{} umount "{}" 2>/dev/null  || true

	local image_name="$(plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep LocalizedDiskImageVolumeName | cut -d '"' -f4)"
	[[ -n "$image_name" ]] || print_err "Could not determine Time Machine disk image name, perhaps none is specified?"
	local sub_device="$( mount | grep "$image_name" | cut -d' ' -f1 | tail -1 )"
	local device="$( echo $sub_device | cut -d's' -f1 )"
	[[ -n "$sub_device" ]] || print_err "Could not determine subdevice from disk image given"
	[[ -n "$device" ]] || print_err "Could not determine device from disk image given"
	
	printf "%s\n" "Attempting to unmount Time Machine sparse bundle: $image_name ..."
	[[ -z "$sub_device" ]] || diskutil unmount "$sub_device" 2>/dev/null || true
	[[ -z "$device" ]] || diskutil unmountDisk "$device" 2>/dev/null || true

	local server="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "NetworkURL" | cut -d '"' -f4 )"
	local mount_source="$( echo "$server" | cut -d ':' -f2 | xargs basename )"
	local dirname="/Volumes/"$( printf "%b\n" "${mount_source//%/\\x}" )""
	[[ -n "$server" ]] || print_err "Could not determine server, perhaps none is specified?"
	[[ -n "$mount_source" ]] || print_err "Could not determine mount source from server name given"

	printf "%s\n" "Attempting to unmount/disconnect from Time Machine server: $server ..."
	[[ -z "$mount_source" ]] || diskutil unmount force "$dirname" 2>/dev/null || true
	[[ -z "$mount_source" ]] || diskutil unmountDisk force "$dirname" 2>/dev/null || true
}

function _mount_timemachine_() {
	local server="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "NetworkURL" | cut -d '"' -f4 )"
	local mount_source="$( echo "$server" | cut -d ':' -f2 | xargs basename )"

	[[ -n "$server" ]] || print_err_exit "Could not determine server address, perhaps none is specified?"
	[[ -n "$mount_source" ]] || print_err_exit "Could not determine mount source from server name"

	local dirname="/Volumes/"$( printf "%b\n" "${mount_source//%/\\x}" )""
	[[ -d "$dirname" ]] || mkdir "$dirname"

	if [[ "$( mount | grep -c "$dirname" )" -eq 0 ]]; then
		printf "%s\n" "Connecting to remote Time Machine: $server ..."
		mount_smbfs -o nobrowse "$server" "$dirname" 2>/dev/null || true
	else
		printf "%s\n" "Skip connecting to remote server, as Time Machine already mounted at: $dirname ..."
	fi

	# Wait for server to connect
	until [[ -d "$dirname" ]]; do sleep 1; done

	local image_name="$(plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep LocalizedDiskImageVolumeName | cut -d '"' -f4)"
	[[ -n "$image_name" ]] || print_err_exit "Could not determine Time Machine disk image name, perhaps none is specified?"
	
	if [[ "$( mount | grep -c "$image_name" )" -eq 0 ]]; then
		printf "%s\n" "Mounting sparse bundle (this may include an fsck): $image_name ..."
		find "$dirname" -type d -iname "*.sparsebundle" | head -1 | xargs -I{} hdiutil attach -readonly -nobrowse "{}"
	else
		printf "%s\n" "Skip mounting sparse bundle, as $image_name appears to already be mounted ..."
	fi
	
	printf "%s\n" "Discovering backup locations (this can take a few seconds)..."
	local backups="$( tmutil listbackups / )"
	local device="$( mount | grep "$image_name" | cut -d' ' -f1 | tail -1 )"
	local uuid="$( echo "$backups" | cut -d "/" -f4 | head -1 )"

	[[ -n "$device" ]] || print_err_exit "Could not determine Time Machine device from image give"
	[[ -n "$uuid" ]] || print_err_exit "Could not determine uuid from list of backup locations"

	[[ "$( mount | grep -c "$image_name" )" -gt 0 ]] || print_err_exit "Time machine disk image did not mount"

	[[ -d "/Volumes/.timemachine/$uuid" ]] || mkdir "/Volumes/.timemachine/$uuid"
	printf "%s\n" "Mounting snapshots..."
	for snap in $( echo "$backups" | xargs basename ); do
		[[ -d "/Volumes/.timemachine/$uuid/$snap" ]] || mkdir "/Volumes/.timemachine/$uuid/$snap"
		printf "%s\n" "Mounting snapshot "com.apple.TimeMachine.$snap" from "$device" at "/Volumes/.timemachine/$uuid/$snap""
		[[ -d "/Volumes/.timemachine/$uuid/$snap" ]] && mount_apfs -o ro,nobrowse -s "com.apple.TimeMachine.$snap" "$device" "/Volumes/.timemachine/$uuid/$snap" 2>/dev/null || true
	done
}

function _exec_() {
	[[ "$( uname )" == "Darwin" ]] || print_err_exit "This script requires you run it on MacOS"
	[[ "$EUID" -eq 0 ]] || print_err_exit "This script requires you run it as root"
	prep_exec

	[[ $# -ge 1 ]] || print_usage
	[[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
	[[ "$1" != "-V" && "$1" != "--version" ]] || print_version

	while [[ $# -ge 1 ]]; do
		if [[ "$1" == "--mount" ]]; then
			_mount_timemachine_
			break
		elif [[ "$1" == "--unmount" ]]; then
			_unmount_timemachine_
			break
		else
			print_err_exit "User must specify whether to mount or unmount the Time Machine volumes."
			break
		fi
	done
}

_exec_ "$@"