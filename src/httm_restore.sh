#!/usr/bin/env bash

function httm_restore () {

	if ! command -v sk &> /dev/null; then
    	echo "sk, AKA skim, could not be found.  httm_restore depends on sk!"
    	exit
	fi
	
	local TMP_DIR=$( mktemp -d )

	if [[ -z $1 ]]; then 
		ls -1a | \
		sk --preview "httm {}" --preview-window=70% | \
		httm > $TMP_DIR/buf1
		cat $TMP_DIR/buf1 | sk -i > $TMP_DIR/buf2
	else
		httm $1 | sk -i > $TMP_DIR/buf2
	fi

	local FILE="$( cut -d'"' -f2 $TMP_DIR/buf2 )" && rm -rf $TMP_DIR	
	
	if [[ -z $FILE ]]; then
		echo "Error: You must select a file."
		exit 2
	fi

	if [[ ! -e $FILE ]]; then
		echo "Error: Selected file does not exist." 
		exit 2		
	fi
	
	local BASENAME="$(basename $FILE)"
	local MODIFY_TIME=$(date -r / "+%m-%d-%Y-%H:%M:%S") 
	local NEWNAME="$BASENAME.httm_restored.$MODIFY_TIME"
	local PWD="$(pwd)"

	if [[ $FILE == $PWD/$BASENAME ]]; then
		echo "Error: Will not restore files as files are the same file." 
		exit 2
	fi

	printf "httm will copy a local ZFS snapshot...\n\n"
	printf "	from: $FILE\n"
	printf "	to:   $PWD/$NEWNAME\n\n"
	
	read -p "Continue? (Y/N): " confirm && [[ $confirm == [yY] || $confirm == [yY][eE][sS] ]] || exit 1
	
	cp -R $FILE $PWD/$NEWNAME && printf "\nRestore completed successfully.\n"
}

httm_restore $1

