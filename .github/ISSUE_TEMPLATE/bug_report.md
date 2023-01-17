---
name: Bug report
about: Create a report to help us improve
title: ''
labels: ''
assignees: ''

---

**Before you begin -- Please RTFM**
Read the README and/or the help (`httm -h` or `man httm`).

A bug report which requests a fix for an issue which is already described within the README is a request for technical support, *not a bug report*, and will be treated as a low priority issue and may be summarily closed ("Answer is contained within the README.").

One example I've seen:

Bug Report: "`httm` doesn't appear to work with my btrfs layout..."

Answer contained within the README: "btrfs, by default, creates snapshots as the privileged user.  That may mean you will need to invoke `httm` with `sudo` or its equivalent.  httm will not fail if it does not have privileges to any particular snapshot directory."

**Is this actually a bug report?**
A bug report which demonstrates that a package is not installable via an unsupported method is probably a feature request, *not a bug report*, and may be summarily closed ("This is a feature request not a bug report.  Please submit via the feature request form.").

The supported install methods are *only* those contained within the README.  Note, `rpm` is an install method described in the README, and `rpm` is not `yum` or `dnf`.  If you can't install via `dnf` or `yum`, and you'd like to, that would be a feature request.  If your distribution/operating system uses an old or incompatible version of `rustc` or `cargo` or `libc`, and `httm` will, for some reason, not install, that is also probably a feature request.

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

**Desktop (please complete the following information):**
 - OS: [e.g. iOS]
 - Browser [e.g. chrome, safari]
 - Version [e.g. 22]

**Smartphone (please complete the following information):**
 - Device: [e.g. iPhone6]
 - OS: [e.g. iOS8.1]
 - Browser [e.g. stock browser, safari]
 - Version [e.g. 22]

**Additional context**
Add any other context about the problem here.
