---
name: Bug report
about: Create a report to help us improve
title: ''
labels: ''
assignees: ''

---

**Before you begin**
Before you begin:

1. Have you tried with the latest version of `httm`?
2. Have you read the README and/or the help (`httm -h` or `man httm`)?

    A bug report which requests a fix for an issue which is already described within the README is a request for technical support, *not a bug report*, and will be treated as low priority and may be summarily closed ("Answer is contained within the README.").  One example I've seen:

    Bug Report: "`httm` doesn't appear to work with my btrfs layout..."

    Answer contained within the README: "btrfs, by default, creates snapshots as the privileged user.  That may mean you will need to invoke `httm` with `sudo` or its equivalent.  httm will not fail if it does not have privileges to any particular snapshot directory."

3. Is this actually a bug report?

    A bug report which demonstrates that a package is not installable via an unsupported method is probably a feature request, *not a bug report*, and may also be summarily closed ("This is a feature request not a bug report.  Please submit via the feature request form.").

    The supported install methods are *only* those contained within the README.  Note, `rpm` is one install method described in the README, but `rpm` is not `yum` or `dnf`.  If you can't install via `dnf` or `yum` (you may be able to?), and you'd like to, that would be a feature request.  If your distribution/operating system uses an old or incompatible version of `rustc` or `cargo` or `libc`, and `httm` will not, for some reason, install, that is also probably a feature request.

**Describe the bug**
A clear and concise description of what the bug is.

**To Reproduce**
Steps to reproduce the behavior:
1. Go to '...'
2. Click on '....'
3. Scroll down to '....'
4. See error

**Expected behavior**
A clear and concise description of what you expected to happen.

**Screenshots**
If applicable, add screenshots to help explain your problem.

**Additional context**
Add any other context about the problem here.  Perhaps you should include system details like:

 - `httm` version
 - OS: [e.g. Ubuntu 22.04]
 - Relevant filesystem/s: [e.g. ZFS or btrfs]
 - `httm --debug` output if applicable
