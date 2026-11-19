# Releasing Outpost

How a version gets from the source to people's computers, and what the
installers do. Written for milestone 9; nothing has been released yet.

## What is built

| File | For | What it does |
| --- | --- | --- |
| `Outpost-Mail-X-windows-x64-setup.exe` | Windows 10 and 11, 64-bit | Installs for the current user only (no administrator needed) into the user's local application folder, adds a Start menu entry and an uninstaller. If the WebView2 runtime is missing (it is part of Windows 11 and current Windows 10) it is downloaded from Microsoft during the install. |
| `outpost-mail_X_amd64.deb` | Debian, Ubuntu, Mint | Installs `/usr/bin/outpost`, a menu entry and icons. Depends on `libwebkit2gtk-4.1-0`, `libgtk-3-0` and `libayatana-appindicator3-1`. |
| `outpost-mail-X.x86_64.rpm` | Fedora, openSUSE | The same. |
| `Outpost-Mail-X-x86_64.AppImage` | Other Linux systems | One file to make executable and run. Nothing is installed, so it cannot be offered as the default mail program. |
| `SHA256SUMS.txt` | | Checksums of the files above. |
| `THIRD-PARTY-NOTICES.txt` | | The licences of every library in the app. Also installed with the app. |

All of them keep mail, settings and the log in the same per-user data
folder the app has always used (`%APPDATA%\io.github.scarylepoo.outpostmail`
on Windows, `~/.local/share/io.github.scarylepoo.outpostmail` on Linux), so
installing over a copy that was run from a build keeps its accounts and
mail. Uninstalling leaves that folder alone (the Windows uninstaller has a
box for deleting it, not ticked unless the user ticks it).

## The installers are not signed

The handoff doc decided against paid code signing. The consequences, which
the release notes tell people about:

- **Windows** shows "Windows protected your PC" (SmartScreen) the first time
  the installer is run. "More info", then "Run anyway", continues. Some
  antivirus programs are suspicious of any unsigned installer.
- The checksums in `SHA256SUMS.txt` are the only way to tell that a download
  is the file GitHub built. They guard against a damaged or swapped
  download, not against a compromised GitHub account.

## Being the default mail program

- **Windows.** The installer registers Outpost as a program that can open
  email links (`crates/shell-tauri/installer/hooks.nsh`: entries under the
  current user's `Software\Classes`, `Software\Clients\Mail` and
  `Software\RegisteredApplications`, removed on uninstall). Since Windows 10
  a program cannot make itself the default; "Make Outpost the default" in
  Settings opens Windows Settings at the right page and the user chooses
  it there.
- **Linux.** The packages' menu entry declares `x-scheme-handler/mailto`.
  "Make Outpost the default" runs
  `xdg-settings set default-url-scheme-handler mailto "Outpost Mail.desktop"`.

Started with a `mailto:` address, Outpost opens a new message to it (to,
cc, bcc, subject and text are taken; nothing else, in particular no
attachment). If Outpost is already running, the running window takes it.

## The update check

Once a day, if "Check for a new version" is on (it is by default), the core
asks `api.github.com` for the latest published release of this repository
and compares its tag with the running version. A newer one is shown in the
status bar and under Settings > About and updates, with a button that opens
the release page in the browser. Nothing is downloaded or installed.

The request contains no account, identifier or version number. The link
that is opened is used only if it is under this repository's releases;
anything else in GitHub's answer is replaced by the releases page.

A **draft** or **pre-release** is not announced: GitHub's "latest release"
skips both.

## Making a release

1. Decide the version. Set it in three places, which the release build
   checks agree: `Cargo.toml` (`[workspace.package]`),
   `crates/shell-tauri/tauri.conf.json`, and the two `package.json` files.
2. Edit `docs/release-notes.md`: it becomes the text of the release.
3. Run the tests and the app self-test on both systems (`docs/testing.md`),
   and the performance run if the store or index changed.
4. Commit, push, wait for CI to pass.
5. Tag and push the tag: `git tag v0.1.0 && git push origin v0.1.0`.
6. The **Release** workflow builds the installers on GitHub's Windows and
   Ubuntu 22.04 machines and creates a **draft** release with them.
7. Download the installers from the draft, install each on a real machine,
   and check: it starts, an account syncs, the previous version's mail is
   still there.
8. Publish the draft on GitHub. From then on, running copies announce it.

To build the installers without releasing anything, run the Release workflow
by hand from the Actions page (or `gh workflow run release.yml`): the files
are kept as a download on the run.

Locally: `npx tauri build --bundles nsis` on Windows, `--bundles deb,rpm` on
Linux. `--no-bundle` builds only the program.

## What has and has not been checked (18 November 2026)

- Built here: the Windows installer (6.1 MB), the `.deb` and the `.rpm`
  (10.8 MB each). The `.deb`'s contents were listed and its menu entry read.
- **Not done:** no installer has been run. Installing needs the owner's
  machine (Windows) or administrator rights (Linux), and should not be done
  unattended. So the Windows registry entries, the uninstaller, SmartScreen's
  behaviour, and "Make Outpost the default" on either system are unverified.
- **Not done:** the AppImage has only been built by the workflow, never
  locally and never run.
- The update check has run against the real GitHub API with no release
  published (answer: nothing newer). It has not yet seen a real newer
  release; that path is covered by unit tests on recorded answers.
