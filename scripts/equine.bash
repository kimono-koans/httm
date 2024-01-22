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
$equine is a script to connect to the Time Machine network volume (NAS), mount
the image file, and finally, mount all APFS snapshots necessary to use with $httm.
This script is not for use with Time Machines which utilize direct attached
storage (DAS).

USAGE:
	equine [OPTIONS]

OPTIONS:
	--mount-local:
		Attempt to mount your local Time Machine snapshots.

	--unmount-local:
		Attempt to unmount your local Time Machine snapshots.

	--mount-remote:
		Attempt to mount your remote Time Machine snapshots and the Time Machine image file,
		and connect the server.

	--unmount-remote:
		Attempt to unmount your remote Time Machine snapshots and the Time Machine image file,
		and disconnect the server.

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
	)" ]] || print_err_exit "'plutil' is required to execute 'equine'.  Please check that 'plutil' is in your path."
	[[ -n "$(
		command -v tmutil
		exit 0
	)" ]] || print_err_exit "'tmutil' is required to execute 'equine'.  Please check that 'tmutil' is in your path."
	[[ -n "$(
		command -v mount_apfs
		exit 0
	)" ]] || print_err_exit "'mount_apfs' is required to execute 'equine'.  Please check that 'mount_apfs' is in your path."
	[[ -n "$(
		command -v mount_smbfs
		exit 0
	)" ]] || print_err_exit "'mount_smbfs' is required to execute 'equine'.  Please check that 'mount_smbfs' is in your path."
	[[ -n "$(
		command -v mount
		exit 0
	)" ]] || print_err_exit "'mount' is required to execute 'equine'.  Please check that 'mount' is in your path."
	[[ -n "$(
		command -v cut
		exit 0
	)" ]] || print_err_exit "'cut' is required to execute 'equine'.  Please check that 'cut' is in your path."
	[[ -n "$(
		command -v grep
		exit 0
	)" ]] || print_err_exit "'grep' is required to execute 'equine'.  Please check that 'grep' is in your path."
	[[ -n "$(
		command -v xargs
		exit 0
	)" ]] || print_err_exit "'xargs' is required to execute 'equine'.  Please check that 'xargs' is in your path."
	[[ -n "$(
		command -v hdiutil
		exit 0
	)" ]] || print_err_exit "'hdiutil' is required to execute 'equine'.  Please check that 'hdiutil' is in your path."
}

function mount_remote() {
	local server="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "NetworkURL" | cut -d '"' -f4 )"
	local mount_source="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "LastKnownVolumeName" | cut -d '"' -f4  )"

	[[ -n "$server" ]] || print_err_exit "Could not determine server address, perhaps none is specified?"
	[[ -n "$mount_source" ]] || print_err_exit "Could not determine mount source from server name"

	local sys_defined_dir="$( find /Volumes/.timemachine -name 'TM Volume' -print -quit )"
	local dirname="/Volumes/$mount_source"
	[[ -z "$sys_defined_dir" ]] || dirname="$sys_defined_dir"

	if [[ "$( mount | grep -c "$dirname" )" -eq 0 ]]; then
		printf "%s\n" "Connecting to remote Time Machine: $server ..."
		[[ -d "$dirname" ]] || mkdir "$dirname"
		mount_smbfs -o nobrowse "$server" "$dirname" 2>/dev/null || true
	else
		printf "%s\n" "Skip connecting to remote server, as Time Machine already mounted ..."
	fi

	# Wait for server to connect
	timeout 30s bash -c "until [[ "$( mount | grep -c "$dirname" )" -gt 0 ]]; do sleep 1; done" || \
	print_err_exit "Wait for server to be mounted timed out.  Quitting."

	local image_name="$(plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep LocalizedDiskImageVolumeName | cut -d '"' -f4)"
	[[ -n "$image_name" ]] || print_err_exit "Could not determine Time Machine disk image name, perhaps none is specified?"

	if [[ "$( mount | grep -c "$image_name" )" -eq 0 ]]; then
		printf "%s\n" "Mounting sparse bundle (this may include an fsck): $image_name ..."
		local bundle_name="$( find "$dirname" -type d -iname "*.sparsebundle" -print -quit )"
		[[ -n "$bundle_name" ]] || print_err_exit "Could not find sparsebundle in location specified"
		hdiutil attach -readonly -nobrowse "$bundle_name" || print_err_exit "Attaching disk image failed.  Quitting."
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

function unmount_remote() {
	printf "%s\n" "Unmounting any mounted snapshots...."
	mount | grep "com.apple.TimeMachine.*.backup@" | cut -d' ' -f1 | xargs -I{} umount "{}" 2>/dev/null  || true

	local image_name="$(plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep LocalizedDiskImageVolumeName | cut -d '"' -f4)"
	[[ -n "$image_name" ]] || print_err "Could not determine Time Machine disk image name, perhaps none is specified?"
	local sub_device="$( mount | grep "$image_name" | cut -d' ' -f1 | tail -1 )"
	local device="$( echo $sub_device | cut -d's' -f1 )"
	[[ -n "$sub_device" ]] || print_err "Could not determine subdevice from disk image given"
	[[ -n "$device" ]] || print_err "Could not determine device from disk image given"

	local server="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "NetworkURL" | cut -d '"' -f4 )"
	local mount_source="$( plutil -p /Library/Preferences/com.apple.TimeMachine.plist | grep "LastKnownVolumeName" | cut -d '"' -f4  )"

	local sys_defined_dir="$( find /Volumes/.timemachine -name 'TM Volume' -print -quit )"
	local dirname="/Volumes/$mount_source"
	[[ -z "$sys_defined_dir" ]] || print_err_exit "Time Machine share is mounted at a system defined mount point (and a backup could be in progress).  Quitting without unmounting."

	printf "%s\n" "Attempting to unmount Time Machine sparse bundle: $image_name ..."
	[[ -z "$sub_device" ]] || diskutil unmount "$sub_device" 2>/dev/null || true
	[[ -z "$device" ]] || diskutil unmountDisk "$device" 2>/dev/null || true

	[[ -n "$server" ]] || print_err "Could not determine server, perhaps none is specified?"
	[[ -n "$mount_source" ]] || print_err "Could not determine mount source from server name given"

	printf "%s\n" "Attempting to unmount/disconnect from Time Machine server: $server ..."
	[[ -z "$mount_source" ]] || diskutil unmount force "$dirname" 2>/dev/null || true
	[[ -z "$mount_source" ]] || diskutil unmountDisk force "$dirname" 2>/dev/null || true
}

function mount_local() {
	printf "%s\n" "Discovering backup locations (this can take a few seconds)..."
	local backups="$( tmutil listlocalsnapshots /System/Volumes/Data | grep -v ':' )"
	local device="$( mount | grep "/System/Volumes/Data " | cut -d' ' -f1 )"
	local hostname="$( hostname )"

	[[ -n "$device" ]] || print_err_exit "Could not determine Time Machine device from image give"

	printf "%s\n" "Mounting snapshots..."
	for snap in $( echo "$backups" ); do
		local snap_uuid=""
		snap_uuid="$( echo $snap | cut -d'.' -f4 )"
		[[ -d "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb/$hostname/$snap_uuid/Data" ]] || \
		mkdir "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb/$hostname/$snap_uuid/Data"

		printf "%s\n" "Mounting snapshot "$snap" from "$device" at "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb/$hostname/$snap_uuid/Data""
		[[ -d "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb/$hostname/$snap_uuid/Data" ]] && \
		mount_apfs -o ro,nobrowse -s "$snap" "$device" "/Volumes/com.apple.TimeMachine.localsnapshots/Backups.backupdb/$hostname/$snap_uuid/Data" 2>/dev/null || true
	done
}

function unmount_local() {
	printf "%s\n" "Unmounting any mounted snapshots...."
	mount | grep "com.apple.TimeMachine.*.local@" | cut -d' ' -f1 | xargs -I{} umount "{}" 2>/dev/null  || true
}

function exec_equine() {
	[[ $# -ge 1 ]] || print_usage
	[[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
	[[ "$1" != "-V" && "$1" != "--version" ]] || print_version

	[[ "$( uname )" == "Darwin" ]] || print_err_exit "This script requires you run it on MacOS"
	[[ "$EUID" -eq 0 ]] || print_err_exit "This script requires you run it as root"
	prep_exec

	while [[ $# -ge 1 ]]; do
		if [[ "$1" == "--mount-remote" ]]; then
			mount_remote
			break
		elif [[ "$1" == "--unmount-remote" ]]; then
			unmount_remote
			break
		elif [[ "$1" == "--mount-local" ]]; then
			mount_local
			break
		elif [[ "$1" == "--unmount-local" ]]; then
			unmount_local
			break
		else
			print_err_exit "User must specify whether to mount or unmount the Time Machine volumes."
			break
		fi
	done
}

exec_equine "$@"
