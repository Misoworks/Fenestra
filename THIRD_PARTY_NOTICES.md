# Third-Party Notices

Fenestra source code is licensed under `MIT OR Apache-2.0`.

Fenestra can resolve, download, build against, or bundle third-party webview
runtimes. Those runtimes are not licensed by Fenestra's project license.

## Chromium Embedded Framework and Chromium

CEF is distributed under a BSD-style license and is based on Chromium, which
includes additional third-party components and notices.

When a Fenestra app bundles or redistributes a CEF/Chromium runtime, the bundle
must preserve the license and notice files shipped with that runtime. In
practice, packaged apps should include the CEF distribution's `LICENSE.txt`,
`README.txt`, and Chromium third-party notice files alongside the bundled
runtime.

User-local runtime installs should keep those files in the installed runtime
directory under `~/.local/share/fenestra/runtimes/cef/`.
