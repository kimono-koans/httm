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
nicotine $(httm --version | cut -f2 -d' ')
" 1>&2
	exit 0
}

print_usage() {
	local nicotine="\e[31mnicotine\e[0m"
	local httm="\e[31mhttm\e[0m"

	printf "\
$nicotine is a wrapper script for $httm which converts unique file versions on snapshots to a git archive.

USAGE:
	nicotine [OPTIONS]... [file1 file2...]

OPTIONS:
	--output-dir:
		Select the output directory.
	--debug:
		Show git and tar command output.
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
		command -v git
		exit 0
	)" ]] || print_err_exit "'git' is required to execute 'nicotine'.  Please check that 'git' is in your path."
	[[ -n "$(
		command -v tar
		exit 0
	)" ]] || print_err_exit "'tar' is required to execute 'nicotine'.  Please check that 'targit' is in your path."
	[[ -n "$(
		command -v mktemp
		exit 0
	)" ]] || print_err_exit "'mktemp' is required to execute 'nicotine'.  Please check that 'mktemp' is in your path."
	[[ -n "$(
		command -v mkdir
		exit 0
	)" ]] || print_err_exit "'mkdir' is required to execute 'nicotine'.  Please check that 'mkdir' is in your path."
	[[ -n "$(
		command -v httm
		exit 0
	)" ]] || print_err_exit "'httm' is required to execute 'nicotine'.  Please check that 'httm' is in your path."
}

function convert2git {
	local debug=$1
	shift
	local working_dir="$1"
	shift
	local tmp_dir="$1"
	shift
	local output_dir="$1"
	shift
	local files="$@"

	local archive_dir="$tmp_dir"

	# copy each version to repo and commit after each copy
	for file in $files; do
		# sanity -- check if file exists
		if [[ ! -e "$file" ]]; then
			printf "$file does not exist. Skipping.\n"
			continue
		fi

		# tar will not create an archive using an empty dir
		if [[ -d "$file" ]] && [[ -n "$( find "$file" -maxdepth 0 -type d -empty )" ]]; then
			printf "$file is an empty directory. Skipping.\n"
			continue
		fi

		# create dir for file
		archive_dir="$tmp_dir/$(basename $file)"
		mkdir "$archive_dir" || print_err_exit "nicotine could not create a temporary directory.  Check you have permissions to create."
		cd "$archive_dir" || print_err_exit "nicotine could not enter a temporary directory.  Check you have permissions to enter."

		# create git repo
		if [[ $debug = true ]]; then
			git init || print_err_exit "git could not initialize directory"
		else
			git init -q >/dev/null || print_err_exit "git could not initialize directory"
		fi

		# copy, add, and commit to git repo in loop
		local -a list=( $( httm -n --omit-ditto "$file" 2>/dev/null || exit 0 ) )

		if [[ ${#list[@]} -ne 0 ]]; then
			for version in $list; do
				cp -aR "$version" "$archive_dir/"

				if [[ $debug = true ]]; then
					git add --all "$archive_dir/$(basename $version)"
					git commit -m "httm commit from ZFS snapshot" --date "$(date -d "$(stat -c %y $version)")"
				else
					git add --all "$archive_dir/$(basename $version)" > /dev/null
					git commit -q -m "httm commit from ZFS snapshot" --date "$(date -d "$(stat -c %y $version)")" > /dev/null
				fi
			done
		else
				cp -aR "$file" "$archive_dir/"

				if [[ $debug = true ]]; then
					git add --all "$archive_dir/$(basename $file)"
					git commit -m "httm commit from ZFS snapshot" --date "$(date -d "$(stat -c %y $file)")"
				else
					git add --all "$archive_dir/$(basename $file)" > /dev/null
					git commit -q -m "httm commit from ZFS snapshot" --date "$(date -d "$(stat -c %y $file)")" > /dev/null
				fi
		fi

		# creat archive
		local output_file="$output_dir/$(basename $file)-snapshot-archive.tar.gz"

		cd "$tmp_dir"

		if [[ $debug = true ]]; then
			tar -zcvf "$output_file" "./$(basename $file)" || print_err_exit "tar.gz archive creation failed.  Quitting."
		else
			tar -zcvf "$output_file" "./$(basename $file)" > /dev/null || print_err_exit "tar.gz archive creation failed.  Quitting."
		fi

		printf "nicotine archive created successfully: $output_file\n"
		archive_dir="$tmp_dir"

		cd "$working_dir"
	done
}

function nicotine {
	# do we have commands to execute?
	prep_exec

	local debug=false
	local output_dir="$( pwd )"
	local working_dir="$( pwd )"

	[[ $# -ge 1 ]] || print_usage
	[[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
	[[ "$1" != "-V" && "$1" != "--version" ]] || print_version

	while [[ $# -ge 1 ]]; do
		if [[ "$1" == "--output-dir" ]]; then
			shift
			[[ $# -ge 1 ]] || print_err_exit "output-dir argument is empty"
			output_dir="$1"
			shift
		elif [[ "$1" == "--debug" ]]; then
			debug=true
			shift
		else
			break
		fi
	done

	local tmp_dir="$( mktemp -d )"
	trap "[[ ! -d "$tmp_dir" ]] || rm -rf "$tmp_dir"" EXIT

	[[ -n "$tmp_dir" ]] || print_err_exit "Could not create a temporary directory for scratch work.  Quitting."
	[[ -n "$output_dir" ]] || print_err_exit "Could not determine the current working directory.  Quitting."

	convert2git $debug "$working_dir" "$tmp_dir" "$output_dir" "$@"
}

nicotine "$@"