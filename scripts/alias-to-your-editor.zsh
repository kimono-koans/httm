#!/bin/zsh

# for the bible tells us so
set -euf -o pipefail

function ounce_of_prevention {
    # declare an array, convert to string later
    # this allows us to exec zfs snapshot once
    local -a FILENAMES_ARRAY

    for a; do
        # is the argument a file?
        if [[ -f "$a" ]]; then
            local LIVE_FILE="$a"
        else
            continue
        fi

        # get last snap version of the live file?
        local LAST_SNAP="$(httm -l "$LIVE_FILE")"

        # check whether to take snap - do we have a snap of the live file >
        # 1) if empty, live file does not have a snapshot, then take snap,>
        # 2) if live file is not the same as the last snap, then take snap
        if [[ -z "$LAST_SNAP" ]] || \
           [[ ! -z "$LAST_SNAP" && "$(stat -c %Y "$LIVE_FILE")" -ne "$(sta>
        then
           FILENAMES_ARRAY+=($( echo "$LIVE_FILE" ))
        fi
    done

    # check if filenames array is not empty
    if [[ ${FILENAMES_ARRAY[@]} ]]; then
      # httm will dynamically determine the location of
      # the file's ZFS dataset and snapshot that mount
      local FILENAMES_STRING="${FILENAMES_ARRAY[@]}"
      sudo httm --snap "$FILENAMES_STRING" > /dev/null &
    fi
}

ounce_of_prevention "$@"
# expressly used `nano` instead of `vim` or `emacs` to avoid a unholy war
/usr/bin/nano "$@"