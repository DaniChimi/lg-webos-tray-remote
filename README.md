# TV Tray: LG webOS TV remote for the Windows system tray

🇬🇧 English · 🇮🇹 [Italiano](README.it.md)

A small Windows app that lives in the system tray, next to the clock, and works as an
**LG TV remote control for Windows 10 and 11**. It controls an **LG webOS TV** on the same
network: remote, volume, inputs, apps, on-screen messages, screenshots and image sending, and
turns the TV on with **Wake-on-LAN**.

It works without Developer Mode, without an LG ThinQ account and without installing anything on
the TV. It uses the TV's local control protocol (SSAP, port 3001).

> Tested on an LG TV running webOS 25. Behaviour on other models and versions is not verified.

## Features

- **Power:** turns the TV off; turns it on with Wake-on-LAN (the MAC address is detected
  automatically the first time you power off from the panel).
- **Sound:** volume up and down, mute.
- **Inputs and apps:** switch input (HDMI…), launch installed apps.
- **Remote:** directional pad with OK, Menu, Home, Back, Exit. In *Mouse* mode the area works as a
  trackpad (TV pointer) with click, wheel and right button = back.
- **Text:** the PC keyboard types into the active text field on the TV.
- **On-screen message:** shows a notification on the TV.
- **Screenshot:** saves the TV screen to `Pictures\TV Screenshots`.
- **Send image:** shows a JPG or PNG from the PC in the TV's browser. The image is resized to
  1920×1080 before sending; the original is not touched.
- **Language:** English and Italian, selectable in Settings (follows Windows by default).
- **TV discovery** on the network with guided pairing; optional start with Windows.

## First run

1. Start TV Tray: the tray icon appears and the panel opens by itself.
2. The app looks for LG TVs on the network. If it finds one it pairs with it; the TV shows a
   pairing request: accept it with the remote.
3. From then on the panel opens with a click on the icon; right-click opens the menu
   (Open panel, Turn TV off, Quit).

The TV must be on and on the same network as the PC. If discovery does not find it, enter the
IP address by hand in **Settings**.

## Where data and keys are stored

- Configuration lives in `%APPDATA%\dev.daniele.tvtray\config.json`: IP address, MAC address,
  language and the pairing key.
- The pairing key is **encrypted with Windows data protection (DPAPI)**: it is valid only for your
  user on this PC. If you change user or PC you need to pair again.
- No key is in the code or in the repository. “Unpair from this TV” in Settings erases the IP,
  MAC and key.
- To send images the app briefly opens a small HTTP server on the local network, at an
  unguessable address, and closes it after one hour or at the next send. Windows may ask you to
  allow firewall access.

## Build

You need Rust (MSVC toolchain) and `tauri-cli` 2.x.

```powershell
cd src-tauri
cargo run                 # development build (debug)
cargo tauri build         # NSIS installer in src-tauri\target\release\bundle\nsis
```

The interface is a single `ui/index.html` file (HTML, CSS and JavaScript, no bundler),
embedded in the executable at build time.

The installer is not signed: Windows shows the SmartScreen warning on first run.

## Layout

| Path | Content |
|---|---|
| `ui/index.html` | Panel interface and its English/Italian dictionary |
| `src-tauri/src/main.rs` | Commands, tray, window, configuration |
| `src-tauri/src/ssap.rs` | SSAP client for the TV |
| `src-tauri/src/discovery.rs` | TV discovery on the network |
| `src-tauri/src/dpapi.rs` | Key encryption with DPAPI |
| `src-tauri/src/capture.rs` | Screenshots |
| `src-tauri/src/share.rs` | Temporary server for sending images |
| `src-tauri/src/i18n.rs` | Native messages (English and Italian) |

## Roadmap

Deferred ideas are in [`ROADMAP.md`](ROADMAP.md).

## Acknowledgements

- The permission manifest (`src-tauri/src/pairing.json`) derives from
  [`lgtv2`](https://github.com/hobbyquaker/lgtv2) (MIT licence).
- How the SSAP protocol and Developer Mode work was learned by observing the TV and
  [webOS Dev Manager](https://github.com/webosbrew/dev-manager-desktop) (Apache-2.0);
  none of its code is included.
- TV Tray is not affiliated with LG Electronics. “LG” and “webOS” are trademarks of their
  respective owners.
