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
