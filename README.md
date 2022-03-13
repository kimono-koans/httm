[![asciicast](https://asciinema.org/a/WMf4IEAqqGuHSikUcpCe2kcbh.svg)](https://asciinema.org/a/WMf4IEAqqGuHSikUcpCe2kcbh)

# Don't call it a h-- t-- time machine

`httm` prints the size, date and corresponding locations of available unique versions (deduplicated by modify time and size) of files residing on ZFS snapshots, as well as the *interactive* viewing and restoration of such files.

Inspired by the wonderful [findoid](https://github.com/jimsalterjrs/sanoid) but about twice as fast as `findoid` in non-interactive mode.

`httm` also boasts an array of seductive features, like:

* Search for deleted files! Ooooooooo!
* Select non-immediate datasets (on a different pool, or remote).
* For use with even rsync-ed non-ZFS local datasets (like ext4, APFS, or NTFS), not just ZFS.
* 3 native interactive modes: lookup, select and restore
* ANSI `ls` colors from your environment
* Non-blocking recursive directory walking (Can you hear my screams of pain?!  Can you celebrate my delight in that this is done??)
* Select from several formatting styles.
* Parseable ... or not ...  oh my!

Use in combination with you favorite shell and a fuzzy finder like `sk` or `fzf` for even more fun.

## Installation

The `httm` project contains two main components:

1. The `httm` executable: To build and install, simply, `git clone kimono-koans/httm`, and `cargo install --path ./httm/`.
2. The optional `zsh` hot-key bindings: Use `ESC+S` to select snapshots filenames to be dropped to your command line, or use `ESC+M` to browse for all of a file's snapshots.  

Note: The main functionality of `httm` is fully native and doesn't depend on `zsh` or any program other than your native `zfs-utils` but *use it how you want to*, as example scripts and key bindings are provided.

## License

httm is licensed under the MPL 2.0 License - see the LICENSE file for more details.