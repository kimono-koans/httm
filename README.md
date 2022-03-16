[![asciicast](https://asciinema.org/a/477019.svg)](https://asciinema.org/a/477019)

# Don't call it a h-- t-- time machine

`httm` prints the size, date and corresponding locations of available unique versions (deduplicated by modify time and size) of files residing on ZFS snapshots, but can also be used to the *interactively* view and restore such files.

`httm` might change the way you use ZFS snapshots (because searching for unique file versions can be a chore) or the Time Machine concept (because `httm` is actually fast!).

Inspired by the wonderful [findoid](https://github.com/jimsalterjrs/sanoid) but is about twice as fast in non-interactive mode.

`httm` also boasts an array of seductive features, like:

* Search for deleted files! Ooooooooo!
* Select non-immediate datasets (on a different pool, or remote).
* For use even with rsync-ed non-ZFS local datasets (like ext4, APFS, or NTFS), not just ZFS.
* 3 native interactive modes: lookup, select and restore
* ANSI `ls` colors from your environment
* Non-blocking recursive directory walking (available in all interactive modes)
* Select from several formatting styles.  Parseable ... or not ...  oh my!

Use in combination with you favorite shell (hot keys!) for even more fun.

## Installation

The `httm` project contains two main components:

1. The `httm` executable: To build and install, simply:
    ```bash
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh 
    git clone kimono-koans/httm 
    cargo install --path ./httm/
    ```
2. The optional `zsh` hot-key bindings: Use `ESC+S` to select snapshots filenames to be dropped to your command line, or use `ESC+M` to browse for all of a file's snapshots.  

Note: The main functionality of `httm` is fully native and doesn't depend on `zsh` or any program other than your native `zfs-utils` but *use it how you would*.  Further example scripts and key bindings are provided as well.

## License

httm is licensed under the MPL 2.0 License - see the LICENSE file for more details.