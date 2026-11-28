# Release Guide — Echoes Studio

How a version reaches users: a tag builds a signed Windows installer on GitHub Actions, and installed
apps update themselves from the published release.

## How it works

- `.github/workflows/release.yml` runs on a tag `vX.Y.Z`. It builds the NSIS installer (the plugin
  bridge sidecar included, `src-tauri/tauri.bundle.conf.json`), signs it for the updater
  (`src-tauri/tauri.release.conf.json` turns `createUpdaterArtifacts` on) and uploads it with
  `latest.json` to a **draft** GitHub release.
- The app (`UpdateDialog.svelte`, `tauri-plugin-updater`) asks
  `https://github.com/armribdev/echoes-studio/releases/latest/download/latest.json` at startup, never in
  `tauri dev`. A newer version opens a dialog; "Install and restart" downloads, verifies the signature
  against the public key in `tauri.conf.json` and runs the installer.
- Installing closes the app without asking: the command `has_unsaved_changes` makes the dialog refuse
  while the project has unsaved edits.
- The repository must be **public**: a private repository answers 404 to the anonymous check.

## One-time setup

1. **Updater key pair** (proves an update comes from us; lose the private key and installed apps can
   never update again):
   `pnpm tauri signer generate -w ~/.tauri/echoes-studio.key`. Put the public key in
   `plugins.updater.pubkey` of `src-tauri/tauri.conf.json`, and the private key and its password in the
   GitHub secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
2. **Authenticode** (optional, removes the SmartScreen warning): buy a code-signing certificate, export it
   as `.pfx`, store `base64 -w0 cert.pfx` in the secret `WINDOWS_CERTIFICATE` and its password in
   `WINDOWS_CERTIFICATE_PASSWORD`. Without them the installer is unsigned but updates stay verified.
   After the first signed build, check the installer and the bridge sidecar with `signtool verify /pa`.
3. macOS notarisation is not set up (no macOS build yet).

## Releasing

1. Bump the version in `src-tauri/tauri.conf.json` and `src-tauri/Cargo.toml` (they must match the tag).
2. `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. When the workflow ends, check the draft release (installer, `.sig`, `latest.json`), write the notes
   (they are shown in the update dialog), then publish it.

`tauri build` refuses a different minor version between the Rust crate and the npm package: keep
`@tauri-apps/plugin-updater` (`~2.12.0` in `package.json`) on the minor of `tauri-plugin-updater` in
`Cargo.lock`.

The app is built for the Windows GUI subsystem in release (`windows_subsystem` in `main.rs`): no console
window, so the `[audio]` / `[plugin]` logs are only visible in `tauri dev` or a debug build.

To try the chain without users, publish a release of a higher version than the one installed and open the
installed app.
