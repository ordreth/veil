# Additional permissions

veil is licensed under the [PolyForm Strict License 1.0.0](LICENSE): you may read the code and use veil for any noncommercial purpose, but not distribute it, change it or make new works from it, and not use it commercially. The licensor is Ordreth, the trade name of veil's author (an individual developer; Ordreth is not a registered company). The licensor grants the following on top of that license, for every version of veil that is released with this file. They do not widen the license in any other way.

## 1. Release binaries and the files in them

You may redistribute the tarballs that veil's release workflow publishes on the project's GitHub Releases page, **unchanged** (byte for byte, with `LICENSE`, this file and every notice in them), and you may build a package of your own (for example an AUR `veil-bin` package, or a mirror) that downloads, checks and installs the files of such a tarball, as long as they are not modified. Installing them in other directories, and the change `install.sh` makes to the `ExecStart` line of the systemd unit so that it names the installed binary, are not modifications.

## 2. Building from source for yourself

You may build veil from its source (`cargo build`, `nix build`, a source package that fetches this repository) and run what you built. The result is for your own use: you may not distribute it, under this section.

## 3. Quoting

You may quote short excerpts of the source code (up to 50 lines at a time) with attribution, in a bug report, a discussion, an article or a review.

## 4. Name and logo

The name "veil" and the icon in `assets/icon/` may be used to refer to this software (in a package listing, an article, a compatibility report), and not as the name or logo of another product, of a modified version or of a work derived from it. The icon files are under the same license as the code.

## Not granted

Commercial use, modified copies, distribution of the source code, and anything else the license does not allow need the written permission of the licensor. Ask at contact@ordreth.com.

## Changes

The licensor may change the license or these permissions for future versions. The terms of a version that was already released stay as they were released.
