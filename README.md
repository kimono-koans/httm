[![asciicast](https://asciinema.org/a/qrTvMMDerBwnCga3X26LLdM37.svg)](https://asciinema.org/a/qrTvMMDerBwnCga3X26LLdM37)

# httm is *not* h-- t-- time machine

`httm` prints the size, date and corresponding locations of available unique versions (dedup-ed by modify time and size) of files residing on ZFS snapshots.

Inspired by the wonderful [findoid](https://github.com/jimsalterjrs/sanoid) but about twice as fast in the ordinary case.  *Ready and able* to be used in scripted interactive shell apps and widgets.  See, for example `httm_restore.sh` in the src folder, which can interactively assist you in restoring a file from ZFS snapshots to your current working directory.

`httm` also boasts an array of seductive features, like:

* Search for deleted files! Ooooooooo!
* Select non-immediate datasets (on a different pool, or remote).
* Use with rsync-ed non-ZFS local datasets (like APFS), not just ZFS.
* Select from several formatting styles.
* Parseable ... or not ...  oh my!

Use in combination with a fuzzy finder like `sk` or `fzf` for even more fun.

## Installation

The `httm` project contains two components:

1. The `httm` executable: To install `git clone` this repo, and `cargo build` for right now.  On MacOS, you will have to [code-sign](https://developer.apple.com/library/archive/documentation/Security/Conceptual/CodeSigningGuide/Procedures/Procedures.html) to use the remote capabilities.  Sorry kids!
3. The `httm_restore.sh` script: To install just place somewhere in your PATH.  Depends upon `sk` or [skim](https://github.com/lotabout/skim) because that's my jam.

## License

httm is licensed under the MPL 2.0 License - see the LICENSE file for more details.