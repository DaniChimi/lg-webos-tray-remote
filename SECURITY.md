# Security policy

## Supported versions

Only the latest release receives fixes. Please update before reporting a problem.

## Reporting a vulnerability

Please do **not** open a public issue for a security problem.

Use GitHub's private reporting: open the **Security** tab of this repository and choose
**Report a vulnerability**. Include what you found, the version of TV Tray, and the steps to
reproduce it.

This is a free project maintained in spare time, so there is no guaranteed response time, but
reports are read and real problems are fixed and credited if you wish.

## What the app does, for context

- It talks only to the TV on the local network.
- The pairing key is stored encrypted with Windows data protection (DPAPI), valid only for the
  current Windows user.
- To send an image it briefly opens a small HTTP server on the local network at an unguessable
  address, and closes it after one hour or at the next send.
- It collects and sends no data anywhere else.

## Disclaimer

The software is provided "as is", without warranty of any kind, as stated in the
[MIT licence](LICENSE). Use it only with devices you own or are allowed to control.
