[![asciicast](https://asciinema.org/a/WMf4IEAqqGuHSikUcpCe2kcbh.svg)](https://asciinema.org/a/WMf4IEAqqGuHSikUcpCe2kcbh)

# Don't call it a h-- t-- time machine

`httm` prints the size, date and corresponding locations of available unique versions (dedup-ed by modify time and size) of files residing on ZFS snapshots, as well as the *interactive* viewing and restoration of such files.

Inspired by the wonderful [findoid](https://github.com/jimsalterjrs/sanoid) but about twice as fast in the ordinary case.  *Ready and able* to be used in scripted interactive shell apps and widgets.

`httm` also boasts an array of seductive features, like:

* Search for deleted files! Ooooooooo!
* Select non-immediate datasets (on a different pool, or remote).
* For use with even rsync-ed non-ZFS local datasets (like ext4, APFS, or NTFS), not just ZFS.
* Fully native, interactive restore (no shell scripts needed, but you do you!)
* Select from several formatting styles.
* Parseable ... or not ...  oh my!

Use in combination with you favorite shell and a fuzzy finder like `sk` or `fzf` for even more fun.

## Installation

The `httm` project contains two components:

1. The `httm` executable: To install `git clone` this repo, and `cargo build` for right now.  On MacOS, you will have to [code-sign](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html) to use the remote capabilities.  Sorry kids!
3. The several outdated example scripts: To install just place somewhere in your PATH.  ~~ Depends upon `sk` or [skim](https://github.com/lotabout/skim) because that's my jam.~~  UPDATE: httm no longer depends on `skim`, as it now calls skim as a library, in *full* interactive mode.  

Look ma no hands -- no shell scripts needed for ZFS restore!

## License

httm is licensed under the MPL 2.0 License - see the LICENSE file for more details.