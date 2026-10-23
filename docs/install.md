# Install Burrata

Download from [Burrata releases](https://github.com/anaxonda/burrata/releases).
Expand **Assets** if the downloads are hidden. Regular releases appear under **Latest**.

## Choose your download

| Computer | Download | Notes |
| --- | --- | --- |
| Apple silicon Mac (M1 or newer), macOS 14 or later | `Burrata_<version>_aarch64.dmg` | No Intel Mac installer is currently supplied. |
| Windows x64 | `Burrata_<version>_x64-setup.exe` | Includes the WebView2 installer. |
| Ubuntu/Debian Linux, x86_64 | `Burrata_<version>_amd64.deb` | Preferred on compatible Debian-based systems. |
| RPM-based Linux, x86_64 | `Burrata-<version>-1.x86_64.rpm` | Use your distribution's software installer. |
| Other compatible Linux, x86_64 | `Burrata_<version>_amd64.AppImage` | Portable; see the extra steps below. |

Download only one app installer. Node.js, FFmpeg, yt-dlp and the browser connection
helper are bundled; you do not need to install them separately. **Source code**
archives are for developers. Supporting helper packages are optional.

### macOS

1. Open the downloaded DMG and drag **Burrata** into **Applications**.
2. Eject the disk image, then open Burrata from Applications. Install it there
   before connecting the browser companion; do not run it from the mounted DMG.
3. This release is not Developer ID signed or notarized. If macOS blocks the first
   launch because the developer cannot be verified, follow Apple's
   [Open Anyway instructions](https://support.apple.com/en-us/102445) in
   **System Settings → Privacy & Security**, after checking that you downloaded
   Burrata from this repository's release page. Do not disable Gatekeeper globally.

### Windows

1. Open the `x64-setup.exe` download and complete the installer.
2. Windows may show an unknown-publisher/SmartScreen warning because this release
   is unsigned. If you trust this download, **More info → Run anyway** may be
   available. If your organization blocks installation, ask its administrator;
   the installer cannot override that policy.
3. Open Burrata from the Start menu. The installer includes WebView2 and the
   required runtime helpers; no terminal commands are needed.

On Windows **N** editions, missing system media components can affect playback.
If media fails despite the app opening, install Microsoft’s optional
[Media Feature Pack](https://support.microsoft.com/en-us/windows/experience/platform-variants/media-feature-pack-for-windows-n)
and restart Windows. This is specific to N editions; ordinary Windows installs
do not need that extra step.

### Linux

For DEB/RPM, open the file in your distribution's software installer and approve
installation. Let the package manager install its dependencies. On Ubuntu/Debian,
if the graphical installer cannot open it, the terminal alternative is
`sudo apt install ./Burrata_<version>_amd64.deb` from the download directory,
substituting the actual filename. This installs WebKitGTK/GStreamer dependencies
as well as Burrata. The release build baseline is Ubuntu 22.04; compatibility with
every distribution is not established.

For AppImage, move the file to a permanent folder such as `Applications` in your
home directory. In its **Properties → Permissions**, enable execution as a program
(the wording varies by file manager), then open it. The terminal alternative is
`chmod +x Burrata_<version>_amd64.AppImage`, followed by
`./Burrata_<version>_amd64.AppImage`. If it reports a FUSE or system-library error,
use your distribution's AppImage instructions or the DEB/RPM package where
compatible. AppImage is not guaranteed to run on every Linux system.

Linux browser connections also need a usable desktop session and, for saved
Spotify/YouTube accounts, an unlocked operating-system keyring. Packaged DEB,
macOS DMG and Windows installers pass automated install/startup smoke tests;
RPM/AppImage are built and checksummed but have no separate installed-app smoke.
Downloaded-file OS security prompts have not been manually qualified on every OS.

## Optional browser extension

The app works without the extension for local-library use and supported direct
search/import paths. Install it to import a supported page open in your browser
or use browser-assisted provider access.

The same extension downloads work on all three systems:

| Browser | Download | Installation |
| --- | --- | --- |
| Firefox | `firefox-extension-<extension-version>.xpi` | [Firefox steps](../extension/README.md#firefox) |
| Chrome / Chromium / Edge | `chrome-extension-<extension-version>.zip` | [Chrome/Edge steps](../extension/README.md#chrome-chromium-and-edge) |

The extension has its own version. Apps **0.1.8**, **0.1.7**, **0.1.6** and **0.1.5** package extension **0.2.3**; app **0.1.3** shipped extension **0.2.2**.
Installing the extension alone is not enough: also use **Settings → Connections →
Browser companion → Connect browser companion** in Burrata, then restart your
browser. See the [complete companion guide](../extension/README.md#install).

## Spotify and YouTube accounts

On Linux/macOS, open **Settings → Connections** to opt into the experimental
in-app provider connection. Sign in, continue through the account check, and
confirm the displayed account/channel. Each provider is confirmed separately.
Saved sessions require an unlocked system keyring; if saving fails, unlock or
configure it and try confirmation again. Provider requests run when needed; you
do not need to keep the sign-in window open after confirmation.

In-app provider sessions are **not enabled on Windows in this release**. Use
supported public-page imports through the browser companion; browser-assisted
account access is a separate experimental option, not a guarantee of equivalent
Windows account functionality. See [connections](connections.md) for current
support and limits. Ordinary current-page imports do not require pairing a
YouTube account. Playlist writing is a separate experimental opt-in; leave it off
unless you intend to review and apply changes.

## Updates and common problems

Updates are manual. Quit Burrata, install the newer app, then reopen it. Do not
delete the app's library/profile to update. Reconnect the browser companion in
Settings after moving an AppImage or changing the installed app location.
Update the extension using its [browser-specific instructions](../extension/README.md#updates).

- **Native host disconnected:** keep Burrata running, choose **Connect browser
  companion**, fully quit/reopen the browser, then open the extension popup.
  Host registration and a live extension connection are separate checks.
- **Browser installed through Snap/Flatpak, or a different Chromium browser:**
  those sandbox/registration paths are not qualified. Start with a normally
  installed Firefox, Chrome, Chromium or Edge; do not assume a registered host
  can be reached from a sandboxed browser.
- **A bundled tool cannot start:** use the **Check tools** action in Settings;
  see [tool setup](tool-setup.md). Normally there is no need to set custom paths.
- **Music source unavailable:** this can depend on the provider, network or
  recording match; it does not by itself mean installation failed.

Soulseek requires a separately configured [slskd service](../deploy/slskd/README.md).
The external mpv player is optional and separately installed. Neither is required
for ordinary Burrata setup.


### Capture a playback diagnostic report

1. Open **Settings → Advanced → Playback diagnostics** and enable **Enable playback diagnostics**.
2. Select **Clear diagnostic events** to start a focused capture. Reproduce one failure, then try one working comparison without changing playback or matching settings.
3. Return to Advanced and select **Export report…**. Save the JSON **before closing or restarting Burrata**; events are held in memory.
4. Include the track names and where you started them (Library, album, search or a direct link), plus the visible error. For a seek failure, include the requested position. Track names are excluded from the report, so supply those separately.

The report is saved locally and is not uploaded automatically. It contains bounded
player events and timings, sanitized source-failure categories and hashed track/source
references. The expanded report in v0.1.8 also includes WebView startup/media-event
history, playback settings, helper versions and the macOS version when available.
It excludes credentials, full media URLs and raw helper errors.

If starting from Library fails while **Play album** works, capture both attempts in
the same session. This helps distinguish source selection, extraction, access denial
and player startup. Existing library data survives replacing the app; deleting and
reinstalling the application alone does not reset saved tracks or settings.
