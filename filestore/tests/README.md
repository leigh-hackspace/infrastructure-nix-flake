# filestore headless-browser test suite

Drives the real filestore web UI in headless Chromium against a scratch
fixture, plus the JSON API. Run it from the repo root:

```bash
just filestore-test
```

`run.sh` builds the SPA (`frontend/build.sh`) and the binary, starts
`filestore --no-auth` on `127.0.0.1` against a scratch fixture, and runs
`suite.mjs`. Every test rebuilds the fixture and reloads the page, so tests are
order-independent.

The fixture and the run's logs live in a fresh `mktemp -d` directory that is
removed when the suite finishes (`FS_TEST_KEEP=1` keeps it for poking around;
`FS_TEST_ROOT` overrides the fixture location if you want it on another
filesystem). Nothing is written to a fixed `/tmp` name, so two runs — or a
leftover from a crashed one — cannot clobber each other.

## Why `--no-auth` exists

The API is session-gated by OIDC, which a headless browser cannot complete.
`--no-auth` treats every request as an authenticated local session, and it is
**only honoured on a loopback bind** (`main.rs` refuses `--no-auth` with a
non-loopback `--bind`), so the escape hatch can never expose the store on a
routable interface. It is never passed in production — the NixOS service runs
without it, and the suite asserts that a server started without `--no-auth`
still answers 401.

## Playwright on NixOS

Playwright's own browser download is unwrapped and cannot find `libglib`, so
`run.sh` uses nixpkgs' wrapped browsers
(`nix build nixpkgs#playwright.browsers-chromium`) and passes the executable
path explicitly. Override with `FS_TEST_BROWSER`.

Node is used here deliberately: Playwright is the only practical headless
driver, and its Rust bindings are not usable offline in this repo.

## Hooks the suite relies on

The SPA is otherwise hard to drive from a browser, so the UI carries stable
test hooks (harmless in production):

- every grid/table row has `data-fs-path`, `data-fs-name` and `data-fs-sel`
  (`1` when the row is selected, so a test can check *which* rows are selected,
  not just how many)
- stable ids: `#fs-content`, `#fs-status`, `#fs-menu`, `#fs-modal`,
  `#fs-modal-input`, `#fs-preview`, `#fs-preview-name` (the popup header),
  `#fs-preview-kind` (the footer naming the kind), `#fs-search`, `#fs-toasts`,
  `#fs-uploads`
- `index.html` uses an empty `data:` icon so Chromium does not request
  `/favicon.ico` (which would show up as a console error)
- drag-out is checked by dispatching a synthetic `dragstart` with a real
  `DataTransfer` and reading the formats back off it — Playwright cannot drop
  onto the OS, so the payload is the only observable half of that path
- rows are not focusable, so Enter goes to whatever control still has focus;
  tests that press Enter on a row must `blur()` first if they clicked a button
  just before

## Bugs this suite found

All of these were found by running the suite and are covered by tests.

1. **Fire-and-forget tasks were cancelled.** `spawn()` attaches a task to the
   current component's scope, and dioxus cancels it when that component is
   dropped. Handlers that change state which unmounts them (grid → "loading…",
   menu closing, modal closing) therefore did nothing: double-clicking a folder,
   the modal OK button, and the context menu actions all silently failed. Fixed
   by `state.rs::spawn_task`, which uses the root scope (`spawn_forever`).
2. **The delete modal sent the wrong paths.** It passed names relative to the
   current folder to `/api/delete`, which takes store-root-relative paths — so
   deleting a file in a subfolder targeted a same-named file at the root.
3. **The background context menu was unreachable.** An overlay div covering the
   grid swallowed every right-click, so the menu only ever opened on rows.
4. **Internal drags always copied.** `DataTransfer::get_data()` returns
   `Some("")` for formats that were never set, so `is_some()` was always true
   and Ctrl-less drags copied instead of moving.
5. **The ↑ button panicked.** It held a read guard on `st.path` while
   `navigate()` wrote the same signal — a re-entrant signal borrow, which shows
   up in wasm as `unreachable`.
6. **`is_previewable()` advertised SVG** but `/api/preview` refuses SVG (script
   risk), so the menu offered Preview and then showed "not previewable". The
   allow-list now matches the server — and it matches structurally, because it is
   no longer a second copy of the list: the table is `common-rs/preview`, one
   crate both halves read. SVG is previewable now, as an `<img>` (which cannot run
   script), with `nosniff` and a `default-src 'none'` CSP on the response for the
   case where the same URL is opened as a document instead.
7. **Shallow search reported `scanned: 0`** (and never `truncated`), so the
   results header always claimed nothing had been scanned.
8. **`/api/list` returned 400 for a missing directory** while every other
   endpoint returns 404.
9. **Arrow-key navigation in the icon grid moved one item per press.**
   `state.rs::js_eval_string` only sees *string* results, so the JS expression
   that reads the rendered `auto-fill` column count returned `None` (it returned a
   number) and the grid fell back to 1 column. The expression now returns text.
10. **The context menu could be painted partly off screen.** It was positioned at
    the click point, so a click near the bottom or right edge pushed it past the
    viewport. It is now clamped, which can only be measured after the first paint
    (the test waits for the corrected position).
