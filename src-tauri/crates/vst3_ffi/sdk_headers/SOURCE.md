# Steinberg VST3 interface headers (subset)

Unmodified copies of the files that declare the interface IDs `vst3_ffi` uses, kept
so a test can compare the Rust constants with the SDK (ADR-033, step 0 and 1).

- Repository: https://github.com/steinbergmedia/vst3_pluginterfaces
- Commit: `4f547e8e102b47de4a8b8aaf343c73b700786372` (2026-08-11, "VST3 SDK 3.8.1",
  tag `v3.8.1_build_84`)
- Licence: MIT, `LICENSE.txt` next to this file (Copyright (c) 2026, Steinberg
  Media Technologies GmbH)

Never edit these files. To update them, fetch the new release into an empty folder
outside the repository, copy the same files, and change the commit above.
