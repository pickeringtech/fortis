# Security policy

## Reporting a vulnerability

Please report a vulnerability privately, through GitHub's private vulnerability reporting: open the repository's
**Security** tab and choose **Report a vulnerability**. Never report one in a public issue, a pull request or a
discussion, because that tells everyone before there is a fix.

A useful report says which version or commit you used, what you did, what happened, and what you expected to
happen. A small program that shows the problem helps most. We will acknowledge your report, keep you informed
while we work on it, and credit you when the fix is published, unless you would rather not be named.

## Supported versions

Only the latest release receives security fixes. If you find a problem in an older release, please check whether
the latest one still has it before you report it.

## What counts

Anything that breaks a guarantee Fortis makes is in scope. For example:

- a program that the checkers accept, but that breaks ownership, effects or failure handling when it runs;
- a way for checked code to reach a capability, a file or the network without the authority the language requires;
- the native backend producing Rust that is unsafe or unsound. The backend emits `#![forbid(unsafe_code)]` Rust, so
  generated code that gets around that is a vulnerability;
- a command that only reads source (`fortis check`, `fmt` or `view`) running code, using the network, or changing
  files it was not asked to change, when given untrusted source.

Fortis is in early releases, and some features are knowingly incomplete. The known limitations
([docs/guide/known-limitations.md](docs/guide/known-limitations.md)) list the ones we already know about.
