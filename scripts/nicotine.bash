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

print_version() {
	printf "\
nicotine $(httm --version | cut -f2 -d' ')
" 1>&2
	exit 0
}

print_usage() {
	local nicotine="\e[31mnicotine\e[0m"
	local httm="\e[31mhttm\e[0m"
	local git="\e[31mgit\e[0m"
	local tar="\e[31mtar\e[0m"

	printf "\
$nicotine is a wrapper script for $httm which converts unique snapshot file versions to a $git archive.

USAGE:
	nicotine [OPTIONS]... [file1 file2...]

OPTIONS:
	--output-dir:
		Select the output directory.  Default is the current working directory.
	--no-archive
		Disable archive creation.  Create a new $git respository (e.g. named \"\$file1-git\") 
		in the output directory.
	--debug:
		Show $git and $tar command output.  Default is to complete silence both.
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
	[[ -n "$(
		command -v find
		exit 0
	)" ]] || print_err_exit "'find' is required to execute 'nicotine'.  Please check that 'find' is in your path."
	[[ -n "$(
		command -v readlink
		exit 0
	)" ]] || print_err_exit "'readlink' is required to execute 'nicotine'.  Please check that 'readlink' is in your path."
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

function traverse {
	local debug=$1
	shift
	local path="$1"
	shift
	local dest_dir="$1"
	shift

	local -a dir_entries=()
 
	while read -r line; do
		dir_entries+=("$line")
	done <<<"$(find "$path" -not -wholename "./.git*" -print0 | httm --omit-ditto --not-so-pretty | cut -f1,3 | sort)"

	# return early - empty dir
	[[ ${#dir_entries[@]} -ne 0 ]] || return 0

	local previous_date=""

	for dated_entry in "${dir_entries[@]}"; do
		local date="$(echo "$dated_entry" | cut -f1)"
		local entry="$(echo "$dated_entry" | cut -f2)"
		local relative_path="$(realpath --relative-to="$path" "$entry")"

		if [[ "$relative_path" == *".zfs"* ]]; then
  			relative_path="$(echo $relative_path | cut -d'/' -f5)"
		fi

		local destination="$dest_dir/$relative_path"
		local parent="$(dirname "$destination")"

		if [[ -d "$entry" ]]; then
			cp -a "$entry" "$parent/"
		else
			mkdir -p "$parent"
			cp -a "$entry" "$destination"
		fi

		if [[ $debug = true ]]; then
			git add --all "$destination" || true
		else
			git add --all "$destination" > /dev/null || true
		fi

		if [[ -z "$previous_date" ]]; then
			previous_date="$date"
    	fi

		if [[ "$previous_date" != "$date" ]]; then
			if [[ $debug = true ]]; then
				git commit -m "httm commit from ZFS snapshot" --date "$(date -d "$(stat -c %y "$destination")")" || true
			else
				git commit -q -m "httm commit from ZFS snapshot" --date "$(date -d "$(stat -c %y "$destination")")" || true
			fi
		fi

		previous_date="$date"
	done
}

function convert_to_git {
	local debug=$1
	shift
	local no_archive=$1
	shift
	local working_dir="$1"
	shift
	local tmp_dir="$1"
	shift
	local output_dir="$1"
	shift
	local path="$1"
	shift

	local archive_dir=""
	local basename=""

	# create dir for file
	raw="$(basename "$path")"
	# Use parameter expansion to remove leading dot: "${FILENAME#.}"
	basename="${raw#.}"
	
	# git requires a dir to init
	archive_dir="$tmp_dir/$basename"
	mkdir "$archive_dir" || print_err_exit "nicotine could not create a temporary directory.  Check you have permissions to create."
	
	# ... and we must enter the dir to have git work
	cd "$archive_dir" || print_err_exit "nicotine could not enter a temporary directory: $archive_dir.  Check you have permissions to enter."

	# create git repo
	if [[ $debug = true ]]; then
		git init || print_err_exit "git could not initialize directory"
	else
		git init -q >/dev/null || print_err_exit "git could not initialize directory"
	fi

	# copy, add, and commit to git repo in loop
	# why branch? because git requires a dir and
	# for files we create a dir specifically for the file
	if [[ -d "$path" ]]; then
		traverse $debug "$path" "$tmp_dir"
	else
		traverse $debug "$path" "$archive_dir"
	fi

	if [[ $no_archive = true ]]; then
		cp -ra "$archive_dir" "$output_dir/$basename-git"
	else
		# tar works with relative paths, so make certain we are in our base tmp_dir
		cd "$tmp_dir"

		# create archive
		local output_file="$output_dir/$basename-git.tar.gz"

		if [[ $debug = true ]]; then
			tar -zcvf "$output_file" "./$basename" || print_err_exit "Archive creation failed.  Quitting."
		else
			tar -zcvf "$output_file" "./$basename" > /dev/null || print_err_exit "Archive creation failed.  Quitting."
		fi
	fi
	
	# cleanup safely
	[[ ! -e "$tmp_dir/$basename" ]] || rm -rf "$tmp_dir/$basename"
	[[ -e "$tmp_dir/$basename" ]] || rm -rf "$tmp_dir/*"

	if [[ $no_archive = true ]]; then
		printf "nicotine git repository created successfully: $basename-git\n"			
	else
		printf "nicotine git archive created successfully: $output_file\n"
	fi
}

function nicotine {
	# do we have commands to execute?
	prep_exec

	local debug=false
	local no_archive=false
	local working_dir
	local output_dir

	working_dir="$(readlink -e "$( pwd )" 2>/dev/null || true)"
	[[ -e "$working_dir" ]] || print_err_exit "Could not obtain current working directory.  Quitting."
	output_dir="$working_dir"

	[[ $# -ge 1 ]] || print_usage
	[[ "$1" != "-h" && "$1" != "--help" ]] || print_usage
	[[ "$1" != "-V" && "$1" != "--version" ]] || print_version

	while [[ $# -ge 1 ]]; do
		if [[ "$1" == "--output-dir" ]]; then
			shift
			[[ $# -ge 1 ]] || print_err_exit "output-dir argument is empty"
			output_dir="$(readlink -e "$1" 2>/dev/null || true)"
			[[ -e "$output_dir" ]] || print_err_exit "Could not obtain output directory from path given.  Quitting."
			shift
		elif [[ "$1" == "--debug" ]]; then
			shift
			debug=true
		elif [[ "$1" == "--no-archive" ]]; then
			shift
			no_archive=true
		else
			break
		fi
	done

	local tmp_dir="$( mktemp -d )"
	trap "[[ ! -d "$tmp_dir" ]] || rm -rf "$tmp_dir"" EXIT
	[[ -e "$tmp_dir" ]] || print_err_exit "Could not create a temporary directory for scratch work.  Quitting."
	
	[[ $# -ne 0 ]] || print_err_exit "User must specify at least one input file.  Quitting."

	for a; do
		canonical_path="$(
			readlink -e "$a" 2>/dev/null
			[[ $? -eq 0 ]] || print_err "Could not determine canonical path for: $a"
		)"
		[[ -n "$canonical_path" ]] || continue

		# check if file exists
		if [[ ! -e "$canonical_path" ]]; then
			printf "$canonical_path does not exist. Skipping.\n"
			continue
		fi

		# ... and tar will not create an archive for an empty dir
		if [[ -d "$canonical_path" ]] && [[ -z "$(find "$canonical_path" -mindepth 1 -maxdepth 1)" ]]; then
			printf "$canonical_path is an empty directory. Skipping.\n"
			continue
		fi

		# if not a file, directory, or symlink, we can't use so also skip
		if [[ ! -f "$canonical_path" && ! -d "$canonical_path" && ! -L "$canonical_path" ]]; then
			printf "$canonical_path is not a file, directory, or symlink. Skipping.\n"
			continue		
		fi

		convert_to_git $debug $no_archive "$working_dir" "$tmp_dir" "$output_dir" "$canonical_path"
	done

}

nicotine "$@"