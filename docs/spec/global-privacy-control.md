# Global Privacy Control

Global Privacy Control (GPC) tells a website that its visitor does not want
their data sold or shared. Some laws oblige a website to honour it; whether
one does is up to the website.

## The setting

Settings → Data & privacy → "Tell websites not to sell or share my data".
Off at first. Not synced.

## What is sent

While it is on:

- every request to a web address (`http` and `https`) carries the header
  `Sec-GPC: 1`: a page's document, what the page loads, and the requests of
  its service workers;
- `navigator.globalPrivacyControl` is `true` in a web page, set before the
  page's own scripts run.

While it is off neither is there: no header, and the property is undefined,
as in a browser without the signal.

- The switch counts at once for requests. An open page gets the property
  with its next document (a reload, a new address).
- Magnetite's own pages, the pages of extensions and Chromium's pages get
  neither: the signal goes by the address asked for. A request an
  extension makes to a web address carries the header as any other.
- The older "Do Not Track" header is not sent.

Chromium has no such signal of its own: the header is added on the way out
and the property by the script a page runs before its own, the one the ad
blocker's scriptlets use (`ad-blocking.md`).

## Not built yet

- The property in a frame that Chromium runs in a process of its own (a
  frame of another site): such a frame gets the header only.
- `/.well-known/gpc.json` is a matter of websites, not of the browser.

Code: `crates/engine/src/privacy.rs`, `crates/engine/src/blocker.rs`
(`resource_request_handler`, `page_script`).
