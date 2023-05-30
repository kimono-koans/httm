[![asciicast](https://asciinema.org/a/529608.svg)](https://asciinema.org/a/529608)

# `httm`

*The dream of a CLI Time Machine is still alive with `httm`.*

`httm` prints the size, date and corresponding locations of available unique versions (deduplicated by modify time and size) of files residing on snapshots, but can also be used *interactively* to select and restore files, even snapshot mounts by file! `httm` might change the way you use snapshots (because ZFS/BTRFS/NILFS2 aren't designed for finding for unique file versions) or the Time Machine concept (because `httm` is very fast!).

`httm` boasts an array of seductive features, like:

* Search for versions of multiple files on distinct datasets simultaneously
* Search for and recursively list deleted files.  *Even browse files hidden behind deleted directories*.
* List file snapshots from *all* local pools (detect local snapshot versions *as well as* locally replicated snapshot versions)!
* List file snapshots from remote backup pools (even overlay replicated remote snapshot directories over live directories).
* For use with `rsync`-ed non-ZFS/BTRFS/NILFS2 local datasets (like ext4, APFS, or NTFS), not just ZFS/BTRFS/NILFS2.
* Optionally preview snapshot file versions with a custom command (default is a `diff` compare to the live version)
* List or even snapshot the mounts for a file directly
* Roll *forward* to a previous snapshots, instead of rolling back (avoids destroying interstitial snapshots)
* Guard any restore actions with precautionary snapshots
* List snapshot names, even purge snapshots, which include a file
* Shortcut features: only display last snapshot, omit duplicates of the live file, etc.
* Uniqueness level: Like `rsync`, `httm` can determine whether file is unique based solely on metadata, or use checksums
* 4 native interactive modes: browse, select, purge and restore
* ANSI `ls` colors from your environment
* Detect and display only categories of the numbers of unique file versions available (`multiple`, `single`, `single-with-snap`,..., etc.)
* Select from several formatting styles (newline, null, tab delimited, JSON, etc.).  Parseable ... or not ...  oh my!
* Packaged scripts which help you, and show you how to, use `httm`: [ounce](https://github.com/kimono-koans/httm/blob/master/scripts/ounce.bash), [bowie](https://github.com/kimono-koans/httm/blob/master/scripts/bowie.bash), and [nicotine](https://github.com/kimono-koans/httm/blob/master/scripts/nicotine.bash)
* Supports ZFS/BTRFS/NILFS2 snapshots

Use in combination with you favorite shell's hot keys for even more fun.

Inspired by the [findoid](https://github.com/jimsalterjrs/sanoid) script, [fzf](https://github.com/junegunn/fzf) and many [zsh](https://www.zsh.org) key bindings.

## Install via Native Packages

For Debian-based distributions (like Ubuntu), I maintain an (unsupported) personal package archive, or PPA.  See the [linked repository](https://github.com/kimono-koans/ppa).

For Debian-based and Redhat-based Linux distributions (like Ubuntu or Fedora, etc.), also check the [tagged releases](https://github.com/kimono-koans/httm/tags) for native packages for your distribution.  For Redhat-based Linux distributions, you may need to use the `--replacefiles` option when installing via `rpm -i`, see the linked [issue](https://github.com/kimono-koans/httm/issues/51).

You may also create and install your own native package from the latest sources, for Debian-based and Redhat-based Linux distributions, like so:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
cargo install cargo-deb 
git clone https://github.com/kimono-koans/httm.git
cd ./httm/; cargo deb
# to install on a Debian/Ubuntu-based system
dpkg -i ./target/debian/httm_*.deb
# or convert to RPM 
alien -r ./target/debian/httm_*.deb
# and install on a Redhat-based system
rpm -i --replacefiles ./httm_*.rpm
```

For Arch-based Linux distributions, you can create and install your own native package from the latest sources, like so:

```bash
# you need to edit the PKGBUILD as needed to conform to the latest release
wget https://raw.githubusercontent.com/kimono-koans/httm/master/packaging/arch/PKGBUILD
makepkg -si
```

For MacOS users, a user from the community (very exciting) has posted a [formulae](https://formulae.brew.sh/formula/httm) for [Homebrew](https://brew.sh).

## Install via Source

The `httm` project contains only a few components:

1. The `httm` executable.  To build and install:

    ```bash
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
    latest="(wget -nv -O - "https://api.github.com/repos/kimono-koans/httm/releases/latest" 2>/dev/null | grep tag_name | cut -d: -f2 | cut -d'"' -f2)"
    cargo install --git https://github.com/kimono-koans/httm.git --tag "$latest"
    ```
2. The optional `zsh` hot-key bindings: Use `ESC+s` to select snapshots filenames to be dropped to your command line (for instance after the `cat` command), or use `ESC+m` to browse for all of a file's snapshots. After you install the `httm` binary, to copy the hot key script to your home directory, and source that script within your `.zshrc`:

    ```bash
    httm --install-zsh-hot-keys
    ```
3. The optional `man` page: `cargo` has no native facilities for man page installation (though it may in the future!).  You can use `manpath` to see the various directories your system uses and decide which directory works best for you.  To install, just copy it to a directory in your `man` path, like so:

    ```bash
    cp ./httm/httm.1 /usr/local/share/man/man1/
    ```
4. The optional  scripts.  See the scripts ([ounce](https://github.com/kimono-koans/httm/blob/master/scripts/ounce.bash), [bowie](https://github.com/kimono-koans/httm/blob/master/scripts/bowie.bash), and [nicotine](https://github.com/kimono-koans/httm/blob/master/scripts/nicotine.bash)) themselves, and below, in the Example Usage section or follow the links, for more information.  To install, just copy it to a directory in your path, like so:

    ```bash
    cp ./httm/scripts/ounce.bash /usr/local/bin/ounce
    cp ./httm/scripts/bowie.bash /usr/local/bin/bowie
    cp ./httm/scripts/nicotine.bash /usr/local/bin/nicotine
    chmod +x /usr/local/bin/bowie /usr/local/bin/ounce /usr/local/bin/nicotine
    ```

### Caveats

Right now, you will need to use a Unix-ish-y Rust-supported platform to build and install (that is: Linux, Solaris/illumos, the BSDs).  Note, your platform *does not* need to support ZFS/BTRFS/NILFS2 to use `httm`.  And there is no fundamental reason a non-interactive Windows version of `httm` could not be built, as it once did build, but Windows platform support is not a priority for me right now.  Contributions from users are, of course, very welcome.

On FreeBSD, after a fresh minimal install, the interactive modes may not render properly, see the linked [issue](https://github.com/kimono-koans/httm/issues/20) for the fix.

On some Linux distributions, which include old versions of `libc`, `cargo` may require building with `musl` instead, see the linked [issue](https://github.com/kimono-koans/httm/issues/17).

## Example Usage

Note: Users may need to use `sudo` (or equivalent) to view versions on BTRFS or NILFS2 datasets, as BTRFS or NILFS2 snapshots may require root permissions in order to be visible.

Like other UNIX utilities (such as `cat`, `uniq`, `sort`), if you include no path/s as arguments, then `httm` will pause waiting for input on stdin:
```bash
# Press CTRL+C to send a SIGINT and quit the program
➜ httm 
# Pipe output of find command to httm
➜ find . -maxdepth 1 | httm
```
Print all unique versions of your history file:
```bash
➜ httm ~/.histfile
```
Print all unique versions of your history file, as formatted JSON:
```bash
➜ httm --json ~/.histfile
```
Print all files on snapshots deleted from your home directory, recursive:
```bash
➜ httm -d -R ~
```
Print all files on snapshots deleted from your home directory, recursive, newline delimited, piped to a text file:
```bash
# pseudo live file versions
➜ httm -d -n -R --no-snap ~ > pseudo-live-versions.txt
# unique snapshot versions
➜ httm -d -n -R --no-live ~ > deleted-unique-versions.txt
```
Browse all files in your home directory, recursively, and view unique versions on local snapshots:
```bash
➜ httm -i -R ~
```
Browse all files deleted from your home directory, recursively, and view unique versions on all local and alternative replicated dataset snapshots:
```bash
➜ httm -d=only -i -a -R ~
```
Browse all files in your home directory, recursively, and view unique versions on local snapshots, to select and ultimately restore to your working directory:
```bash
➜ httm -r -R ~
```
View unique versions of a file for recovery (shortcut, no need to browse a directory):
```bash
➜ httm -r /var/log/samba/log.smbd
```
View `bowie`-formatted `diff` of each unique snapshot of `~/.zshrc` against the live file version:
```bash
➜ httm --preview -s ~/.zshrc
```
View `cat` output of each unique snapshot of `~/.zshrc`:
```bash
➜ httm --preview="cat {snap_file}" -s ~/.zshrc
```
Recover the last-in-time unique file version (shortcut, no need to browse a directory or select from among other unique versions):
```bash
➜ httm -l -r /var/log/samba/log.smbd
```
Snapshot the dataset upon which `/etc/samba/smb.conf` is located:
```bash
➜ sudo httm -S /etc/samba/smb.conf
``` 
Browse all files, recursively, in a folder backed up via `rsync` to a remote share, and view unique versions on remote snapshots directly (only available for BTRFS Snapper and ZFS datasets).  
```bash
# mount the share
➜ open smb://<your name>@<your remote share>.local/Home
# execute httm
➜ httm -i -R /Volumes/Home
```
Browse all files, recursively, in your MacOS home directory backed up via `rsync` to a ZFS or BTRFS Snapper remote share, shared via `smbd`, and view unique versions on remote snapshots. Note: The difference from above is, here, you're browsing files from a "live" directory:
```bash
# mount the share
➜ open smb://<your name>@<your remote share>.local/Home
# execute httm
➜ httm -i -R --map-aliases /Users/<your name>:/Volumes/Home ~
```
View the differences between each unique snapshot version of the `httm` `man` page and each previous version (this simple script is the basis for [bowie](https://github.com/kimono-koans/httm/blob/master/scripts/bowie.bash)):
```bash
filename="./httm/httm.1"
# previous version is unset
previous_version=""
for current_version in $(httm -n --omit-ditto $filename); do
    # check if initial "previous_version" needs to be set
    if [[ -z "$previous_version" ]]; then
        previous_version="$current_version"
        continue
    fi

    # print that current version and previous version differ
    diff -q "$previous_version" "$current_version"
    # print the difference between that current version and previous_version
    diff "$previous_version" "$current_version"

    # set current_version to previous_version
    previous_version="$current_version"
done
```
Create a simple `tar` archive of all unique versions of your `/var/log/syslog`:
```bash
➜ httm -n --omit-ditto /var/log/syslog | tar -zcvf all-versions-syslog.tar.gz -T -
```
Create a *kinda fancy* `tar` archive of all unique versions of your `/var/log/syslog`:
```bash
file="/var/log/syslog"
dir_name="${$(dirname $file)/\//}"
base_dir="$(basename $file)_all_versions"
# squash extra directories by "transforming" them to simply snapshot names 
httm -n --omit-ditto "$file" | \
tar \
--transform="flags=r;s|$dir_name|$base_dir|" \
--transform="flags=r;s|.zfs/snapshot/||" \
--show-transformed-names \
-zcvf "all-versions-$(basename $file).tar.gz" -T  -
```
Create a *super fancy* `git` archive of all unique versions of `/var/log/syslog` (this simple script is the basis for [nicotine](https://github.com/kimono-koans/httm/blob/master/scripts/nicotine.bash)):
```bash
# create variable for file name
file="/var/log/syslog"
# create git repo
mkdir ./archive-git; cd ./archive-git; git init
# copy each version to repo and commit after each copy
for version in $(httm -n --omit-ditto $file); do
    cp "$version" ./
    git add "./$(basename $version)"
    # modify commit date to match snapshot modify date-time
    git commit -m "httm commit from ZFS snapshot" \
    --date "$(date -d "$(stat -c %y $version)")"
done
# create git tar.gz archive
tar -zcvf "../all-versions-$(basename $file).tar.gz" "./"
# and to view
git log --stat
```
Use [ounce](https://github.com/kimono-koans/httm/blob/master/scripts/ounce.bash) (codename: "dimebag"), a wrapper script for `httm`, for no mental overhead, non-periodic dynamic snapshots:
```bash
# request ZFS snapshot privileges
➜ ounce --give-priv
# here you create a "dummyfile", ounce will add a snapshot of "dummyfile" 
# before you remove it, and httm will allow you to view the snapshot created
➜ touch ~/dummyfile; ounce rm ~/dummyfile; httm ~/dummyfile
────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
Wed Feb 15 12:59:42 2023  0 bytes  "/home/kimono/.zfs/snapshot/snap_2023-02-15-12:59:42_ounceSnapFileMount/dummyfile"
────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
                                   "/home/kimono/dummyfile"
────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
# use as an alias around programs which modify files/dirs
➜ printf "
# ounce aliases 
alias vim=\"ounce --trace vim\"
alias emacs=\"ounce --trace emacs\"
alias nano=\"ounce --trace nano\"
alias rm=\"ounce rm\"" >> ~/.zsh_aliases
```
Use [bowie](https://github.com/kimono-koans/httm/blob/master/scripts/bowie.bash), a wrapper script for `httm`, to display the difference between unique snapshot versions and the live file:
```bash
➜ bowie ~/.zshrc
/home/kimono/.zshrc
__
Files /home/kimono/.zfs/snapshot/snap_2023-02-14-13:42:11_ounceSnapFileMount/.zshrc and /home/kimono/.zshrc differ
1c1
<	### If you come from bash you might have to change your $PATH.
---
>	# If you come from bash you might have to change your $PATH.
```
Use [nicotine](https://github.com/kimono-koans/httm/blob/master/scripts/nicotine.bash), a wrapper script for `httm`, to convert unique snapshot file versions to `git` archives:
```bash
➜ nicotine .zshrc
nicotine git archive created successfully: /home/kimono/zshrc-git.tar.gz
```
Roll *forward* to a previous ZFS snapshot, instead of rolling back (this avoids destroying interstitial snapshots):
```bash
➜ sudo httm --roll-forward=rpool/scratch@snap_2023-04-01-15:26:06_httmSnapFileMount
[sudo] password for kimono:
httm took a pre-execution snapshot named: rpool/scratch@snap_pre_2023-04-01-15:27:38_httmSnapRollForward
...
httm roll forward completed successfully.
httm took a post-execution snapshot named: rpool/scratch@snap_post_2023-04-01-15:28:40_:snap_2023-04-01-15:26:06_httmSnapFileMount:_httmSnapRollForward
```

## Yo, @kimono-koans, where do your snapshots come from?

If you'd like to read more about how someone else/I personally create snapshots, you might try my [A Somewhat Opinionated Guide to Effective ZFS Snapshots](https://kimono-koans.github.io/opinionated-guide/).

## I know what you're thinking, but slow your roll

![To be clear, httm is *not*...](https://i.pinimg.com/originals/23/7f/2a/237f2ab8765663c721325366406197b7.gif)

## License

httm is licensed under the MPL 2.0 License - see the LICENSE file for more details.