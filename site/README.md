# agent-utils website

The public website, published by GitHub Pages at <https://rrnewton.github.io/agent-utils/>. It
is the overview layer: the suite's purpose, tools and architecture, with links down into the
repository's own documents, which stay the authority.

| File | What it is |
|---|---|
| `index.html` | Overview: why the suite exists, the architecture figure, the tools, adoption, engineering. |
| `architecture.html` | Layers, a delegated task end to end, the harness directory, where state lives. |
| `tools.html` | One section per tool, anchored by tool name (`tools.html#wrkslots`). |
| `style.css` | The only stylesheet, with light and dark palettes. |
| `favicon.svg` | The site icon. |
| `.nojekyll` | Tells a branch-based Pages build not to run Jekyll. The Actions deployment ignores it. |

## Rules

- Hand-written HTML and one stylesheet. No build step, no frameworks, and no external resources
  of any kind: no web fonts, CDNs, analytics or trackers. The pages must read well with
  JavaScript off; the only script stores the System/Light/Dark theme choice.
- Every page carries the same Content-Security-Policy `<meta>`, which allows this origin and the
  hash of the inline theme script, and nothing else. If you change that script,
  `scripts/check_site.py` prints the hash to put in every page's policy. Inline `style`
  attributes, `<style>` elements and event-handler attributes are refused for the same reason.
- Pages share one header, skip link and footer. A new page copies them from an existing page and
  is added to every page's navigation.
- Link pages to each other relatively. Link repository content as
  `https://github.com/rrnewton/agent-utils/blob/main/<path>` for files and `/tree/main/<dir>`
  for directories, and only to tracked paths that are not symlinks.
- Introduce a tool by the user's problem, the useful outcome, then its dependencies, and state its
  maturity the way its own documents do. This is a public page: never name a project that
  consumes agent-utils, and keep host- or organisation-specific details out.
- Every image has `alt`. GIFs stay at or under 3 MB.

## Check and preview

```sh
python3 scripts/check_site.py --self-test
python3 scripts/check_site.py
python3 -m http.server --directory site 8000    # then open http://127.0.0.1:8000/
```

Preview over HTTP rather than `file://`: the page policy allows only `'self'`, which some
browsers do not grant to local files. `make validate` runs the checker on every change, because
renaming a document or rewording a heading anywhere in the repository can break a link here.

## Deploy

`.github/workflows/pages.yml` runs the checker and publishes this directory on every push to
`main` that touches `site/` or the workflow, and on manual dispatch. It needs one repository
setting: Settings, Pages, Build and deployment, Source: GitHub Actions.
