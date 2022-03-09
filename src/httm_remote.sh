#!/usr/bin/env bash

function httm_remote () {

	local DATASET="/Volumes/Home"
	local RELATIVE_DIR="/Users/<YOUR_NAME>"

	httm --mnt-point $DATASET --relative $RELATIVE_DIR "$@"
}

httm_remote "$@"