# Third-Party Notices

This document records third-party software CloudDesk-OS runs as an
external, unmodified OCI container -- not code compiled into or
distributed as part of the CloudDesk-OS binaries themselves. Rust/npm
dependency licenses are tracked separately by each package's own
lockfile/registry metadata and are not duplicated here.

## code-server (VS Code-compatible runtime, Phase 7)

```
Product:              code-server
Publisher:             Coder Technologies Inc.
Image:                 codercom/code-server
Pinned tag:            4.133.0
Pinned digest:         sha256:e073a441c61c85821a7f16b64cf93b4e77b4092899bb1f3bed906fbd558afd62
Underlying editor:     Code - OSS 1.133.0 (the MIT-licensed open-source
                       base of Visual Studio Code, not Microsoft's
                       proprietary VS Code build)
```

### License

Confirmed by inspecting the actual pulled image (not assumed from
documentation):

- `/usr/lib/code-server/LICENSE` inside the image: the MIT License,
  `Copyright (c) 2019 Coder Technologies Inc.`
- `/usr/lib/code-server/package.json`: `"license": "MIT"`,
  `"name": "code-server"`, `"version": "4.133.0"`
- `/usr/lib/code-server/lib/vscode/product.json`:
  `"nameShort": "code-server"`, `"nameLong": "code-server"`,
  `"licenseUrl": "https://github.com/coder/code-server/blob/main/LICENSE"`,
  `"serverLicenseUrl": "https://github.com/microsoft/vscode/blob/main/LICENSE.txt"`
  (the MIT-licensed Code - OSS repository's own license file).

`code-server` and its bundled editor base are both MIT-licensed. This
is a factual reading of the license text and metadata shipped inside
the image; it is not a legal opinion, and does not constitute legal
advice on CloudDesk-OS's own compliance obligations.

### What CloudDesk-OS does and does not do with it

- CloudDesk-OS runs the published `codercom/code-server` OCI image
  **unmodified** -- no source is vendored, patched, or recompiled.
- CloudDesk-OS does **not** bundle, distribute, or depend on any
  Microsoft-proprietary component. `product.json`'s own
  `nameShort`/`nameLong` fields confirm the running product identifies
  itself as "code-server", not "Visual Studio Code".
- Extensions are installed from **Open VSX** (`open-vsx.org`), the
  open-source extension registry code-server ships configured for by
  default. CloudDesk-OS does **not** have, claim, or attempt to use
  access to the Microsoft VS Code Marketplace, which requires a
  separate license Coder Technologies (and therefore CloudDesk-OS) does
  not hold. See `PHASE7_CODE_EVIDENCE.md` item 18 for the live
  extension-install evidence that confirms Open VSX is the registry
  actually used.
- No proprietary Microsoft telemetry, update-check, or marketplace
  endpoints are reachable: `--disable-telemetry`, `--disable-update-check`,
  and (as of the Phase 7 closure pass) `--disable-proxy` are always
  passed; see `services/clouddeskd/src/code_runtime.rs`.

### Attribution

Per the MIT License's own terms, the above copyright notice and
permission notice are reproduced here as shipped inside the image:

> The MIT License
>
> Copyright (c) 2019 Coder Technologies Inc.
>
> Permission is hereby granted, free of charge, to any person obtaining
> a copy of this software and associated documentation files (the
> "Software"), to deal in the Software without restriction, including
> without limitation the rights to use, copy, modify, merge, publish,
> distribute, sublicense, and/or sell copies of the Software, and to
> permit persons to whom the Software is furnished to do so, subject to
> the following conditions:
>
> The above copyright notice and this permission notice shall be
> included in all copies or substantial portions of the Software.
>
> THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
> EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
> MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
> NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE
> LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
> OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION
> WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

### Supply-chain pinning

CloudDesk-OS never runs `codercom/code-server:latest` and never accepts
an image/tag/digest from an HTTP request. The image reference is a
compiled-in default (`crates/config`'s `RuntimeConfig::code_image`),
pinned to the immutable content digest above rather than only the
mutable `4.133.0` tag -- a tag can, in principle, later be repointed at
different content by the publisher; a digest cannot. Re-verify with:

```sh
docker inspect codercom/code-server:4.133.0 --format '{{index .RepoDigests 0}}'
```

## Collabora Online (LibreOffice/Office runtime, Phase 8)

```
Product:               Collabora Online Development Edition (CODE)
Publisher:              Collabora Productivity Ltd.
Image:                  collabora/code
Pinned tag:             26.04.3.1.1
Pinned digest:          sha256:6b70f91f0b6e9c76f75f162f58ef0a12cf9415d78e14713d33c0318ddc4a2cc0
coolwsd server version: 26.04.3.1 (confirmed live via container startup logs)
```

### CODE is the development/testing edition -- not the recommended production deployment

**Collabora Online Development Edition (CODE)** is Collabora's own
free, community-support-only edition, explicitly positioned by
Collabora as suitable for evaluation, development, and small/personal
deployments -- not as the vendor-recommended path for a production
worldwide release. CloudDesk-OS uses CODE in this phase because it is
readily available as a pre-built OCI image suitable for disposable
local/CI testing, per this phase's closure instructions.

**For a production CloudDesk-OS deployment**, an administrator should
point CloudDesk at an appropriately supported Collabora Online
deployment (Collabora's commercially supported offering, or a
self-managed Collabora Online Server deployment an organization has
chosen to operate and support itself) rather than CODE. CloudDesk-OS's
architecture does not require CODE specifically: `services/clouddeskd`
speaks the real, standard WOPI protocol (`/hosting/discovery`,
`CheckFileInfo`/`GetFile`/`PutFile`/locking) to whichever Collabora
Online server the runtime adapter is pointed at -- CODE is a
configuration choice (Task 2/59), not an architectural dependency.
External-mode configuration itself is not yet wired to a Settings API
this phase (see `crates/config`'s `RuntimeConfig::office_external_url`
doc comment and `PHASE8_OFFICE_EVIDENCE.md` Task 23).

### License

`coolwsd` (the Collabora Online WOPI/collaboration server) and the
bundled LibreOffice Online core are licensed under the **Mozilla Public
License 2.0 (MPL-2.0)**. Confirmed directly from the running container's
own served output -- the editor bootstrap HTML CloudDesk's WOPI host
receives through the real proxy begins with:

```
<!--
  SPDX-License-Identifier: MPL-2.0

  Copyright the Collabora Online contributors.
  Copyright Collabora Productivity Limited.

  This Source Code Form is subject to the terms of the Mozilla Public
  License, v. 2.0. If a copy of the MPL was not distributed with this
  file, You can obtain one at http://mozilla.org/MPL/2.0/.
-->
```

and from the image's own OCI labels (`docker inspect
collabora/code:26.04.3.1.1 --format '{{json .Config.Labels}}'`):

```
"author": "Collabora Productivity Ltd."
"commit.history.core": "https://gerrit.collaboraoffice.com/plugins/gitiles/online/+log/cp-26.04.3-1"
"version": "26.04.3.1"
```

The bundled LibreOffice engine itself is separately licensed under the
Mozilla Public License 2.0 (its own long-standing upstream license);
this document does not attempt to enumerate every third-party component
LibreOffice itself bundles -- that inventory belongs to the upstream
LibreOffice and Collabora Online projects, not CloudDesk-OS. No claim is
made here about any commercial licensing terms Collabora Productivity
Ltd. may separately offer; this section only records what CloudDesk-OS
actually observed and pins.

### Supply-chain pinning

CloudDesk-OS never runs `collabora/code:latest` and never accepts an
image/tag/digest from an HTTP request. The image reference is a
compiled-in default (`crates/config`'s `RuntimeConfig::office_image`),
pinned to the immutable content digest above rather than only the
mutable `26.04.3.1.1` tag. Re-verify with:

```sh
docker inspect collabora/code:26.04.3.1.1 --format '{{index .RepoDigests 0}}'
```

### Deployment model

CloudDesk-OS core (`clouddeskd`) does not require the Office runtime to
start, and a normal installation with Office disabled starts zero
Collabora processes or containers (Task 60) -- Office is an optional,
heavier-weight runtime, activated only when an administrator enables it
via Settings, exactly like the Code runtime (Phase 7). Collabora
receives no bind mounts of the filesystem at all; document bytes cross
the boundary only through authorized WOPI HTTP operations
(`services/clouddeskd/src/wopi.rs`) -- see `PHASE8_OFFICE_EVIDENCE.md`
for the full security model and live evidence.

## Brave Browser (remote Browser runtime, Phase 9)

```
Product:               Brave Browser
Publisher:             Brave Software, Inc.
Pinned version:        1.93.136 (Chromium 151)
CloudDesk image tag:   clouddesk-brave:1.93.136
Build source:          docker/brave/Dockerfile -- built locally from
                       Brave's own official apt repository, not a
                       third-party pre-built image
```

### License

Brave Browser itself is proprietary freeware, built on top of the
BSD-licensed Chromium open-source project. **No formal legal
conclusion about Brave's own license is drawn here** -- operators
should review Brave Software's published license terms directly before
any commercial redistribution decision that bundles or depends on it.
Chromium's own upstream BSD license governs the portions of the
codebase Brave derives from it.

### Deployment model

Like Code and Office, Browser is an optional, heavier-weight runtime:
disabled by default, zero resident containers/processes while disabled,
started only when an administrator enables it via Settings. It runs
network-isolated on a dedicated Docker subnet
(`clouddesk-browser-net`), never as root, never with a bind-mounted
host filesystem -- see `PHASE9_BROWSER_EVIDENCE.md` for the full
security model and live evidence.

## FFmpeg (media transcode/probe pipeline, Phase 3)

```
Product:               FFmpeg
Tested/shipped version: 8.1.2
Distribution model:    system package (not bundled/redistributed by
                       CloudDesk-OS itself -- CloudDesk-OS invokes
                       whatever ffmpeg/ffprobe binary is present on
                       the host, or is absent, and disables media
                       features accordingly)
```

### License

FFmpeg is dual-licensed (LGPL or GPL depending on build configuration).
**The specific build actually exercised during this project's live
testing (`ffmpeg version 8.1.2`) reports `--enable-gpl` in its own
`ffmpeg -version` configuration banner** -- confirmed live, not
assumed -- meaning that specific build includes GPL-licensed
components, not an LGPL-only configuration. Because CloudDesk-OS does
not bundle or redistribute the ffmpeg binary itself (it is a host
system dependency the administrator installs separately), this is
recorded as an **engineering finding about the tested environment's
build, not a claim about every operator's own ffmpeg installation**.
**Requires legal review** before any release messaging that assumes an
LGPL-only FFmpeg dependency, since a GPL-configured system ffmpeg is a
materially different licensing situation from an LGPL one for
distribution purposes.
