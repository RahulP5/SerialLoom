# Publishing and releasing

## 1. Private repository first

```powershell
gh auth login                                   # once, in your terminal
gh repo create serialloom --private --source . --push
```

Then on GitHub:

- **Actions:** confirm the *CI* workflow passes. It builds SerialLoom's driver, fetches the signed usbip-win2 files, runs all tests and builds the installer. Download the `installer-unsigned` artifact and test it on a clean PC or VM.
- **Settings > Branches:** protect `main` (require the CI check to pass; block force pushes).
- SerialLoom does not take outside code contributions. GitHub cannot switch pull requests off for a public repository; close unsolicited ones, or limit who can open them under **Settings > Moderation options > Interaction limits**.

## 2. Test checklist (private phase)

- [ ] Fresh install on Windows 10 and Windows 11 (Secure Boot on, no test mode).
- [ ] Settings shows the virtual USB driver as *ready*.
- [ ] Virtual pair with chosen numbers (e.g. COM20/COM21); two terminals exchange data.
- [ ] *Change number* moves a port; a number used by a real USB adapter (plugged in or not) is refused.
- [ ] Split a real port into two virtual ports; both programs receive data.
- [ ] Bridge a port to TCP (`0.0.0.0:7000`), connect with a TCP client.
- [ ] Delete a port: it disappears from Device Manager (also with *View > Show hidden devices*).
- [ ] Close and reopen SerialLoom: ports come back with the same numbers.
- [ ] Uninstall: ports, the virtual USB controller (if SerialLoom installed it) and leftovers are gone.

## 3. Before making the repository public

- [ ] Request a SerialLoom product ID at [pid.codes](https://pid.codes) and replace the test PID in `crates/serialloom-core/src/usbip/cdc.rs`.
- [ ] Review `git log` for anything private.
- [ ] Settings > Code security: enable **private vulnerability reporting** (used by `SECURITY.md`), Dependabot alerts and secret scanning.
- [ ] Settings > General: enable Discussions (linked from the issue template).

Then: **Settings > General > Danger zone > Change visibility > Public**.

## 4. Releases

Push a version tag; the *Release* workflow builds the installer and opens a **draft** release:

```powershell
git tag v0.1.0
git push origin v0.1.0
```

Review the draft, attach release notes from `CHANGELOG.md`, and publish it.

The installer is not code-signed yet, so Windows SmartScreen asks users to confirm. Free signing for open-source projects is available from [SignPath Foundation](https://signpath.org) once the repository is public.
