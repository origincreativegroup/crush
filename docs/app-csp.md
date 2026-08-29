# App content security policy

`crates/app/src-tauri/tauri.conf.json` ships this strict CSP (previously `null`):

```
default-src 'self';
script-src 'self';
style-src 'self' 'unsafe-inline';
img-src 'self' asset: http://asset.localhost https://asset.localhost;
media-src 'self' asset: http://asset.localhost https://asset.localhost;
connect-src 'self' ipc: http://ipc.localhost
```

(Whitespace above is for readability; the config value is a single line.)

## Each exception, verified against real usage

- **`default-src 'self'`** — fallback for everything else. The UI ships `styles.css`,
  `search.css`, `app.js`, and `search.js` from the bundle; no fonts, frames, objects, or
  workers are used, so no further exceptions are needed.
- **`script-src 'self'`** — the two deferred app scripts only. No inline scripts, no
  `eval`, and the UI must never load remote code (no network egress rule).
- **`style-src 'self' 'unsafe-inline'`** — the stylesheets are bundled; `'unsafe-inline'`
  covers the JS-set progress-fill widths (`app.js` model download rows and the ingest row
  progress bars assign `style.width`). These are CSSOM property assignments, which CSP
  does not actually block, so this exception is conservative; dropping it would also work
  but is kept for defense against future `style=""` usage. Refactoring the fills to
  classes only is not worth the churn.
- **`img-src 'self' asset: http://asset.localhost https://asset.localhost`** —
  `convertFileSrc` (`search.js`) feeds result thumbnails (`img.src`) and the detail photo
  (`#detail-photo`). Tauri 2 serves the asset protocol as `asset://…` on macOS and
  `http://asset.localhost/…` on Windows, so both hosts are required. No remote images.
- **`media-src 'self' asset: http://asset.localhost https://asset.localhost`** — the
  shot detail player (`#detail-video`) plays proxy or source video through
  `convertFileSrc`, needing the same two asset-protocol hosts as images.
- **`connect-src 'self' ipc: http://ipc.localhost`** — Tauri 2's IPC channel is
  `ipc://localhost` (Windows/Linux) or `http://ipc.localhost` (macOS); without these the
  `invoke` bridge cannot reach the Rust backend. `'self'` covers same-origin fetches;
  the UI performs no other network requests.

## Verification

- Thumbnails, detail photo, and video playback all go through `convertFileSrc` → covered
  by the `asset:` hosts above (this was the only runtime consumer of remote-shaped URLs).
- No `fetch`/`XHR`/WebSocket exists anywhere in `crates/app/ui`; the only connect-src
  consumer is the Tauri IPC bridge.
- If a directive ever breaks thumbnails or the asset protocol on the Mac, narrow it
  (e.g. keep only the platform-specific asset host) rather than removing the CSP.
