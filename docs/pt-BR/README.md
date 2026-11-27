# Brazilian Portuguese pages

These are the Brazilian Portuguese pages of veil's documentation on
[ordreth.com/apps/veil/docs](https://ordreth.com/apps/veil/docs), one file per page, named by the page's slug. They
are written here: this repository is the only source of everything about veil, in every language. The website
copies them as they are with `pnpm sync:veil` and never edits them; ordreth-ops holds no copy.

- Each file translates the English page the website builds from the file named in `source` (a section of the
  README becomes a page of its own, so a page and a file do not always match one to one).
- The front matter has `title` and `description` in Portuguese, `group`, `order` and `source` as in the
  English page, and `source_hash`: the hash of the English page the translation was made from. The website's
  `pnpm i18n:check` fails when the English page has changed since.
- After bringing a translation up to date, run `pnpm i18n:stamp veil/<slug>` in the website: it writes the new
  `source_hash` into this file and into the website's copy. Then commit it here.
- Links point to the website's routes (`/apps/veil/docs/<slug>`): these pages are read there.
- A page without a file here (the license) is shown in English, with a notice.
- This README is not a page; the sync skips it.
