# TV Tray — possible next steps

Ideas deferred after version 0.2.1. None is implemented; the order is not a priority.

## SSH module (Developer Mode)

SSH connection to the TV (port 9922, user `prisoner`) with the passphrase shown by the
*Developer Mode* app on the TV. The passphrase would be entered once in Settings and stored
encrypted with DPAPI, like the SSAP key. The key server is on port 9991 and only answers while the
Developer Mode app is open on the TV.

This is the basis for the items below.

## Automatic Developer Mode session renewal

Avoid reopening the *Developer Mode* app on the TV by hand when the session is about to expire.

- Read `/var/luna/preferences/devmode_enabled` over SSH to get the `sessionToken`.
- Renew with `https://developer.lge.com/secure/ResetDevModeSession.dev?sessionToken=<token>`.
- Read the expiry with `https://developer.lge.com/secure/CheckDevModeSession.dev`.
- Check at startup and at intervals; renew when less than about 12 hours remain.
- In Settings: “Developer Mode session: expires in N h” and a “Renew automatically” switch.
- Token stored encrypted with DPAPI.

To verify on a real TV: how long the extension actually lasts, any extra cookie or parameters,
behaviour when the session has already expired (it may need reactivating from the TV).
Source of the addresses: strings in `webos-dev-manager.exe`.

## Real file sending and app installation

With SSH: copy files to `/media/developer` and install `.ipk` packages, instead of only opening
a URL in the TV's browser. Read system logs.

## Interface

- Tray icon that changes state (TV reachable or not).
- Confirmation before “Power off”.
- Optional “Import key” field to reuse an existing SSAP key without pairing again.
- More languages: add a dictionary to `ui/index.html` and the table in `src-tauri/src/i18n.rs`.

## Other platforms

macOS and Linux builds. The Windows-specific parts are the key encryption (DPAPI), the ARP
lookup for the MAC address, “show file” after a screenshot, the blur window effect and the
panel placement above the tray icon. They would need the system keychain, `arp`/`ip neigh` and
per-platform window handling, plus testing on real machines.

## To verify on a real TV (features already present)

- Image sending from the app with a large photo (it is resized to 1920×1080 first).
- App list populated in the panel.
- PC keyboard to TV in a real text field (accents, dead keys, Ctrl+V).
- Scrolling with the wheel (`type: scroll`, undocumented).
- Installation from the installer and start with Windows.