11. **Enter did nothing.** The global key handler had no Enter branch, so the
    keyboard could move the selection but not open it. It now opens the focused
    row (folder → navigate, previewable file → preview, anything else →
    download), and leaves Enter alone when a text field or button has focus so
    the search box and the modal input keep working.
12. **Dragging a row out of the browser dropped nothing.** Only
    `application/x-filestore` was set, which nothing outside the page understands,
    so the target refused the drop. Rows now also publish `text/uri-list` (and
    `text/plain`) with the download URL — the ZIP URL for a folder, since a
    folder has no single file. A page can only hand an OS target a URL or bytes it
    already holds, so Finder turns that into a `.webloc`; the drag also carries
    `text/html` with the file name as the link text, because Finder names the
    shortcut from the link text and was calling every one
    `filestore.int.leighhack.org:.webloc`.
13. **Uploads over the limit showed "HTTP 413".** The upload route streams the
    raw body, so axum's `DefaultBodyLimit` never applied to it, and nginx's
    default `client_max_body_size` (10m) rejected anything bigger with an HTML
    error page that the upload popup then printed verbatim. The server now
    enforces `--max-upload` (2G in production) and answers with the JSON error
    the popup expects, and the vhost raises nginx's limit to match.
14. **A media preview re-downloaded the whole file on every seek.** `/api/preview`
    ignored `Range`, so scrubbing a video or an audio file transferred the file
    again from the NAS. It now answers a single range with 206 + `Content-Range`
    and advertises `Accept-Ranges`.

## What the preview tests check

The table is shared, so the tests check both halves of it: that each kind is
offered by the menu, rendered in the element a browser can actually show (`img`,
`video`, `audio`, `iframe`, `pre`), served with the content type the table gives,
and that a media response answers a `Range` request. The audio fixture is a real
1 s WAV, so that test proves the element decodes rather than merely exists; the
video fixture is deliberately not decodable — there it is the element and the
code that matter, and a headless Chromium cannot draw a PDF viewer either.

Types a browser cannot render are not in the table (`image/tiff`, `image/heic`,
`video/x-msvideo`, Office documents), so the UI does not promise a preview for
them; a file with an unknown extension is not advertised but the API still tries
it as text, which the suite checks both ways.

## What the thumbnail tests check

The icon grid shows a thumbnail for image rows, generated on demand and cached in
temporary storage (`run.sh` points `--thumb-cache` at the scratch directory and
exports it as `FS_TEST_THUMB_CACHE`, so the tests can look inside it).  The tests
check the parts that are easy to get wrong:

- **It is a real thumbnail** — a decodable fixture (a 256x128 BMP, since a 1 px PNG
cannot show resizing) comes back as a JPEG resized to `--thumb-max-side`, and the
second request for the same file state is a cache hit (`X-Thumb-Cache`).
- **It is never stale** — the cache key is a hash of the file's identity
(`mtime + ctime + size + inode`), so changing the file changes the key and the
entry served is generated from the file as it is now.  The test rewrites the
fixture and asserts the served thumbnail is the new file, and that a hand-built URL
with an old fingerprint regenerates rather than serving the old entry (`v` is only
there to bust the browser cache; the key is always recomputed from the current
stat).
- **The browser cache is keyed on the same identity** — the response is
`max-age=31536000, immutable` with the identity as its ETag, so a stored response
can only be reused for the same file state.  Revalidating with the current ETag is
a 304; an ETag from another state is not honoured.
- **A file the decoder cannot read falls back to the emoji**, and the failure is
cached as a marker so a broken image is not re-decoded on every render.
- **The cap holds** — with `--thumb-cache-max` set to about three thumbnails, five
files stay under it and at least one entry is pruned.

Two things the suite has to allow for:

- **A refused thumbnail is a normal outcome**, so a failed `/api/thumb` request is
not counted as a page console error (the runner filters that one URL).  Without the
exception, any test that renders a folder containing an undecodable image fails.
- **The fixture is rebuilt for every test**, which can hand a test the same inode
and the same second as the previous one, so the tests do not assume the cache is
empty at the start — they use a file the test itself creates, or compare entries
rather than asserting hit/miss.

The row that shows a thumbnail is still identified by `data-fs-name`, so the tests
read the rendered `img`'s `src` and compare it with the URL the SPA should build.

## Keyboard and URL behaviour the suite covers

- **Type-ahead** — a bare letter selects the entry in the current folder that
  starts with it, and repeat presses cycle through the matches in the order the
  grid shows them (directories first, so `p` gives `Photos` before `pandas.csv`).
  The test checks the cycle, that a different letter restarts, and that the search
  box keeps its own typing.
- **Preview stepping** — left/right move to the previous/next previewable file in
  the folder and clamp at the ends rather than wrapping; the row selection follows,
  so closing the popup leaves you on the file you last looked at.
- **URL hash** — the current folder is mirrored into the hash with
  `replaceState`, because the app keeps its own back/forward history (`hist` /
  `hidx`, Backspace) and the browser must not build a second one that disagrees
  with it. A reload lands back in the folder, including a percent-encoded name,
  and the restored folder is the current history entry rather than a step back to
  a root the user never visited.
