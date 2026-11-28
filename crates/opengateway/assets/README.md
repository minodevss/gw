# Admin page assets

These files are compiled into the binary and served under `/admin/assets/`.

- `admin.css`: written for OpenGateway.
- `htmx.min.js`: htmx 4.0.2, unmodified.
  - Source: https://github.com/bigskysoftware/htmx/releases/download/v4.0.2/htmx.min.js
  - SHA-256: `fd25ba3fe5e44768756f23862a6daa4f928192199a121e825e882e719bc8aabd`
  - Same bytes as `dist/htmx.min.js` in the npm package `htmx.org@4.0.2`.
  - Dependabot does not track this file. To update it, download the new release, check it against the npm package, and update the version and hash here.
  - The admin pages use no htmx feature that needs `eval` (`hx-on`, `js:` values, trigger filters), so the Content Security Policy allows neither `unsafe-eval` nor `unsafe-inline`.
