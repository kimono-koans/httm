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

## Note: env is zsh/bash here but could maybe/should work in zsh/bash too? ##

set -euf -o pipefail
#set -x

function print_err_exit {
	print_err "$*"
	exit 1
}

function print_err {
	printf "%s\n" "ERROR: $*" 1>&2
}

function print_info {
	printf "%b\n" "$*"
}

function prep_exec {
	[[ -n "$(
		command -v mkdir
		exit 0
	)" ]] || print_err_exit "'mkdir' is required to execute 'install-from-source.bash'.  Please check that 'mkdir' is in your path."
	[[ -n "$(
		command -v install
		exit 0
	)" ]] || print_err_exit "'install' is required to execute 'install-from-source.bash'.  Please check that 'install' is in your path."
	[[ -n "$(
		command -v bash
		exit 0
	)" ]] || print_err_exit "'bash' is required to execute 'install-from-source.bash'.  Please check that 'bash' is in your path."
	[[ -n "$(
		command -v cargo
		exit 0
	)" ]] || print_err_exit "'cargo' is required to execute 'install-from-source.bash'.  Please check that 'cargo' is in your path."
	[[ -n "$(
		command -v git
		exit 0
	)" ]] || print_err_exit "'git' is required to execute 'install-from-source.bash'.  Please check that 'git' is in your path."
}

function prep_sudo {
	local sudo_program=""

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
		print_err_exit "'sudo'-like program is required to execute.  Please check that 'sudo' (or 'doas' or 'pkexec') is in your path."

	printf "%s" "$sudo_program"
}

function build_httm() {
	print_info "Building httm..."

	[[ ! -d "./httm" ]] || rm -rf "./httm"
	git clone https://github.com/kimono-koans/httm.git

	cd httm
	cargo build --release --locked --features "std"
	cd - 2>&1 > /dev/null
}

function install_httm() {
	local sudo_program=""
	sudo_program="$1"
	[[ -n "$sudo_program" ]] || print_err_exit "sudo-like program is required to execute."

	[[ -d "./httm" ]] || print_err_exit "Working build path does not exist."

	print_info "Installing httm..."

	cd httm

	$sudo_program mkdir -p /usr/local/bin && $sudo_program mkdir -p /usr/local/share/doc/httm && $sudo_program mkdir -p /usr/local/share/licenses/httm

	# install executable
	$sudo_program install -vDm755 "target/release/httm" "/usr/local/bin/httm"

	# install bowie script
	$sudo_program install -vDm755 "scripts/bowie.bash" "/usr/local/bin/bowie"

	# install ounce script
	$sudo_program install -vDm755 "scripts/ounce.bash" "/usr/local/bin/ounce"

	# install nicotine script
	$sudo_program install -vDm755 "scripts/nicotine.bash" "/usr/local/bin/nicotine"

	# install equine script
	$sudo_program install -vDm755 "scripts/equine.bash" "/usr/local/bin/equine"

	# install man page
	$sudo_program install -vDm644 "httm.1" "/usr/local/share/man/man1/httm.1"

	# install README.md
	$sudo_program install -vDm644 "README.md" "/usr/local/share/doc/httm/README.md"

	# install LICENSE
	$sudo_program install -vDm644 "LICENSE" "/usr/local/share/licenses/httm/LICENSE"

	cd - 2>&1 > /dev/null
}

function run_install() {
	prep_exec
	local sudo_program="$( prep_sudo )"
	[[ -n "$sudo_program" ]] || print_err_exit "sudo-like program is required to execute."

    trap "[[ ! -d "./httm" ]] || rm -rf "./httm"" EXIT

	build_httm
	install_httm $sudo_program

}

run_install