#!/usr/bin/env bash

function httm_restore () {

	TMP_DIR=$( mktemp -d )

	if [[ -z $1 ]]; then 
		ls -1a | \
		sk --preview "httm {}" --preview-window=70% | \
		httm > $TMP_DIR/buf1
		cat $TMP_DIR/buf1 | sk -i > $TMP_DIR/buf2
	else
		httm $1 | sk -i > $TMP_DIR/buf2
	fi

	FILE="$( cut -d'"' -f2 $TMP_DIR/buf2 )" && rm -rf $TMP_DIR
	
	if [[ -e "$FILE" ]]; then
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
	
	read -p "Continue? (Y/N): " confirm && [[ $confirm == [yY] || $confirm == [yY][eE][sS] ]] || echo "No restore made." && exit 1
	
	cp -r "$FILE" "$PWD/$NEWNAME"

}

httm_restore $1

