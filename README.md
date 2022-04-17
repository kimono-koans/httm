[![asciicast](https://asciinema.org/a/477019.svg)](https://asciinema.org/a/477019)

# zsys is *le sigh*, but the dream of a CLI ZFS Time Machine is still alive with `httm`

`httm` prints the size, date and corresponding locations of available unique versions (deduplicated by modify time and size) of files residing on ZFS snapshots, but can also be used *interactively* to view and restore such files.  `httm` might change the way you use ZFS snapshots (because ZFS isn't designed for finding for unique file versions) or the Time Machine concept (because `httm` is fast! -- about 60x faster than `findoid` on Linux).

`httm` boasts an array of seductive features, like:

* Search for and recursively list all deleted files! Ooooooooo!
* Select non-immediate datasets (on a different pool, or remote).
* For use with even `rsync`-ed non-ZFS local datasets (like ext4, APFS, or NTFS), not just ZFS.
* Specify multiple files for lookup on different datasets
* 3 native interactive modes: lookup, select and restore
* ANSI `ls` colors from your environment
* Non-blocking recursive directory walking (available in all interactive modes)
* Select from several formatting styles.  Parseable ... or not ...  oh my!

Use in combination with you favorite shell (hot keys!) for even more fun.

Inspired by the [findoid](https://github.com/jimsalterjrs/sanoid) script, [fzf](https://github.com/junegunn/fzf) and many [zsh](https://www.zsh.org) key bindings.

## Installation

The `httm` project contains only a few components:

1. The `httm` executable. To build and install:

    ```bash
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh 
    git clone kimono-koans/httm 
    cargo install --path ./httm/
    ```
2. The optional `zsh` hot-key bindings: Use `ESC+s` to select snapshots filenames to be dropped to your command line (for instance after the `cat` command), or use `ESC+m` to browse for all of a file's snapshots. After you install the `httm` binary -- to copy the hot key script to your home directory, and source that script within your zshrc:

    ```bash
    httm --install-zsh-hot-keys
    ```
3. The optional `man` page: `cargo` has no native facilities for man page installation (though it may in the future!).  You can use `manpath` to see the various directories your system uses and decide which directory works best for you.  To install, just copy it to a directory in your `man` path, like so:

    ```bash
    cp ./httm/httm.1 /usr/local/share/man/man1/
    ```

## License

httm is licensed under the MPL 2.0 License - see the LICENSE file for more details.