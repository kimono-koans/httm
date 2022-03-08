[![asciicast](https://asciinema.org/a/qrTvMMDerBwnCga3X26LLdM37.svg)](https://asciinema.org/a/qrTvMMDerBwnCga3X26LLdM37)

# httm is not a h-- t-- time machine 

`httm` prints size, date and corresponding locations of available versions of files residing on snapshots.

Inspired by the wonderful [findoid](https://github.com/jimsalterjrs/sanoid) but about twice as fast in the ordinary case.  *Ready and able* to be used in scripted interactive shell apps and widgets.  See, for example `httm_restore.sh` in the src folder.

It also boasts an array of features, like:

* Ability to search for deleted files! Ooooooooo!
* Select from several formatting styles!
* Parseable ... or not ...  oh my!

Use in combination with a fuzzy finder like `sk` or `fzf` for even more fun.

## Installation

The `httm` project contains two components:

1. The `httm` executable: To install `git clone` this repo, and `cargo build` for right now.  Sorry kids!
3. The `httm_restore.sh` script: To install just place somewhere in your PATH.  Depends upon `sk` or [skim](https://github.com/lotabout/skim) because that's my jam.

## License

httm is licensed under the MPL 2.0 License - see the LICENSE file for more details.


